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
//! - `container_status`
//! - `container_cancellations`
//! - `container_heartbeats`
//! - `instance_images`

use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
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
}

impl Default for DbCleanupWorkerConfig {
    fn default() -> Self {
        Self {
            enabled: false,                               // Disabled by default for safety
            poll_interval: Duration::from_secs(3600),     // 1 hour
            max_age: Duration::from_secs(30 * 24 * 3600), // 30 days
            batch_size: 100,
        }
    }
}

impl DbCleanupWorkerConfig {
    /// Load configuration from environment variables.
    ///
    /// Environment variables:
    /// - `RUNTARA_DB_CLEANUP_ENABLED`: "true" or "1" to enable (default: false)
    /// - `RUNTARA_DB_CLEANUP_POLL_INTERVAL_SECS`: seconds between cleanup runs (default: 3600)
    /// - `RUNTARA_DB_CLEANUP_MAX_AGE_DAYS`: days before terminal instances are deleted (default: 30)
    /// - `RUNTARA_DB_CLEANUP_BATCH_SIZE`: max instances per batch (default: 100)
    pub fn from_env() -> Self {
        let enabled = std::env::var("RUNTARA_DB_CLEANUP_ENABLED")
            .map(|v| v == "true" || v == "1")
            .unwrap_or(false);

        let poll_interval_secs = std::env::var("RUNTARA_DB_CLEANUP_POLL_INTERVAL_SECS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(3600);

        let max_age_days = std::env::var("RUNTARA_DB_CLEANUP_MAX_AGE_DAYS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(30);

        let batch_size = std::env::var("RUNTARA_DB_CLEANUP_BATCH_SIZE")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(100);

        Self {
            enabled,
            poll_interval: Duration::from_secs(poll_interval_secs),
            max_age: Duration::from_secs(max_age_days * 24 * 3600),
            batch_size,
        }
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

        loop {
            tokio::select! {
                biased;

                _ = self.shutdown.notified() => {
                    info!("Database cleanup worker received shutdown signal");
                    break;
                }

                _ = tokio::time::sleep(self.config.poll_interval) => {
                    if let Err(e) = self.cleanup_old_instances().await {
                        error!(error = %e, "Failed to cleanup old instances");
                    }
                }
            }
        }

        info!("Database cleanup worker stopped");
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

        // container_status
        sqlx::query("DELETE FROM container_status WHERE instance_id = ANY($1)")
            .bind(instance_ids)
            .execute(&mut *tx)
            .await?;

        // container_cancellations
        sqlx::query("DELETE FROM container_cancellations WHERE instance_id = ANY($1)")
            .bind(instance_ids)
            .execute(&mut *tx)
            .await?;

        // container_heartbeats
        sqlx::query("DELETE FROM container_heartbeats WHERE instance_id = ANY($1)")
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
        assert!(!config.enabled);
        assert_eq!(config.poll_interval, Duration::from_secs(3600));
        assert_eq!(config.max_age, Duration::from_secs(30 * 24 * 3600));
        assert_eq!(config.batch_size, 100);
    }

    #[test]
    fn test_config_max_age_days() {
        let config = DbCleanupWorkerConfig {
            max_age: Duration::from_secs(7 * 24 * 3600), // 7 days
            ..Default::default()
        };
        assert_eq!(config.max_age.as_secs() / 86400, 7);
    }

    #[test]
    fn test_config_disabled_by_default() {
        let config = DbCleanupWorkerConfig::default();
        assert!(
            !config.enabled,
            "Cleanup should be disabled by default for safety"
        );
    }
}
