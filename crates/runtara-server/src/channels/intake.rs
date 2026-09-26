//! Durable intake for inbound channel messages.
//!
//! A provider webhook is acknowledged only after its message is stored here,
//! keyed on `(tenant, connection, identity)`. The row is the authority for
//! deduplication and for launch identity: an execution started from it uses
//! the intake id as its instance id, so a retried launch is deduplicated by the
//! execution engine. The id is derived from the provider identity, so this
//! holds even for a redelivery that arrives after its row was removed.
//!
//! Rows stay `pending` until the session has handled them. A pending row whose
//! `next_attempt_at` has passed is claimed and dispatched again with backoff,
//! so a transient launch failure is retried without a restart. Handled rows are
//! deleted after [`RETENTION`].

use std::time::Duration;

use chrono::{DateTime, Utc};
use serde_json::Value;
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use uuid::Uuid;

use super::session::InboundMessage;

/// Stable dedup identity for an inbound message. Providers supply one (Slack
/// `event_id`, Telegram `update_id`, Teams activity `id`, Mailgun `Message-Id`);
/// otherwise the raw payload hash still collapses identical redeliveries.
pub fn identity(msg: &InboundMessage) -> String {
    match msg.activity_id.as_deref().filter(|id| !id.is_empty()) {
        Some(id) => format!("id:{id}"),
        None => {
            let payload = serde_json::to_vec(&msg.original_message).unwrap_or_default();
            format!("sha256:{:x}", Sha256::digest(payload))
        }
    }
}

pub enum Accepted {
    New(Uuid),
    Duplicate,
}

/// How long a freshly accepted row is left to its session before the sweep
/// may dispatch it again. Handling normally completes within a second.
pub const HANDOFF_GRACE: Duration = Duration::from_secs(120);

/// A row claimed this many times without being handled is failed.
pub const MAX_ATTEMPTS: i32 = 12;

/// Handled rows are kept this long so redeliveries are still recognised.
/// It must exceed every provider's redelivery window (Telegram retries an
/// unacknowledged update for up to a day). A redelivery after deletion still
/// maps to the same instance id and is deduplicated by the execution engine.
pub const RETENTION: Duration = Duration::from_secs(7 * 24 * 3600);

/// Backoff after the `attempt`-th claim: 30s doubling, at most an hour.
fn backoff(attempt: i32) -> Duration {
    let exponent = attempt.clamp(1, 8) as u32 - 1;
    Duration::from_secs((30u64 << exponent).min(3600))
}

/// The workflow and version a message launches, fixed when it is accepted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PinnedWorkflow {
    pub id: String,
    pub version: i32,
}

/// The request a reply was bound to when it arrived, recorded before the reply
/// is buffered. `payload` is exactly what is handed to the managed queue.
#[derive(Debug, Clone, PartialEq)]
pub struct ReplyBinding {
    pub session_id: String,
    pub instance_id: String,
    pub request_id: String,
    pub payload: Value,
}

pub struct PendingIntake {
    pub intake_id: Uuid,
    pub connection_id: String,
    pub message: InboundMessage,
    /// Set when the message was a bound reply: recovery delivers it to that
    /// request and never launches a run from it.
    pub reply: Option<ReplyBinding>,
}

#[derive(Clone)]
pub struct IntakeStore {
    pool: PgPool,
}

impl IntakeStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Record a verified inbound message for `workflow`, which every launch
    /// from this row uses. `Duplicate` means the same provider identity
    /// is already stored, so the delivery must be acknowledged and dropped.
    /// Any error means nothing was stored and the provider must retry.
    pub async fn accept(
        &self,
        tenant_id: &str,
        connection_id: &str,
        trigger_id: &str,
        workflow: &PinnedWorkflow,
        msg: &InboundMessage,
    ) -> sqlx::Result<Accepted> {
        let identity = identity(msg);
        // Derived, not random: a redelivery arriving after its row is gone
        // still maps to the same instance id, which the engine deduplicates.
        let intake_id = Uuid::new_v5(
            &Uuid::NAMESPACE_URL,
            format!("channel-intake:{tenant_id}:{connection_id}:{identity}").as_bytes(),
        );
        let message = serde_json::to_value(msg).map_err(|e| sqlx::Error::Encode(Box::new(e)))?;
        let inserted: Option<Uuid> = sqlx::query_scalar(
            "INSERT INTO channel_intake
                 (intake_id, tenant_id, connection_id, identity, trigger_id, workflow_id, workflow_version, message, next_attempt_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, NOW() + make_interval(secs => $9))
             ON CONFLICT DO NOTHING
             RETURNING intake_id",
        )
        .bind(intake_id)
        .bind(tenant_id)
        .bind(connection_id)
        .bind(identity)
        .bind(trigger_id)
        .bind(&workflow.id)
        .bind(workflow.version)
        .bind(message)
        .bind(HANDOFF_GRACE.as_secs_f64())
        .fetch_optional(&self.pool)
        .await?;
        Ok(match inserted {
            Some(id) => Accepted::New(id),
            None => Accepted::Duplicate,
        })
    }

    /// The message reached its destination: an execution was queued, or the
    /// reply was buffered, refused with a notice, or consumed by collection.
    pub async fn mark_processed(
        &self,
        intake_id: Uuid,
        outcome: &str,
        instance_id: Option<&str>,
    ) -> sqlx::Result<()> {
        sqlx::query(
            "UPDATE channel_intake
             SET status = 'processed', outcome = $2, instance_id = $3, updated_at = NOW()
             WHERE intake_id = $1 AND status = 'pending'",
        )
        .bind(intake_id)
        .bind(outcome)
        .bind(instance_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Record the request a reply answers, before it is buffered. The row stays
    /// pending until the reply is handed to the managed queue.
    pub async fn bind_reply(&self, intake_id: Uuid, binding: &ReplyBinding) -> sqlx::Result<()> {
        sqlx::query(
            "UPDATE channel_intake
             SET reply_session_id = $2, reply_instance_id = $3, reply_request_id = $4,
                 reply_payload = $5, updated_at = NOW()
             WHERE intake_id = $1 AND status = 'pending'",
        )
        .bind(intake_id)
        .bind(&binding.session_id)
        .bind(&binding.instance_id)
        .bind(&binding.request_id)
        .bind(&binding.payload)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// `mark_processed` for a message that may not carry an intake id. A
    /// failed update leaves the row pending, so the sweep dispatches it again
    /// rather than losing it.
    pub async fn settle(&self, intake_id: Option<Uuid>, outcome: &str, instance_id: Option<&str>) {
        let Some(intake_id) = intake_id else {
            return;
        };
        if let Err(error) = self.mark_processed(intake_id, outcome, instance_id).await {
            tracing::warn!(%intake_id, error = %error, "Unable to mark channel intake processed");
        }
    }

    /// The message can never be handled (no route, invalid input); stop
    /// dispatching it.
    pub async fn mark_failed(&self, intake_id: Uuid, error: &str) -> sqlx::Result<()> {
        sqlx::query(
            "UPDATE channel_intake
             SET status = 'failed', last_error = $2, updated_at = NOW()
             WHERE intake_id = $1 AND status = 'pending'",
        )
        .bind(intake_id)
        .bind(error)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Make pending rows accepted before `before` due now. Called at startup
    /// with the process start time: no such row is in flight in this process.
    pub async fn release_orphans(
        &self,
        tenant_id: &str,
        before: DateTime<Utc>,
    ) -> sqlx::Result<u64> {
        let result = sqlx::query(
            "UPDATE channel_intake SET next_attempt_at = NOW()
             WHERE tenant_id = $1 AND status = 'pending' AND created_at < $2
               AND next_attempt_at > NOW()",
        )
        .bind(tenant_id)
        .bind(before)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected())
    }

    /// Claim up to `limit` due pending rows, oldest first, pushing each one's
    /// next attempt out by its backoff. Rows claimed [`MAX_ATTEMPTS`] times are
    /// failed instead. `SKIP LOCKED` keeps concurrent sweeps from sharing rows.
    pub async fn claim_due(&self, tenant_id: &str, limit: i64) -> sqlx::Result<Vec<PendingIntake>> {
        sqlx::query(
            "UPDATE channel_intake
             SET status = 'failed', last_error = 'dispatch attempts exhausted', updated_at = NOW()
             WHERE tenant_id = $1 AND status = 'pending' AND attempts >= $2 AND next_attempt_at <= NOW()",
        )
        .bind(tenant_id)
        .bind(MAX_ATTEMPTS)
        .execute(&self.pool)
        .await?;
        type Row = (
            Uuid,
            String,
            String,
            i32,
            Value,
            i32,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<Value>,
        );
        let rows: Vec<Row> = sqlx::query_as(
            "UPDATE channel_intake AS row
             SET attempts = row.attempts + 1, updated_at = NOW()
             FROM (SELECT intake_id FROM channel_intake
                   WHERE tenant_id = $1 AND status = 'pending' AND next_attempt_at <= NOW()
                   ORDER BY created_at, intake_id
                   LIMIT $2
                   FOR UPDATE SKIP LOCKED) AS due
             WHERE row.intake_id = due.intake_id
             RETURNING row.intake_id, row.connection_id, row.workflow_id, row.workflow_version,
                       row.message, row.attempts, row.reply_session_id, row.reply_instance_id,
                       row.reply_request_id, row.reply_payload",
        )
        .bind(tenant_id)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        let mut claimed = Vec::with_capacity(rows.len());
        for (
            intake_id,
            connection_id,
            workflow_id,
            version,
            message,
            attempts,
            reply_session,
            reply_instance,
            reply_request,
            reply_payload,
        ) in rows
        {
            let reply = match (reply_session, reply_instance, reply_request, reply_payload) {
                (Some(session_id), Some(instance_id), Some(request_id), Some(payload)) => {
                    Some(ReplyBinding {
                        session_id,
                        instance_id,
                        request_id,
                        payload,
                    })
                }
                _ => None,
            };
            sqlx::query(
                "UPDATE channel_intake SET next_attempt_at = NOW() + make_interval(secs => $2)
                 WHERE intake_id = $1",
            )
            .bind(intake_id)
            .bind(backoff(attempts).as_secs_f64())
            .execute(&self.pool)
            .await?;
            match serde_json::from_value::<InboundMessage>(message) {
                Ok(mut message) => {
                    message.intake_id = Some(intake_id);
                    message.workflow = Some(PinnedWorkflow {
                        id: workflow_id,
                        version,
                    });
                    claimed.push(PendingIntake {
                        intake_id,
                        connection_id,
                        message,
                        reply,
                    });
                }
                Err(error) => {
                    self.mark_failed(intake_id, &format!("unreadable message: {error}"))
                        .await?;
                }
            }
        }
        Ok(claimed)
    }

    /// Delete up to `limit` handled rows last updated more than `retention` ago.
    /// Pending rows are never deleted.
    pub async fn purge(
        &self,
        tenant_id: &str,
        retention: Duration,
        limit: i64,
    ) -> sqlx::Result<u64> {
        let result = sqlx::query(
            "DELETE FROM channel_intake WHERE intake_id IN (
                 SELECT intake_id FROM channel_intake
                 WHERE tenant_id = $1 AND status <> 'pending'
                   AND updated_at < NOW() - make_interval(secs => $2)
                 LIMIT $3)",
        )
        .bind(tenant_id)
        .bind(retention.as_secs_f64())
        .bind(limit)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_doubles_from_thirty_seconds_up_to_an_hour() {
        assert_eq!(backoff(1), Duration::from_secs(30));
        assert_eq!(backoff(2), Duration::from_secs(60));
        assert_eq!(backoff(5), Duration::from_secs(480));
        assert_eq!(backoff(8), Duration::from_secs(3600));
        assert_eq!(backoff(MAX_ATTEMPTS), Duration::from_secs(3600));
    }
}
