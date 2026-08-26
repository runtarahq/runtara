// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Background worker for retaining server-side invocation data.
//!
//! Prunes aggregated `workflow_metrics_hourly` rows past their retention
//! window, and sweeps expired `oauth_state` CSRF rows in the same cycle — the
//! consume path only deletes the single row it matches, so abandoned
//! authorization flows would otherwise accumulate forever.
//!
//! Execution rows are not swept here: runtara-environment owns instance
//! history, and the server-side `workflow_executions` tables this worker used
//! to prune were dropped once it became clear nothing had written them since
//! execution moved out of the server.

use std::time::Duration;

use chrono::Utc;
use runtara_connections::repository::oauth::OAuthRepository;
use runtara_core::config::parse_enabled_env;
use sqlx::PgPool;
use tracing::{debug, error, info, warn};

use crate::shutdown::ShutdownSignal;

/// Configuration for the invocation cleanup worker.
#[derive(Debug, Clone)]
pub struct InvocationCleanupWorkerConfig {
    /// Whether cleanup is enabled.
    pub enabled: bool,
    /// How often to run the cleanup cycle.
    pub poll_interval: Duration,
    /// Retention window for aggregated metrics (`workflow_metrics_hourly`).
    pub metrics_max_age: Duration,
}

impl Default for InvocationCleanupWorkerConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            poll_interval: Duration::from_secs(3600),
            metrics_max_age: Duration::from_secs(365 * 24 * 3600),
        }
    }
}

impl InvocationCleanupWorkerConfig {
    /// Load configuration from environment variables.
    ///
    /// Environment variables:
    /// - `RUNTARA_INVOCATION_CLEANUP_ENABLED`: set to `false`/`0`/`no`/`off`
    ///   (case-insensitive) to disable. **Any other value — including unset,
    ///   typos, or `"yes"`/`"on"` — leaves cleanup enabled.** Cleanup is on
    ///   by default; only an explicit opt-out turns it off.
    /// - `RUNTARA_INVOCATION_CLEANUP_POLL_INTERVAL_SECS`: seconds between cycles (default: 3600)
    /// - `RUNTARA_INVOCATION_CLEANUP_MAX_AGE_DAYS`: days before executions are deleted (default: 3)
    /// - `RUNTARA_INVOCATION_CLEANUP_METRICS_MAX_AGE_DAYS`: days before metrics are deleted (default: 365)
    /// - `RUNTARA_INVOCATION_CLEANUP_BATCH_SIZE`: max executions per batch (default: 500)
    pub fn from_env() -> Self {
        let enabled = parse_enabled_env("RUNTARA_INVOCATION_CLEANUP_ENABLED");

        let poll_interval_secs = std::env::var("RUNTARA_INVOCATION_CLEANUP_POLL_INTERVAL_SECS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(3600);

        let metrics_max_age_days = std::env::var("RUNTARA_INVOCATION_CLEANUP_METRICS_MAX_AGE_DAYS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(365);

        Self {
            enabled,
            poll_interval: Duration::from_secs(poll_interval_secs),
            metrics_max_age: Duration::from_secs(metrics_max_age_days * 24 * 3600),
        }
    }
}

/// Background worker that prunes old invocation data from the server database.
pub struct InvocationCleanupWorker {
    pool: PgPool,
    oauth_repo: OAuthRepository,
    config: InvocationCleanupWorkerConfig,
    shutdown: ShutdownSignal,
}

impl InvocationCleanupWorker {
    pub fn new(
        pool: PgPool,
        config: InvocationCleanupWorkerConfig,
        shutdown: ShutdownSignal,
    ) -> Self {
        Self {
            oauth_repo: OAuthRepository::new(pool.clone()),
            pool,
            config,
            shutdown,
        }
    }

    /// Run the cleanup loop. Exits when the shutdown signal fires.
    pub async fn run(&self) {
        if !self.config.enabled {
            info!("Invocation cleanup worker disabled");
            return;
        }

        info!(
            poll_interval_secs = self.config.poll_interval.as_secs(),
            metrics_max_age_days = self.config.metrics_max_age.as_secs() / 86400,
            "Invocation cleanup worker started"
        );

        // Eager first pass: run a cleanup cycle immediately on startup so that
        // bounded retention is enforced even when the server restarts more
        // frequently than `poll_interval`. Race against the shutdown signal so
        // a slow cleanup (e.g. unreachable DB) cannot block shutdown.
        tokio::select! {
            biased;
            _ = self.shutdown.clone().wait() => {
                info!("Invocation cleanup worker exiting on shutdown signal");
                return;
            }
            res = self.cleanup_once() => {
                if let Err(e) = res {
                    error!(error = %e, "Invocation cleanup cycle failed");
                }
            }
        }

        loop {
            if self.shutdown.is_shutting_down() {
                info!("Invocation cleanup worker exiting on shutdown signal");
                return;
            }

            tokio::select! {
                biased;
                _ = self.shutdown.clone().wait() => {
                    info!("Invocation cleanup worker exiting on shutdown signal");
                    return;
                }
                _ = tokio::time::sleep(self.config.poll_interval) => {
                    if let Err(e) = self.cleanup_once().await {
                        error!(error = %e, "Invocation cleanup cycle failed");
                    }
                }
            }
        }
    }

    /// Run a single cleanup cycle: executions, then metrics, then expired
    /// OAuth CSRF state.
    pub async fn cleanup_once(&self) -> Result<(u64, u64), sqlx::Error> {
        let metrics_deleted = self.cleanup_old_metrics().await?;
        let oauth_states_deleted = self.oauth_repo.cleanup_expired().await?;

        if metrics_deleted > 0 || oauth_states_deleted > 0 {
            info!(
                metrics_deleted,
                oauth_states_deleted, "Invocation cleanup cycle completed"
            );
        } else {
            debug!("Invocation cleanup cycle completed, nothing to delete");
        }

        Ok((metrics_deleted, oauth_states_deleted))
    }

    /// Delete aggregated metrics older than `metrics_max_age`.
    async fn cleanup_old_metrics(&self) -> Result<u64, sqlx::Error> {
        let cutoff = Utc::now()
            - chrono::Duration::from_std(self.config.metrics_max_age).unwrap_or_else(|_| {
                warn!("Invalid metrics_max_age, falling back to 365 days");
                chrono::Duration::days(365)
            });

        let deleted = sqlx::query("DELETE FROM workflow_metrics_hourly WHERE hour_bucket < $1")
            .bind(cutoff)
            .execute(&self.pool)
            .await?
            .rows_affected();

        Ok(deleted)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_default() {
        let config = InvocationCleanupWorkerConfig::default();
        assert!(config.enabled);
        assert_eq!(config.poll_interval, Duration::from_secs(3600));
        assert_eq!(config.metrics_max_age, Duration::from_secs(365 * 24 * 3600));
    }

    #[test]
    fn test_config_from_env_defaults_when_unset() {
        // Ensure env vars aren't set in the test's address space
        // (best effort — these might bleed across threads but the test still exercises defaults)
        unsafe {
            std::env::remove_var("RUNTARA_INVOCATION_CLEANUP_ENABLED");
            std::env::remove_var("RUNTARA_INVOCATION_CLEANUP_POLL_INTERVAL_SECS");
            std::env::remove_var("RUNTARA_INVOCATION_CLEANUP_MAX_AGE_DAYS");
            std::env::remove_var("RUNTARA_INVOCATION_CLEANUP_METRICS_MAX_AGE_DAYS");
            std::env::remove_var("RUNTARA_INVOCATION_CLEANUP_BATCH_SIZE");
        }

        let config = InvocationCleanupWorkerConfig::from_env();
        assert!(config.enabled, "Enabled-by-default expected");
        assert_eq!(config.metrics_max_age.as_secs() / 86400, 365);
    }

    #[tokio::test]
    async fn test_run_exits_immediately_when_disabled() {
        let pool = PgPool::connect_lazy("postgres://localhost/dummy").unwrap();
        let config = InvocationCleanupWorkerConfig {
            enabled: false,
            ..Default::default()
        };
        let worker = InvocationCleanupWorker::new(pool, config, ShutdownSignal::new());

        // Must return promptly without touching the DB.
        tokio::time::timeout(Duration::from_secs(1), worker.run())
            .await
            .expect("run() should exit immediately when disabled");
    }
}
