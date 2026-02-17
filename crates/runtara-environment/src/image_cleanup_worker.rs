// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Background worker for cleaning up unused images.
//!
//! Images accumulate over time because:
//! - Each scenario recompilation registers a new image (new `image_id`)
//! - The image registry upserts on `(tenant_id, name)`, replacing the old `image_id`
//! - Neither the registry nor the caller deletes the old image files from disk
//!
//! This worker handles two types of cleanup:
//!
//! 1. **Orphaned disk directories**: image directories on disk whose `image_id`
//!    no longer exists in the database (created by upsert overwrites). These are
//!    always safe to delete immediately.
//!
//! 2. **Stale DB images**: images older than `max_age` with no active (non-terminal)
//!    instances referencing them. Both the database record and disk files are removed.
//!    The `ON DELETE CASCADE` on `instance_images.image_id` handles join table cleanup.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use sqlx::PgPool;
use tokio::sync::Notify;
use tracing::{debug, error, info, warn};

use crate::error::Result;

/// Configuration for the image cleanup worker.
#[derive(Debug, Clone)]
pub struct ImageCleanupWorkerConfig {
    /// Whether image cleanup is enabled.
    pub enabled: bool,
    /// How often to run cleanup.
    pub poll_interval: Duration,
    /// Maximum age for unused images before cleanup.
    pub max_age: Duration,
    /// Maximum images to delete per cycle (prevents long I/O storms).
    pub batch_size: i64,
    /// Data directory containing the `images/` subdirectory.
    pub data_dir: PathBuf,
}

impl Default for ImageCleanupWorkerConfig {
    fn default() -> Self {
        Self {
            enabled: false,                               // Disabled by default for safety
            poll_interval: Duration::from_secs(6 * 3600), // 6 hours
            max_age: Duration::from_secs(7 * 24 * 3600),  // 7 days
            batch_size: 50,
            data_dir: PathBuf::new(), // Set by runtime
        }
    }
}

impl ImageCleanupWorkerConfig {
    /// Load configuration from environment variables.
    ///
    /// Environment variables:
    /// - `RUNTARA_IMAGE_CLEANUP_ENABLED`: "true" or "1" to enable (default: false)
    /// - `RUNTARA_IMAGE_CLEANUP_POLL_INTERVAL_SECS`: seconds between cleanup runs (default: 21600)
    /// - `RUNTARA_IMAGE_CLEANUP_MAX_AGE_DAYS`: days before stale images are deleted (default: 7)
    /// - `RUNTARA_IMAGE_CLEANUP_BATCH_SIZE`: max images per cycle (default: 50)
    pub fn from_env() -> Self {
        let enabled = std::env::var("RUNTARA_IMAGE_CLEANUP_ENABLED")
            .map(|v| v == "true" || v == "1")
            .unwrap_or(false);

        let poll_interval_secs = std::env::var("RUNTARA_IMAGE_CLEANUP_POLL_INTERVAL_SECS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(6 * 3600);

        let max_age_days = std::env::var("RUNTARA_IMAGE_CLEANUP_MAX_AGE_DAYS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(7);

        let batch_size = std::env::var("RUNTARA_IMAGE_CLEANUP_BATCH_SIZE")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(50);

        Self {
            enabled,
            poll_interval: Duration::from_secs(poll_interval_secs),
            max_age: Duration::from_secs(max_age_days * 24 * 3600),
            batch_size,
            data_dir: PathBuf::new(), // Set by runtime
        }
    }
}

/// Background worker that cleans up unused images.
pub struct ImageCleanupWorker {
    pool: PgPool,
    config: ImageCleanupWorkerConfig,
    shutdown: Arc<Notify>,
}

impl ImageCleanupWorker {
    /// Create a new image cleanup worker.
    pub fn new(pool: PgPool, config: ImageCleanupWorkerConfig) -> Self {
        Self {
            pool,
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
    /// This will periodically scan for and remove orphaned and stale images.
    /// The loop exits when the shutdown signal is received.
    pub async fn run(&self) {
        if !self.config.enabled {
            info!("Image cleanup worker disabled");
            return;
        }

        info!(
            poll_interval_secs = self.config.poll_interval.as_secs(),
            max_age_days = self.config.max_age.as_secs() / 86400,
            batch_size = self.config.batch_size,
            "Image cleanup worker started"
        );

        loop {
            tokio::select! {
                biased;

                _ = self.shutdown.notified() => {
                    info!("Image cleanup worker received shutdown signal");
                    break;
                }

                _ = tokio::time::sleep(self.config.poll_interval) => {
                    if let Err(e) = self.cleanup_images().await {
                        error!(error = %e, "Failed to cleanup images");
                    }
                }
            }
        }

        info!("Image cleanup worker stopped");
    }

    /// Run both cleanup phases.
    async fn cleanup_images(&self) -> Result<()> {
        let orphaned_cleaned = self.cleanup_orphaned_directories().await;
        let stale_cleaned = self.cleanup_stale_images().await?;

        if orphaned_cleaned > 0 || stale_cleaned > 0 {
            info!(
                orphaned_cleaned = orphaned_cleaned,
                stale_cleaned = stale_cleaned,
                "Image cleanup cycle completed"
            );
        } else {
            debug!("Image cleanup cycle completed, nothing to clean");
        }

        Ok(())
    }

    /// Phase 1: Clean up image directories on disk that have no corresponding DB record.
    ///
    /// These orphans are created when `ImageRegistry::register()` upserts on
    /// `(tenant_id, name)` with a new `image_id` — the old directory is left behind.
    async fn cleanup_orphaned_directories(&self) -> u64 {
        let images_dir = self.config.data_dir.join("images");

        // Read all subdirectory names from disk
        let mut dir_entries = match tokio::fs::read_dir(&images_dir).await {
            Ok(d) => d,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                debug!("Images directory does not exist, nothing to clean");
                return 0;
            }
            Err(e) => {
                warn!(error = %e, "Failed to read images directory");
                return 0;
            }
        };

        let mut disk_ids = Vec::new();
        while let Ok(Some(entry)) = dir_entries.next_entry().await {
            if let Ok(ft) = entry.file_type().await
                && ft.is_dir()
            {
                disk_ids.push(entry.file_name().to_string_lossy().to_string());
            }
        }

        if disk_ids.is_empty() {
            return 0;
        }

        // Batch-check which IDs exist in the database
        let db_ids: HashSet<String> = match sqlx::query_scalar::<_, String>(
            "SELECT image_id FROM images WHERE image_id = ANY($1)",
        )
        .bind(&disk_ids)
        .fetch_all(&self.pool)
        .await
        {
            Ok(ids) => ids.into_iter().collect(),
            Err(e) => {
                warn!(error = %e, "Failed to query images table for orphan detection");
                return 0;
            }
        };

        // Delete directories not found in DB
        let mut cleaned = 0u64;
        for id in &disk_ids {
            if db_ids.contains(id) {
                continue;
            }

            let dir = images_dir.join(id);
            match tokio::fs::remove_dir_all(&dir).await {
                Ok(()) => {
                    info!(image_id = %id, "Removed orphaned image directory");
                    cleaned += 1;
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    // Already gone, that's fine
                }
                Err(e) => {
                    warn!(image_id = %id, error = %e, "Failed to remove orphaned image directory");
                }
            }
        }

        cleaned
    }

    /// Phase 2: Clean up images in the DB that are stale (old + no active or recent instances).
    async fn cleanup_stale_images(&self) -> Result<u64> {
        let cutoff = Utc::now()
            - chrono::Duration::from_std(self.config.max_age)
                .map_err(|e| crate::error::Error::Other(format!("Invalid duration: {}", e)))?;

        let stale_images: Vec<(String, String, String)> = sqlx::query_as(
            r#"
            SELECT i.image_id, i.tenant_id, i.name
            FROM images i
            WHERE i.updated_at < $1
              AND NOT EXISTS (
                SELECT 1
                FROM instance_images ii
                JOIN instances inst ON ii.instance_id = inst.instance_id
                WHERE ii.image_id = i.image_id
                  AND (
                    inst.status NOT IN ('completed', 'failed', 'cancelled')
                    OR ii.created_at > $1
                  )
              )
            ORDER BY i.updated_at ASC
            LIMIT $2
            "#,
        )
        .bind(cutoff)
        .bind(self.config.batch_size)
        .fetch_all(&self.pool)
        .await?;

        if stale_images.is_empty() {
            return Ok(0);
        }

        let mut cleaned = 0u64;
        for (image_id, tenant_id, name) in &stale_images {
            // Delete from DB (CASCADE handles instance_images)
            if let Err(e) = sqlx::query("DELETE FROM images WHERE image_id = $1")
                .bind(image_id)
                .execute(&self.pool)
                .await
            {
                warn!(
                    image_id = %image_id,
                    error = %e,
                    "Failed to delete stale image from database"
                );
                continue;
            }

            // Delete disk directory
            let image_dir = self.config.data_dir.join("images").join(image_id);
            if let Err(e) = tokio::fs::remove_dir_all(&image_dir).await
                && e.kind() != std::io::ErrorKind::NotFound
            {
                warn!(
                    image_id = %image_id,
                    error = %e,
                    "Failed to remove stale image directory"
                );
            }

            info!(
                image_id = %image_id,
                tenant_id = %tenant_id,
                name = %name,
                "Deleted stale image"
            );
            cleaned += 1;
        }

        Ok(cleaned)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_config_default() {
        let config = ImageCleanupWorkerConfig::default();
        assert!(!config.enabled);
        assert_eq!(config.poll_interval, Duration::from_secs(6 * 3600));
        assert_eq!(config.max_age, Duration::from_secs(7 * 24 * 3600));
        assert_eq!(config.batch_size, 50);
    }

    #[test]
    fn test_config_disabled_by_default() {
        let config = ImageCleanupWorkerConfig::default();
        assert!(
            !config.enabled,
            "Image cleanup should be disabled by default for safety"
        );
    }

    #[test]
    fn test_config_max_age_days() {
        let config = ImageCleanupWorkerConfig {
            max_age: Duration::from_secs(3 * 24 * 3600), // 3 days
            ..Default::default()
        };
        assert_eq!(config.max_age.as_secs() / 86400, 3);
    }

    #[test]
    fn test_config_custom_values() {
        let config = ImageCleanupWorkerConfig {
            enabled: true,
            poll_interval: Duration::from_secs(600),
            max_age: Duration::from_secs(24 * 3600),
            batch_size: 10,
            data_dir: PathBuf::from("/tmp/test"),
        };
        assert!(config.enabled);
        assert_eq!(config.poll_interval.as_secs(), 600);
        assert_eq!(config.max_age.as_secs() / 86400, 1);
        assert_eq!(config.batch_size, 10);
    }

    #[test]
    fn test_shutdown_handle() {
        let pool = PgPool::connect_lazy("postgres://localhost/dummy").unwrap();
        let config = ImageCleanupWorkerConfig::default();
        let worker = ImageCleanupWorker::new(pool, config);
        let handle = worker.shutdown_handle();
        // Both the worker and the returned handle hold a reference
        assert!(Arc::strong_count(&handle) >= 2);
    }

    #[tokio::test]
    async fn test_run_exits_immediately_when_disabled() {
        let pool = PgPool::connect_lazy("postgres://localhost/dummy").unwrap();
        let config = ImageCleanupWorkerConfig {
            enabled: false,
            ..Default::default()
        };
        let worker = ImageCleanupWorker::new(pool, config);

        // Should return immediately without blocking
        tokio::time::timeout(Duration::from_secs(1), worker.run())
            .await
            .expect("run() should exit immediately when disabled");
    }

    #[tokio::test]
    async fn test_cleanup_orphaned_nonexistent_images_dir() {
        let pool = PgPool::connect_lazy("postgres://localhost/dummy").unwrap();
        let config = ImageCleanupWorkerConfig {
            data_dir: PathBuf::from("/nonexistent/path/that/does/not/exist"),
            ..Default::default()
        };
        let worker = ImageCleanupWorker::new(pool, config);

        // Should return 0 without error when images dir doesn't exist
        let cleaned = worker.cleanup_orphaned_directories().await;
        assert_eq!(cleaned, 0);
    }

    #[tokio::test]
    async fn test_cleanup_orphaned_empty_images_dir() {
        let temp_dir = TempDir::new().unwrap();
        tokio::fs::create_dir_all(temp_dir.path().join("images"))
            .await
            .unwrap();

        let pool = PgPool::connect_lazy("postgres://localhost/dummy").unwrap();
        let config = ImageCleanupWorkerConfig {
            data_dir: temp_dir.path().to_path_buf(),
            ..Default::default()
        };
        let worker = ImageCleanupWorker::new(pool, config);

        // Should return 0 when images dir is empty
        let cleaned = worker.cleanup_orphaned_directories().await;
        assert_eq!(cleaned, 0);
    }

    #[tokio::test]
    async fn test_cleanup_orphaned_skips_files() {
        let temp_dir = TempDir::new().unwrap();
        let images_dir = temp_dir.path().join("images");
        tokio::fs::create_dir_all(&images_dir).await.unwrap();

        // Create a file (not a directory) — should be skipped
        tokio::fs::write(images_dir.join("not-a-directory.txt"), "hello")
            .await
            .unwrap();

        let pool = PgPool::connect_lazy("postgres://localhost/dummy").unwrap();
        let config = ImageCleanupWorkerConfig {
            data_dir: temp_dir.path().to_path_buf(),
            ..Default::default()
        };
        let worker = ImageCleanupWorker::new(pool, config);

        // Should return 0 because files are not processed, only directories
        let cleaned = worker.cleanup_orphaned_directories().await;
        assert_eq!(cleaned, 0);

        // File should still exist
        assert!(images_dir.join("not-a-directory.txt").exists());
    }

    #[tokio::test]
    async fn test_run_responds_to_shutdown() {
        let pool = PgPool::connect_lazy("postgres://localhost/dummy").unwrap();
        let config = ImageCleanupWorkerConfig {
            enabled: true,
            poll_interval: Duration::from_secs(3600), // Long interval
            ..Default::default()
        };
        let worker = ImageCleanupWorker::new(pool, config);
        let shutdown = worker.shutdown_handle();

        let handle = tokio::spawn(async move {
            worker.run().await;
        });

        // Signal shutdown
        shutdown.notify_one();

        // Should exit promptly
        tokio::time::timeout(Duration::from_secs(2), handle)
            .await
            .expect("worker should shut down within 2 seconds")
            .expect("worker task should not panic");
    }
}
