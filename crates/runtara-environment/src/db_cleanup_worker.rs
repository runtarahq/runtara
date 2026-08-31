// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Background worker for cleaning up old database records.
//!
//! Terminal instances (completed, failed, cancelled) older than the configured
//! retention period are deleted along with all related records.
//!
//! The deletion process:
//! 1. Queries for terminal instances older than `max_age`
//! 2. Cleans up environment-specific tables (no FK cascade)
//! 3. Deletes from `instances` table (CASCADE handles core tables)
//!
//! Environment-specific tables cleaned before instance deletion:
//! - `container_registry`
//! - `instance_images`

use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use runtara_core::config::parse_enabled_env;
use runtara_core::persistence::Persistence;
use sqlx::PgPool;
use tokio::sync::Notify;
use tracing::{debug, error, info, warn};

use crate::error::Result;

/// Configuration for the database cleanup worker.
#[derive(Debug, Clone)]
pub struct DbCleanupWorkerConfig {
    /// Whether database cleanup is enabled.
    pub enabled: bool,
    /// How often to run cleanup.
    pub poll_interval: Duration,
    /// Maximum age for terminal instances before cleanup.
    pub max_age: Duration,
    /// Maximum instances to delete per batch (prevents long transactions).
    pub batch_size: i64,
    /// Maximum age for step-debug events before they are swept, independent of
    /// instance retention. `None` disables the sweep.
    pub debug_event_max_age: Option<Duration>,
}

impl Default for DbCleanupWorkerConfig {
    fn default() -> Self {
        Self {
            enabled: true, // Enabled by default — retention is
            // bounded; override via env to disable
            poll_interval: Duration::from_secs(3600), // 1 hour
            max_age: Duration::from_secs(3 * 24 * 3600), // 3 days
            batch_size: 100,
            // Step-debug payloads are the bulk of instance_events and are read
            // while a run is recent, so they age out well before the instance
            // does. A burst that drains a large sleeping population would
            // otherwise pin every debug row for the full instance window.
            debug_event_max_age: Some(Duration::from_secs(24 * 3600)), // 1 day
        }
    }
}

impl DbCleanupWorkerConfig {
    /// Load configuration from environment variables.
    ///
    /// Environment variables:
    /// - `RUNTARA_DB_CLEANUP_ENABLED`: set to `false`/`0`/`no`/`off`
    ///   (case-insensitive) to disable. **Any other value — including unset,
    ///   typos, or `"yes"`/`"on"` — leaves cleanup enabled.** Cleanup is on
    ///   by default; only an explicit opt-out turns it off.
    /// - `RUNTARA_DB_CLEANUP_POLL_INTERVAL_SECS`: seconds between cleanup runs (default: 3600)
    /// - `RUNTARA_DB_CLEANUP_MAX_AGE_DAYS`: days before terminal instances are deleted (default: 3)
    /// - `RUNTARA_DB_CLEANUP_BATCH_SIZE`: max instances per batch (default: 100)
    /// - `RUNTARA_EVENT_DEBUG_RETENTION_HOURS`: hours before step-debug events
    ///   are swept, independently of instance retention (default: 24). `0`
    ///   disables the sweep, leaving debug events to age out with their
    ///   instance as before.
    pub fn from_env() -> Self {
        let enabled = parse_enabled_env("RUNTARA_DB_CLEANUP_ENABLED");

        let poll_interval_secs = positive_or_default(
            std::env::var("RUNTARA_DB_CLEANUP_POLL_INTERVAL_SECS")
                .ok()
                .as_deref(),
            3600,
        );

        let max_age_days = positive_or_default(
            std::env::var("RUNTARA_DB_CLEANUP_MAX_AGE_DAYS")
                .ok()
                .as_deref(),
            3,
        );

        let batch_size = positive_or_default(
            std::env::var("RUNTARA_DB_CLEANUP_BATCH_SIZE")
                .ok()
                .as_deref(),
            100,
        );

        let debug_event_max_age = debug_event_max_age_from_raw(
            std::env::var("RUNTARA_EVENT_DEBUG_RETENTION_HOURS")
                .ok()
                .as_deref(),
        );

        Self {
            enabled,
            poll_interval: Duration::from_secs(poll_interval_secs),
            max_age: Duration::from_secs(max_age_days * 24 * 3600),
            batch_size,
            debug_event_max_age,
        }
    }
}

/// Parse a positive setting, falling back to `default` for anything that is
/// absent, unparseable, or non-positive.
///
/// Zero is rejected rather than honoured: a zero batch size makes the sweep
/// delete `LIMIT 0` rows forever, because the loop only stops when a pass comes
/// back short of a full batch and `0 < 0` never does. A zero poll interval
/// spins the same way. Both hammer Postgres and stop shutdown from joining the
/// worker, so neither is a value anyone can usefully ask for.
fn positive_or_default<T>(raw: Option<&str>, default: T) -> T
where
    T: std::str::FromStr + PartialOrd + Default + Copy,
{
    raw.and_then(|v| v.trim().parse::<T>().ok())
        .filter(|parsed| *parsed > T::default())
        .unwrap_or(default)
}

/// Step-debug retention window from `RUNTARA_EVENT_DEBUG_RETENTION_HOURS`.
///
/// `None` disables the sweep, leaving debug events to age out with their
/// instance as before. An explicit `0` means exactly that; an unset or
/// unparseable value keeps the 24-hour default rather than silently turning
/// retention off.
///
/// Split from the environment read so it can be tested without mutating
/// process-global state shared by every test in the binary.
fn debug_event_max_age_from_raw(raw: Option<&str>) -> Option<Duration> {
    match raw.map(str::trim) {
        None => Some(Duration::from_secs(24 * 3600)),
        Some(v) => match v.parse::<u64>() {
            Ok(0) => None,
            Ok(hours) => Some(Duration::from_secs(hours * 3600)),
            Err(_) => Some(Duration::from_secs(24 * 3600)),
        },
    }
}

/// Background worker that cleans up old database records.
pub struct DbCleanupWorker {
    pool: PgPool,
    persistence: Arc<dyn Persistence>,
    config: DbCleanupWorkerConfig,
    shutdown: Arc<Notify>,
}

impl DbCleanupWorker {
    /// Create a new database cleanup worker.
    pub fn new(
        pool: PgPool,
        persistence: Arc<dyn Persistence>,
        config: DbCleanupWorkerConfig,
    ) -> Self {
        Self {
            pool,
            persistence,
            config,
            shutdown: Arc::new(Notify::new()),
        }
    }

    /// Get a handle that can be used to signal shutdown.
    pub fn shutdown_handle(&self) -> Arc<Notify> {
        self.shutdown.clone()
    }

    /// Run the cleanup worker loop.
    ///
    /// This will periodically scan for and remove old terminal instances.
    /// The loop exits when the shutdown signal is received.
    pub async fn run(&self) {
        if !self.config.enabled {
            info!("Database cleanup worker disabled");
            return;
        }

        info!(
            poll_interval_secs = self.config.poll_interval.as_secs(),
            max_age_days = self.config.max_age.as_secs() / 86400,
            batch_size = self.config.batch_size,
            "Database cleanup worker started"
        );

        // Eager first pass: enforce retention immediately on startup so that
        // cleanup runs even when the server restarts more frequently than
        // `poll_interval`. Race against the shutdown signal so a slow or
        // hanging cleanup (e.g. unreachable DB) cannot block shutdown.
        tokio::select! {
            biased;

            _ = self.shutdown.notified() => {
                info!("Database cleanup worker received shutdown signal during eager pass");
                return;
            }

            res = self.run_cleanup_pass() => {
                if let Err(e) = res {
                    error!(error = %e, "Failed to cleanup old instances");
                }
            }
        }

        loop {
            tokio::select! {
                biased;

                _ = self.shutdown.notified() => {
                    info!("Database cleanup worker received shutdown signal");
                    break;
                }

                _ = tokio::time::sleep(self.config.poll_interval) => {
                    if let Err(e) = self.run_cleanup_pass().await {
                        error!(error = %e, "Failed to cleanup old instances");
                    }
                }
            }
        }

        info!("Database cleanup worker stopped");
    }

    /// One retention pass: expired instances, then expired debug events.
    ///
    /// Instances first, so anything the instance sweep removes takes its debug
    /// events with it via ON DELETE CASCADE and the event sweep only has to
    /// deal with events belonging to instances that are still live.
    async fn run_cleanup_pass(&self) -> Result<()> {
        self.cleanup_old_instances().await?;
        self.cleanup_old_debug_events().await
    }

    /// Sweep step-debug events past their own, shorter retention window.
    ///
    /// Separate from instance retention because these rows dominate
    /// `instance_events` while being useful only while a run is recent. The
    /// run's lifecycle events and its `instances` row are untouched, so history
    /// and status survive for the full instance window; only step-level detail
    /// ages out early. Instrumentation is unchanged — workflows still record
    /// every step.
    async fn cleanup_old_debug_events(&self) -> Result<()> {
        let Some(max_age) = self.config.debug_event_max_age else {
            return Ok(());
        };

        let cutoff = Utc::now()
            - chrono::Duration::from_std(max_age)
                .map_err(|e| crate::error::Error::Other(format!("Invalid duration: {}", e)))?;

        let mut total_deleted = 0u64;
        loop {
            let deleted = self
                .persistence
                .delete_debug_events_older_than(cutoff, self.config.batch_size)
                .await?;
            total_deleted += deleted;

            // Short of a full batch means the backlog is drained. The
            // non-positive guard is belt and braces against a config built
            // directly rather than through `from_env`: with a batch size of
            // zero every pass deletes nothing and `0 < 0` never breaks.
            if self.config.batch_size <= 0 || deleted < self.config.batch_size as u64 {
                break;
            }
        }

        if total_deleted > 0 {
            info!(
                total_deleted = total_deleted,
                cutoff = %cutoff,
                "Step-debug event retention sweep completed"
            );
        } else {
            debug!("Step-debug event retention sweep completed, nothing expired");
        }

        Ok(())
    }

    /// Cleanup old terminal instances.
    async fn cleanup_old_instances(&self) -> Result<()> {
        let cutoff = Utc::now()
            - chrono::Duration::from_std(self.config.max_age)
                .map_err(|e| crate::error::Error::Other(format!("Invalid duration: {}", e)))?;

        let mut total_deleted = 0u64;

        loop {
            // Get batch of instances to delete
            let instance_ids = self
                .persistence
                .get_terminal_instances_older_than(cutoff, self.config.batch_size)
                .await?;

            if instance_ids.is_empty() {
                break;
            }

            let batch_size = instance_ids.len();

            // Clean up environment-specific tables first (no FK cascade)
            if let Err(e) = self.cleanup_environment_tables(&instance_ids).await {
                warn!(
                    error = %e,
                    batch_size = batch_size,
                    "Failed to cleanup environment tables, skipping batch"
                );
                break;
            }

            // Delete from instances table (cascades to Core tables)
            let deleted = self
                .persistence
                .delete_instances_batch(&instance_ids)
                .await?;

            total_deleted += deleted;

            debug!(
                batch_size = batch_size,
                deleted = deleted,
                total_deleted = total_deleted,
                "Cleaned up batch of instances"
            );

            // If we got fewer than batch_size, we're done
            if batch_size < self.config.batch_size as usize {
                break;
            }
        }

        if total_deleted > 0 {
            info!(
                total_deleted = total_deleted,
                cutoff = %cutoff,
                "Database cleanup cycle completed"
            );
        } else {
            debug!("Database cleanup cycle completed, no old instances found");
        }

        Ok(())
    }

    /// Clean up environment-specific tables that don't have FK cascade.
    async fn cleanup_environment_tables(&self, instance_ids: &[String]) -> Result<()> {
        if instance_ids.is_empty() {
            return Ok(());
        }

        // Use a transaction to ensure consistency
        let mut tx = self.pool.begin().await?;

        // container_registry
        sqlx::query("DELETE FROM container_registry WHERE instance_id = ANY($1)")
            .bind(instance_ids)
            .execute(&mut *tx)
            .await?;

        // instance_images
        sqlx::query("DELETE FROM instance_images WHERE instance_id = ANY($1)")
            .bind(instance_ids)
            .execute(&mut *tx)
            .await?;

        tx.commit().await?;

        debug!(
            count = instance_ids.len(),
            "Cleaned up environment tables for instances"
        );

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_default() {
        let config = DbCleanupWorkerConfig::default();
        assert!(config.enabled);
        assert_eq!(config.poll_interval, Duration::from_secs(3600));
        assert_eq!(config.max_age, Duration::from_secs(3 * 24 * 3600));
        assert_eq!(config.batch_size, 100);
    }

    #[test]
    fn test_config_max_age_days() {
        let config = DbCleanupWorkerConfig {
            max_age: Duration::from_secs(3 * 24 * 3600), // 3 days
            ..Default::default()
        };
        assert_eq!(config.max_age.as_secs() / 86400, 3);
    }

    #[test]
    fn test_config_enabled_by_default() {
        let config = DbCleanupWorkerConfig::default();
        assert!(
            config.enabled,
            "Cleanup should be enabled by default; disable via RUNTARA_DB_CLEANUP_ENABLED=false"
        );
    }
}

#[cfg(test)]
mod setting_validation_tests {
    use super::positive_or_default;

    /// Zero is the dangerous value, not merely an odd one: it makes the debug
    /// sweep delete `LIMIT 0` rows in a loop that only exits on a short batch,
    /// which never happens because `0 < 0` is false. That spins on Postgres and
    /// stops shutdown from joining the worker.
    #[test]
    fn non_positive_settings_fall_back_to_the_default() {
        assert_eq!(positive_or_default(Some("0"), 100i64), 100);
        assert_eq!(positive_or_default(Some("-5"), 100i64), 100);
        assert_eq!(positive_or_default(Some(""), 100i64), 100);
        assert_eq!(positive_or_default(Some("not-a-number"), 100i64), 100);
        assert_eq!(positive_or_default(None, 100i64), 100);
        assert_eq!(positive_or_default(Some("0"), 3600u64), 3600);
    }

    #[test]
    fn positive_settings_are_honoured() {
        assert_eq!(positive_or_default(Some("50"), 100i64), 50);
        assert_eq!(positive_or_default(Some("  7  "), 100i64), 7);
        assert_eq!(positive_or_default(Some("1"), 3600u64), 1);
    }
}

#[cfg(test)]
mod retention_window_tests {
    use super::*;

    #[test]
    fn default_window_when_unset_or_malformed() {
        let day = Duration::from_secs(24 * 3600);
        assert_eq!(debug_event_max_age_from_raw(None), Some(day));
        // A typo must not silently disable retention and let the table grow.
        assert_eq!(debug_event_max_age_from_raw(Some("soon")), Some(day));
        assert_eq!(debug_event_max_age_from_raw(Some("")), Some(day));
    }

    #[test]
    fn explicit_zero_disables_the_sweep() {
        assert_eq!(debug_event_max_age_from_raw(Some("0")), None);
    }

    #[test]
    fn hours_are_honoured() {
        assert_eq!(
            debug_event_max_age_from_raw(Some("1")),
            Some(Duration::from_secs(3600))
        );
        assert_eq!(
            debug_event_max_age_from_raw(Some(" 72 ")),
            Some(Duration::from_secs(72 * 3600))
        );
    }

    #[test]
    fn default_config_sweeps_debug_events_sooner_than_instances() {
        let config = DbCleanupWorkerConfig::default();
        let debug = config
            .debug_event_max_age
            .expect("the debug sweep is on by default");
        assert!(
            debug < config.max_age,
            "debug payloads must age out before the instances that own them: \
             {debug:?} vs {:?}",
            config.max_age
        );
    }
}
