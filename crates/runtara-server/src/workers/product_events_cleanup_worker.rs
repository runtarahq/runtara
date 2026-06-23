// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Background worker for product-analytics retention.
//!
//! Deletes `product_events` rows older than `max_age` in batches. Retention is
//! driven by `ingested_at` (row insertion time), not `occurred_at` — the
//! `product_events_ingested_idx` index exists for exactly this sweep. The split
//! lets async/batched writes preserve correct event time (`occurred_at`) while
//! keeping the retention cutoff deterministic on insertion time.

use std::time::Duration;

use chrono::Utc;
use runtara_core::config::parse_enabled_env;
use sqlx::PgPool;
use tracing::{debug, error, info, warn};

use crate::shutdown::ShutdownSignal;

/// Configuration for the product-events cleanup worker.
#[derive(Debug, Clone)]
pub struct ProductEventsCleanupWorkerConfig {
    /// Whether cleanup is enabled.
    pub enabled: bool,
    /// How often to run the cleanup cycle.
    pub poll_interval: Duration,
    /// Retention window — rows with `ingested_at` older than this are deleted.
    pub max_age: Duration,
    /// Max rows deleted per batch (bounds transaction size).
    pub batch_size: i64,
}

impl Default for ProductEventsCleanupWorkerConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            poll_interval: Duration::from_secs(3600),
            max_age: Duration::from_secs(45 * 24 * 3600),
            batch_size: 500,
        }
    }
}

impl ProductEventsCleanupWorkerConfig {
    /// Load configuration from environment variables.
    ///
    /// Environment variables:
    /// - `RUNTARA_PRODUCT_EVENTS_CLEANUP_ENABLED`: set to `false`/`0`/`no`/`off`
    ///   (case-insensitive) to disable. **Any other value — including unset,
    ///   typos, or `"yes"`/`"on"` — leaves cleanup enabled.** Cleanup is on by
    ///   default; only an explicit opt-out turns it off.
    /// - `RUNTARA_PRODUCT_EVENTS_CLEANUP_POLL_INTERVAL_SECS`: seconds between cycles (default: 3600)
    /// - `RUNTARA_PRODUCT_EVENTS_RETENTION_DAYS`: days before events are deleted (default: 45)
    /// - `RUNTARA_PRODUCT_EVENTS_CLEANUP_BATCH_SIZE`: max rows per batch (default: 500)
    pub fn from_env() -> Self {
        let enabled = parse_enabled_env("RUNTARA_PRODUCT_EVENTS_CLEANUP_ENABLED");

        let poll_interval_secs = std::env::var("RUNTARA_PRODUCT_EVENTS_CLEANUP_POLL_INTERVAL_SECS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(3600);

        let max_age_days = std::env::var("RUNTARA_PRODUCT_EVENTS_RETENTION_DAYS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(45);

        let batch_size = std::env::var("RUNTARA_PRODUCT_EVENTS_CLEANUP_BATCH_SIZE")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(500);

        Self {
            enabled,
            poll_interval: Duration::from_secs(poll_interval_secs),
            max_age: Duration::from_secs(max_age_days * 24 * 3600),
            batch_size,
        }
    }
}

/// Background worker that prunes old product-analytics events.
pub struct ProductEventsCleanupWorker {
    pool: PgPool,
    config: ProductEventsCleanupWorkerConfig,
    shutdown: ShutdownSignal,
}

impl ProductEventsCleanupWorker {
    pub fn new(
        pool: PgPool,
        config: ProductEventsCleanupWorkerConfig,
        shutdown: ShutdownSignal,
    ) -> Self {
        Self {
            pool,
            config,
            shutdown,
        }
    }

    /// Run the cleanup loop. Exits when the shutdown signal fires.
    pub async fn run(&self) {
        if !self.config.enabled {
            info!("Product events cleanup worker disabled");
            return;
        }

        info!(
            poll_interval_secs = self.config.poll_interval.as_secs(),
            max_age_days = self.config.max_age.as_secs() / 86400,
            batch_size = self.config.batch_size,
            "Product events cleanup worker started"
        );

        // Eager first pass: enforce bounded retention immediately on startup, even
        // when the server restarts more frequently than `poll_interval`. Race
        // against the shutdown signal so a slow cleanup (e.g. unreachable DB)
        // cannot block shutdown.
        tokio::select! {
            biased;
            _ = self.shutdown.clone().wait() => {
                info!("Product events cleanup worker exiting on shutdown signal");
                return;
            }
            res = self.cleanup_once() => {
                if let Err(e) = res {
                    error!(error = %e, "Product events cleanup cycle failed");
                }
            }
        }

        loop {
            if self.shutdown.is_shutting_down() {
                info!("Product events cleanup worker exiting on shutdown signal");
                return;
            }

            tokio::select! {
                biased;
                _ = self.shutdown.clone().wait() => {
                    info!("Product events cleanup worker exiting on shutdown signal");
                    return;
                }
                _ = tokio::time::sleep(self.config.poll_interval) => {
                    if let Err(e) = self.cleanup_once().await {
                        error!(error = %e, "Product events cleanup cycle failed");
                    }
                }
            }
        }
    }

    /// Delete `product_events` rows older than `max_age`, in batches bounded by
    /// `batch_size`. Retention is keyed on `ingested_at`.
    pub async fn cleanup_once(&self) -> Result<u64, sqlx::Error> {
        let cutoff = Utc::now()
            - chrono::Duration::from_std(self.config.max_age).unwrap_or_else(|_| {
                warn!("Invalid max_age, falling back to 45 days");
                chrono::Duration::days(45)
            });

        let mut total = 0u64;

        loop {
            if self.shutdown.is_shutting_down() {
                return Ok(total);
            }

            let deleted = sqlx::query(
                r#"
                WITH victims AS (
                    SELECT event_id
                    FROM product_events
                    WHERE ingested_at < $1
                    ORDER BY ingested_at ASC
                    LIMIT $2
                )
                DELETE FROM product_events
                WHERE event_id IN (SELECT event_id FROM victims)
                "#,
            )
            .bind(cutoff)
            .bind(self.config.batch_size)
            .execute(&self.pool)
            .await?
            .rows_affected();

            total += deleted;

            if (deleted as i64) < self.config.batch_size {
                break;
            }
        }

        if total > 0 {
            info!(
                events_deleted = total,
                "Product events cleanup cycle completed"
            );
        } else {
            debug!("Product events cleanup cycle completed, nothing to delete");
        }

        Ok(total)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_default() {
        let config = ProductEventsCleanupWorkerConfig::default();
        assert!(config.enabled);
        assert_eq!(config.poll_interval, Duration::from_secs(3600));
        assert_eq!(config.max_age, Duration::from_secs(45 * 24 * 3600));
        assert_eq!(config.batch_size, 500);
    }

    #[test]
    fn test_config_from_env_defaults_when_unset() {
        // Ensure the vars aren't set in this test's address space (best effort).
        unsafe {
            std::env::remove_var("RUNTARA_PRODUCT_EVENTS_CLEANUP_ENABLED");
            std::env::remove_var("RUNTARA_PRODUCT_EVENTS_CLEANUP_POLL_INTERVAL_SECS");
            std::env::remove_var("RUNTARA_PRODUCT_EVENTS_RETENTION_DAYS");
            std::env::remove_var("RUNTARA_PRODUCT_EVENTS_CLEANUP_BATCH_SIZE");
        }

        let config = ProductEventsCleanupWorkerConfig::from_env();
        assert!(config.enabled, "Enabled-by-default expected");
        assert_eq!(config.max_age.as_secs() / 86400, 45);
        assert_eq!(config.batch_size, 500);
    }

    #[tokio::test]
    async fn test_run_exits_immediately_when_disabled() {
        let pool = PgPool::connect_lazy("postgres://localhost/dummy").unwrap();
        let config = ProductEventsCleanupWorkerConfig {
            enabled: false,
            ..Default::default()
        };
        let worker = ProductEventsCleanupWorker::new(pool, config, ShutdownSignal::new());

        // Must return promptly without touching the DB.
        tokio::time::timeout(Duration::from_secs(1), worker.run())
            .await
            .expect("run() should exit immediately when disabled");
    }
}
