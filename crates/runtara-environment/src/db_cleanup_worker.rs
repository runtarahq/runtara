// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Background worker for cleaning up old database records.
//!
//! Terminal instances (completed, failed, cancelled) older than the configured
//! retention period are deleted along with all related records. A finished
//! child of a `control:start` parent is kept until the parent is terminal and
//! both are past retention; external outcomes of children that never launched
//! follow the same rule, aged by publication.
//!
//! A pinned child that retention would already have deleted (terminal and
//! past retention by its own finish, parent still running or parked) is
//! pruned instead: its checkpoints, signals, closed input requests, park,
//! invocation lease and attempts, `input` and `stderr` go, and its row,
//! outcome, events and accepted input receipts stay, so `get` and `wait`
//! read the same result. The prune runs after the deletion pass on the same
//! `RUNTARA_DB_CLEANUP_MAX_AGE_DAYS` window and batch size.
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

use crate::config::{ProcessEnv, Vars, days, parse_enabled, positive};
use chrono::Utc;
use runtara_core::persistence::{Persistence, RetentionCursor};
use sqlx::PgPool;
use tokio::sync::Notify;
use tracing::{debug, info, warn};

use crate::error::Result;
use crate::periodic::PeriodicLoop;

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
        Self::from_vars(&ProcessEnv)
    }

    /// [`Self::from_env`] against a supplied set of values.
    pub(crate) fn from_vars(vars: &dyn Vars) -> Self {
        Self {
            enabled: parse_enabled(vars.get("RUNTARA_DB_CLEANUP_ENABLED").as_deref()),
            poll_interval: Duration::from_secs(positive(
                vars,
                "RUNTARA_DB_CLEANUP_POLL_INTERVAL_SECS",
                3600,
            )),
            max_age: days(positive(vars, "RUNTARA_DB_CLEANUP_MAX_AGE_DAYS", 3)),
            batch_size: positive(vars, "RUNTARA_DB_CLEANUP_BATCH_SIZE", 100),
            debug_event_max_age: debug_event_max_age_from_raw(
                vars.get("RUNTARA_EVENT_DEBUG_RETENTION_HOURS").as_deref(),
            ),
        }
    }
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

/// What one retention pass did.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct RetentionPassStats {
    /// Terminal instances deleted.
    pub deleted: u64,
    /// Terminal children past retention kept because their parent is not
    /// terminal, or finished too recently. Each is counted once per pass.
    pub pinned_terminal_children: u64,
    /// Pages the pass read.
    pub pages: u64,
    /// External outcomes of never-launched children deleted.
    pub outcomes_deleted: u64,
    /// Pinned terminal children of live parents pruned in place; a child
    /// already pruned by an earlier pass is not counted again.
    pub pruned_children: u64,
    /// Pages the prune pass read.
    pub prune_pages: u64,
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

        PeriodicLoop {
            name: "Database cleanup worker",
            poll_interval: self.config.poll_interval,
            shutdown: &self.shutdown,
            eager_first_pass: true,
            pass_error: "Failed to cleanup old instances",
        }
        .run(|| self.run_cleanup_pass())
        .await;
    }

    /// One retention pass: expired instances, then expired debug events.
    ///
    /// Instances first, so anything the instance sweep removes takes its debug
    /// events with it via ON DELETE CASCADE and the event sweep only has to
    /// deal with events belonging to instances that are still live.
    async fn run_cleanup_pass(&self) -> Result<()> {
        self.run_once().await.map(|_| ())
    }

    /// Run one retention pass now and report what it did.
    ///
    /// Deletion first, then pruning, so a child whose parent has just been
    /// released is deleted outright rather than pruned first.
    pub async fn run_once(&self) -> Result<RetentionPassStats> {
        let cutoff = self.retention_cutoff()?;
        let mut stats = self.cleanup_old_instances(cutoff).await?;
        self.prune_pinned_children(cutoff, &mut stats).await?;
        self.cleanup_old_debug_events().await?;
        Ok(stats)
    }

    /// Instances that finished before this are past retention.
    fn retention_cutoff(&self) -> Result<chrono::DateTime<Utc>> {
        Ok(Utc::now()
            - chrono::Duration::from_std(self.config.max_age)
                .map_err(|e| crate::error::Error::Other(format!("Invalid duration: {}", e)))?)
    }

    /// Prune the pinned terminal children that retention would already have
    /// deleted, had their parent not been live: one cursor walk per pass, one
    /// transaction per page. Logs `pruned_children`.
    async fn prune_pinned_children(
        &self,
        cutoff: chrono::DateTime<Utc>,
        stats: &mut RetentionPassStats,
    ) -> Result<()> {
        let mut after: Option<RetentionCursor> = None;
        loop {
            let page = self
                .persistence
                .prune_pinned_terminal(cutoff, after.as_ref(), self.config.batch_size)
                .await?;
            stats.prune_pages += 1;
            stats.pruned_children += page.pruned;
            match page.next {
                Some(next) => after = Some(next),
                None => break,
            }
        }
        if stats.pruned_children > 0 {
            info!(
                pruned_children = stats.pruned_children,
                pages = stats.prune_pages,
                cutoff = %cutoff,
                "Pruned pinned terminal children"
            );
        } else {
            debug!("Prune pass completed, no pinned children to prune");
        }
        Ok(())
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
                .delete_paired_events_older_than(
                    crate::step_vocabulary::workflow_steps(),
                    cutoff,
                    self.config.batch_size,
                )
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

    /// Cleanup old terminal instances, then the external outcomes of
    /// children that never launched.
    ///
    /// One pass walks the terminal instances past retention with a cursor,
    /// so children pinned by a live parent are read once per pass rather
    /// than once per batch; their count is logged as
    /// `pinned_terminal_children`.
    async fn cleanup_old_instances(
        &self,
        cutoff: chrono::DateTime<Utc>,
    ) -> Result<RetentionPassStats> {
        let mut stats = RetentionPassStats::default();
        let mut after: Option<RetentionCursor> = None;

        loop {
            // Get the next page of terminal instances past retention.
            let page = self
                .persistence
                .get_terminal_instances_older_than(cutoff, after.as_ref(), self.config.batch_size)
                .await?;
            stats.pages += 1;
            stats.pinned_terminal_children += page.pinned;
            let instance_ids = page.eligible;

            if !instance_ids.is_empty() {
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

                stats.deleted += deleted;

                debug!(
                    batch_size = batch_size,
                    deleted = deleted,
                    total_deleted = stats.deleted,
                    "Cleaned up batch of instances"
                );
            }

            // A short page ends the pass; a full one carries the cursor on.
            match page.next {
                Some(next) => after = Some(next),
                None => break,
            }
        }

        // Outcomes of children that never launched age like the children:
        // from publication, and only once the parent is terminal or gone.
        loop {
            let deleted = self
                .persistence
                .delete_external_outcomes_older_than(cutoff, self.config.batch_size)
                .await?;
            stats.outcomes_deleted += deleted;
            if self.config.batch_size <= 0 || deleted < self.config.batch_size as u64 {
                break;
            }
        }

        if stats.deleted > 0 || stats.outcomes_deleted > 0 || stats.pinned_terminal_children > 0 {
            info!(
                total_deleted = stats.deleted,
                outcomes_deleted = stats.outcomes_deleted,
                pinned_terminal_children = stats.pinned_terminal_children,
                cutoff = %cutoff,
                "Database cleanup cycle completed"
            );
        } else {
            debug!("Database cleanup cycle completed, no old instances found");
        }

        Ok(stats)
    }

    /// Clean up environment-specific tables that don't have FK cascade.
    async fn cleanup_environment_tables(&self, instance_ids: &[String]) -> Result<()> {
        if instance_ids.is_empty() {
            return Ok(());
        }

        // One transaction across three tables, which is why these DELETEs stay
        // here rather than moving to the registries that own each table:
        // splitting them would let a failure part-way leave an instance whose
        // rows disagree about whether it still exists.
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
    use super::*;
    use crate::config::FixedVars;

    /// Zero is the dangerous value, not merely an odd one: it makes the debug
    /// sweep delete `LIMIT 0` rows in a loop that only exits on a short batch,
    /// which never happens because `0 < 0` is false. That spins on Postgres and
    /// stops shutdown from joining the worker.
    ///
    /// The rule itself is `crate::config::positive_or_default` and is tested
    /// there; what this pins is that this worker's settings go through it.
    #[test]
    fn non_positive_settings_fall_back_to_the_default() {
        let config = DbCleanupWorkerConfig::from_vars(&FixedVars::new([
            ("RUNTARA_DB_CLEANUP_POLL_INTERVAL_SECS", "0"),
            ("RUNTARA_DB_CLEANUP_MAX_AGE_DAYS", "0"),
            ("RUNTARA_DB_CLEANUP_BATCH_SIZE", "-5"),
        ]));

        assert_eq!(config.poll_interval, Duration::from_secs(3600));
        assert_eq!(config.max_age, Duration::from_secs(3 * 24 * 3600));
        assert_eq!(config.batch_size, 100);
    }

    #[test]
    fn positive_settings_are_honoured() {
        let config = DbCleanupWorkerConfig::from_vars(&FixedVars::new([
            ("RUNTARA_DB_CLEANUP_POLL_INTERVAL_SECS", "60"),
            ("RUNTARA_DB_CLEANUP_MAX_AGE_DAYS", "  7  "),
            ("RUNTARA_DB_CLEANUP_BATCH_SIZE", "25"),
        ]));

        assert_eq!(config.poll_interval, Duration::from_secs(60));
        assert_eq!(config.max_age, Duration::from_secs(7 * 24 * 3600));
        assert_eq!(config.batch_size, 25);
    }

    /// The retention window is the one setting here where zero is a real
    /// answer, and it must keep meaning "do not sweep" rather than being
    /// swallowed by the positive-only rule the others follow.
    #[test]
    fn a_zero_retention_window_still_disables_the_sweep() {
        let config = DbCleanupWorkerConfig::from_vars(&FixedVars::new([(
            "RUNTARA_EVENT_DEBUG_RETENTION_HOURS",
            "0",
        )]));

        assert!(config.debug_event_max_age.is_none());
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
