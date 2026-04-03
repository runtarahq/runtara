//! Cron Scheduler Worker
//!
//! Polls the invocation_trigger table for active CRON triggers and publishes
//! trigger events when their schedules match the current time.

use std::time::Duration;

use chrono::{DateTime, Utc};
use croner::Cron;
use serde_json::json;
use sqlx::PgPool;
use tracing::{debug, error, info, instrument, warn};
use uuid::Uuid;

use crate::api::dto::trigger_event::TriggerEvent;
use crate::api::dto::triggers::InvocationTrigger;
use crate::api::repositories::trigger_stream::TriggerStreamPublisher;

/// Configuration for the cron scheduler
#[derive(Debug, Clone)]
pub struct CronSchedulerConfig {
    /// Tenant ID (from TENANT_ID env var)
    pub tenant_id: String,
    /// How often to check for due cron triggers (in seconds)
    pub check_interval_secs: u64,
}

impl Default for CronSchedulerConfig {
    fn default() -> Self {
        Self {
            tenant_id: std::env::var("TENANT_ID").unwrap_or_else(|_| "default".to_string()),
            check_interval_secs: 60, // Check every minute
        }
    }
}

/// Background worker that schedules cron-triggered scenario executions
#[instrument(skip(pool, redis_url))]
pub async fn run(pool: PgPool, redis_url: String, config: CronSchedulerConfig) {
    let scheduler_id = format!("cron-scheduler-{}", Uuid::new_v4());
    let tenant_id = config.tenant_id.clone();

    info!(
        scheduler_id = %scheduler_id,
        tenant_id = %tenant_id,
        check_interval_secs = config.check_interval_secs,
        "Starting cron scheduler"
    );

    let trigger_stream = TriggerStreamPublisher::new(redis_url);

    // Main scheduling loop
    let mut interval = tokio::time::interval(Duration::from_secs(config.check_interval_secs));

    loop {
        interval.tick().await;

        debug!(scheduler_id = %scheduler_id, "Checking for due cron triggers");

        // Get all active CRON triggers for this tenant
        let triggers = match get_active_cron_triggers(&pool, &tenant_id).await {
            Ok(triggers) => triggers,
            Err(e) => {
                error!(error = %e, "Failed to fetch cron triggers");
                continue;
            }
        };

        let now = Utc::now();

        for trigger in triggers {
            // Check if this trigger should run now
            match should_trigger_run(&trigger, now) {
                Ok(true) => {
                    info!(
                        trigger_id = %trigger.id,
                        scenario_id = %trigger.scenario_id,
                        "Cron trigger is due, publishing event"
                    );

                    // Build and publish trigger event
                    if let Err(e) =
                        publish_cron_trigger(&trigger_stream, &trigger, &tenant_id).await
                    {
                        error!(
                            trigger_id = %trigger.id,
                            error = %e,
                            "Failed to publish cron trigger event"
                        );
                        continue;
                    }

                    // Update last_run timestamp
                    if let Err(e) = update_last_run(&pool, &trigger.id).await {
                        warn!(
                            trigger_id = %trigger.id,
                            error = %e,
                            "Failed to update last_run timestamp"
                        );
                    }
                }
                Ok(false) => {
                    // Not due yet, skip
                    debug!(
                        trigger_id = %trigger.id,
                        "Cron trigger not due yet"
                    );
                }
                Err(e) => {
                    warn!(
                        trigger_id = %trigger.id,
                        error = %e,
                        "Failed to evaluate cron expression"
                    );
                }
            }
        }
    }
}

/// Get all active CRON triggers for a tenant
async fn get_active_cron_triggers(
    pool: &PgPool,
    tenant_id: &str,
) -> Result<Vec<InvocationTrigger>, sqlx::Error> {
    sqlx::query_as::<_, InvocationTrigger>(
        r#"
        SELECT id, tenant_id, scenario_id, trigger_type, active, configuration,
               created_at, last_run, updated_at, remote_tenant_id, single_instance
        FROM invocation_trigger
        WHERE (tenant_id = $1 OR tenant_id IS NULL)
          AND trigger_type = 'CRON'
          AND active = true
        "#,
    )
    .bind(tenant_id)
    .fetch_all(pool)
    .await
}

/// Check if a cron trigger should run based on its schedule and last_run time
fn should_trigger_run(trigger: &InvocationTrigger, now: DateTime<Utc>) -> Result<bool, String> {
    // Get cron expression from configuration
    let cron_expr = trigger
        .configuration
        .as_ref()
        .and_then(|c| c.get("expression"))
        .and_then(|e| e.as_str())
        .ok_or_else(|| "Missing 'expression' in trigger configuration".to_string())?;

    // Parse cron expression
    let cron = Cron::new(cron_expr)
        .parse()
        .map_err(|e| format!("Invalid cron expression '{}': {}", cron_expr, e))?;

    // Determine the reference time for finding next occurrence
    let reference_time = trigger.last_run.unwrap_or(trigger.created_at);

    // Find the next occurrence after the reference time
    let next_occurrence = cron
        .find_next_occurrence(&reference_time, false)
        .map_err(|e| format!("Failed to calculate next occurrence: {}", e))?;

    // Trigger should run if the next occurrence is at or before now
    Ok(next_occurrence <= now)
}

/// Publish a cron trigger event to the stream
async fn publish_cron_trigger(
    trigger_stream: &TriggerStreamPublisher,
    trigger: &InvocationTrigger,
    tenant_id: &str,
) -> Result<(), String> {
    let cron_expr = trigger
        .configuration
        .as_ref()
        .and_then(|c| c.get("expression"))
        .and_then(|e| e.as_str())
        .unwrap_or("unknown");

    // Get inputs from configuration (if any)
    let inputs = trigger
        .configuration
        .as_ref()
        .and_then(|c| c.get("inputs"))
        .cloned()
        .unwrap_or_else(|| json!({}));

    // Read debug flag from trigger configuration
    let debug = trigger
        .configuration
        .as_ref()
        .and_then(|c| c.get("debug"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    // Generate instance ID
    let instance_id = Uuid::new_v4();

    // Build TriggerEvent
    let event = TriggerEvent::cron(
        instance_id.to_string(),
        tenant_id.to_string(),
        trigger.scenario_id.clone(),
        None, // use current version
        inputs,
        false, // track_events
        trigger.id.clone(),
        cron_expr.to_string(),
        debug,
    );

    // Publish to stream
    trigger_stream
        .publish(tenant_id, &event)
        .await
        .map_err(|e| format!("Failed to publish to stream: {}", e))?;

    info!(
        instance_id = %instance_id,
        trigger_id = %trigger.id,
        scenario_id = %trigger.scenario_id,
        cron_expr = %cron_expr,
        "Published cron trigger event"
    );

    Ok(())
}

/// Update the last_run timestamp on a trigger
async fn update_last_run(pool: &PgPool, trigger_id: &str) -> Result<(), sqlx::Error> {
    sqlx::query!(
        "UPDATE invocation_trigger SET last_run = NOW() WHERE id = $1",
        trigger_id
    )
    .execute(pool)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Timelike};

    #[test]
    fn test_cron_parsing() {
        // Test basic cron expression
        let cron = Cron::new("0 * * * *").parse().unwrap();
        let now = Utc.with_ymd_and_hms(2025, 1, 15, 10, 30, 0).unwrap();
        let next = cron.find_next_occurrence(&now, false).unwrap();

        // Should be 11:00
        assert_eq!(next.hour(), 11);
        assert_eq!(next.minute(), 0);
    }

    #[test]
    fn test_every_minute_cron() {
        let cron = Cron::new("* * * * *").parse().unwrap();
        let now = Utc.with_ymd_and_hms(2025, 1, 15, 10, 30, 0).unwrap();
        let next = cron.find_next_occurrence(&now, false).unwrap();

        // Should be 10:31
        assert_eq!(next.hour(), 10);
        assert_eq!(next.minute(), 31);
    }
}
