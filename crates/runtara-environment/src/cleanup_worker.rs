// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Background worker for cleaning up old run directories.
//!
//! Run directories (`{DATA_DIR}/{tenant_id}/runs/{instance_id}/`) contain:
//! - `input.json` - Input data for the instance
//! - `output.json` - Output written by the workflow
//! - `stderr.log` - Captured stderr from the container
//! - `config.json` - Per-instance OCI configuration
//!
//! These directories are not cleaned up immediately after execution to allow
//! for debugging and to avoid race conditions with output processing.
//! Instead, this worker periodically scans for old directories and removes them.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use tokio::sync::Notify;
use tracing::{debug, error, info, warn};

/// Configuration for the cleanup worker.
#[derive(Debug, Clone)]
pub struct CleanupWorkerConfig {
    /// Data directory containing tenant run directories.
    pub data_dir: PathBuf,
    /// How often to scan for old directories.
    pub poll_interval: Duration,
    /// Maximum age of run directories before cleanup.
    pub max_age: Duration,
}

impl Default for CleanupWorkerConfig {
    fn default() -> Self {
        Self {
            data_dir: PathBuf::from(".data"),
            poll_interval: Duration::from_secs(3600), // 1 hour
            max_age: Duration::from_secs(24 * 3600),  // 24 hours
        }
    }
}

/// Background worker that cleans up old run directories.
pub struct CleanupWorker {
    config: CleanupWorkerConfig,
    shutdown: Arc<Notify>,
}

impl CleanupWorker {
    /// Create a new cleanup worker.
    pub fn new(config: CleanupWorkerConfig) -> Self {
        Self {
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
    /// This will periodically scan for and remove old run directories.
    /// The loop exits when the shutdown signal is received.
    pub async fn run(&self) {
        info!(
            data_dir = %self.config.data_dir.display(),
            poll_interval_secs = self.config.poll_interval.as_secs(),
            max_age_hours = self.config.max_age.as_secs() / 3600,
            "Cleanup worker started"
        );

        loop {
            tokio::select! {
                biased;

                _ = self.shutdown.notified() => {
                    info!("Cleanup worker received shutdown signal");
                    break;
                }

                _ = tokio::time::sleep(self.config.poll_interval) => {
                    if let Err(e) = self.cleanup_old_directories().await {
                        error!(error = %e, "Failed to cleanup old directories");
                    }
                }
            }
        }

        info!("Cleanup worker stopped");
    }

    /// Scan for and remove old run directories.
    async fn cleanup_old_directories(&self) -> std::io::Result<()> {
        let cutoff = Utc::now() - chrono::Duration::from_std(self.config.max_age).unwrap();
        let mut cleaned = 0u64;
        let mut errors = 0u64;

        // Scan all tenant directories
        let mut tenant_dirs = match tokio::fs::read_dir(&self.config.data_dir).await {
            Ok(d) => d,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                debug!("Data directory does not exist, nothing to clean");
                return Ok(());
            }
            Err(e) => return Err(e),
        };

        while let Some(tenant_entry) = tenant_dirs.next_entry().await? {
            let tenant_path = tenant_entry.path();

            // Skip non-directories and special directories
            if !tenant_path.is_dir() {
                continue;
            }

            let tenant_name = tenant_entry.file_name();
            let tenant_name_str = tenant_name.to_string_lossy();

            // Skip known non-tenant directories
            if matches!(
                tenant_name_str.as_ref(),
                "bundles" | "images" | "logs" | "library_cache" | "pids"
            ) {
                continue;
            }

            // Skip files (like .db files)
            if tenant_name_str.ends_with(".db") {
                continue;
            }

            // Check for runs subdirectory
            let runs_dir = tenant_path.join("runs");
            if !runs_dir.exists() {
                continue;
            }

            // Scan run directories for this tenant
            let (tenant_cleaned, tenant_errors) = self.cleanup_tenant_runs(&runs_dir, cutoff).await;
            cleaned += tenant_cleaned;
            errors += tenant_errors;
        }

        if cleaned > 0 || errors > 0 {
            info!(
                cleaned = cleaned,
                errors = errors,
                "Cleanup cycle completed"
            );
        } else {
            debug!("Cleanup cycle completed, no old directories found");
        }

        Ok(())
    }

    /// Clean up old run directories for a single tenant.
    async fn cleanup_tenant_runs(
        &self,
        runs_dir: &std::path::Path,
        cutoff: DateTime<Utc>,
    ) -> (u64, u64) {
        let mut cleaned = 0u64;
        let mut errors = 0u64;

        let mut run_dirs = match tokio::fs::read_dir(runs_dir).await {
            Ok(d) => d,
            Err(e) => {
                warn!(
                    path = %runs_dir.display(),
                    error = %e,
                    "Failed to read runs directory"
                );
                return (0, 1);
            }
        };

        while let Ok(Some(run_entry)) = run_dirs.next_entry().await {
            let run_path = run_entry.path();

            if !run_path.is_dir() {
                continue;
            }

            // Check modification time of the directory
            let metadata = match tokio::fs::metadata(&run_path).await {
                Ok(m) => m,
                Err(e) => {
                    debug!(
                        path = %run_path.display(),
                        error = %e,
                        "Failed to get metadata for run directory"
                    );
                    errors += 1;
                    continue;
                }
            };

            let modified = match metadata.modified() {
                Ok(t) => DateTime::<Utc>::from(t),
                Err(e) => {
                    debug!(
                        path = %run_path.display(),
                        error = %e,
                        "Failed to get modification time"
                    );
                    errors += 1;
                    continue;
                }
            };

            // Skip if too recent
            if modified > cutoff {
                continue;
            }

            // Remove the old directory
            match tokio::fs::remove_dir_all(&run_path).await {
                Ok(()) => {
                    debug!(
                        path = %run_path.display(),
                        age_hours = (Utc::now() - modified).num_hours(),
                        "Removed old run directory"
                    );
                    cleaned += 1;
                }
                Err(e) => {
                    warn!(
                        path = %run_path.display(),
                        error = %e,
                        "Failed to remove old run directory"
                    );
                    errors += 1;
                }
            }
        }

        (cleaned, errors)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_config_default() {
        let config = CleanupWorkerConfig::default();
        assert_eq!(config.poll_interval, Duration::from_secs(3600));
        assert_eq!(config.max_age, Duration::from_secs(24 * 3600));
    }

    #[test]
    fn test_worker_new() {
        let config = CleanupWorkerConfig::default();
        let worker = CleanupWorker::new(config);
        assert!(Arc::strong_count(&worker.shutdown) >= 1);
    }

    #[test]
    fn test_shutdown_handle() {
        let config = CleanupWorkerConfig::default();
        let worker = CleanupWorker::new(config);
        let handle = worker.shutdown_handle();
        assert!(Arc::strong_count(&handle) >= 2);
    }

    #[tokio::test]
    async fn test_cleanup_empty_data_dir() {
        let temp_dir = TempDir::new().unwrap();
        let config = CleanupWorkerConfig {
            data_dir: temp_dir.path().to_path_buf(),
            poll_interval: Duration::from_secs(1),
            max_age: Duration::from_secs(1),
        };
        let worker = CleanupWorker::new(config);

        // Should not error on empty directory
        let result = worker.cleanup_old_directories().await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_cleanup_nonexistent_data_dir() {
        let config = CleanupWorkerConfig {
            data_dir: PathBuf::from("/nonexistent/path/that/does/not/exist"),
            poll_interval: Duration::from_secs(1),
            max_age: Duration::from_secs(1),
        };
        let worker = CleanupWorker::new(config);

        // Should not error on nonexistent directory
        let result = worker.cleanup_old_directories().await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_cleanup_skips_special_directories() {
        let temp_dir = TempDir::new().unwrap();

        // Create special directories that should be skipped
        tokio::fs::create_dir_all(temp_dir.path().join("bundles"))
            .await
            .unwrap();
        tokio::fs::create_dir_all(temp_dir.path().join("images"))
            .await
            .unwrap();
        tokio::fs::create_dir_all(temp_dir.path().join("logs"))
            .await
            .unwrap();

        let config = CleanupWorkerConfig {
            data_dir: temp_dir.path().to_path_buf(),
            poll_interval: Duration::from_secs(1),
            max_age: Duration::from_secs(0), // Immediate cleanup
        };
        let worker = CleanupWorker::new(config);

        worker.cleanup_old_directories().await.unwrap();

        // Special directories should still exist
        assert!(temp_dir.path().join("bundles").exists());
        assert!(temp_dir.path().join("images").exists());
        assert!(temp_dir.path().join("logs").exists());
    }

    #[tokio::test]
    async fn test_cleanup_removes_old_run_directories() {
        let temp_dir = TempDir::new().unwrap();

        // Create a tenant with old run directory
        let runs_dir = temp_dir.path().join("test-tenant").join("runs");
        let old_run = runs_dir.join("old-instance");
        tokio::fs::create_dir_all(&old_run).await.unwrap();

        // Create some files in the run directory
        tokio::fs::write(old_run.join("output.json"), "{}")
            .await
            .unwrap();

        let config = CleanupWorkerConfig {
            data_dir: temp_dir.path().to_path_buf(),
            poll_interval: Duration::from_secs(1),
            max_age: Duration::from_secs(0), // Immediate cleanup
        };
        let worker = CleanupWorker::new(config);

        worker.cleanup_old_directories().await.unwrap();

        // Old run directory should be removed
        assert!(!old_run.exists());
        // But runs directory should still exist
        assert!(runs_dir.exists());
    }
}
