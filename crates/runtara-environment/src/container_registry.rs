// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Container Registry
//!
//! PostgreSQL-based registry for tracking running containers/instances.
//! Enables fire-and-forget launching, runtime restart recovery, and distributed cancellation.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use std::time::Duration;

use crate::error::Result;

/// Container registry entry stored in PostgreSQL
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct ContainerInfo {
    /// Container/handle ID used with the runner
    pub container_id: String,
    /// Execution instance ID (UUID)
    pub instance_id: String,
    /// Tenant ID
    pub tenant_id: String,
    /// Path to the executable binary
    pub binary_path: String,
    /// Path to the OCI bundle (if containerized)
    pub bundle_path: Option<String>,
    /// When the container was started
    pub started_at: DateTime<Utc>,
    /// Process ID (if known)
    pub pid: Option<i32>,
    /// Execution timeout in seconds
    pub timeout_seconds: Option<i64>,
}

/// Cancellation request stored in PostgreSQL
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct CancellationRequest {
    /// Instance ID
    pub instance_id: String,
    /// When cancellation was requested
    pub requested_at: DateTime<Utc>,
    /// Grace period before force kill (seconds)
    pub grace_period_seconds: i32,
    /// Reason for cancellation
    pub reason: String,
}

/// Container status reported by the container itself
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ContainerStatus {
    /// Container is running.
    Running {
        /// When the status was updated.
        updated_at: DateTime<Utc>,
    },
    /// Container completed successfully.
    Completed {
        /// When the status was updated.
        updated_at: DateTime<Utc>,
        /// Output data from the container.
        #[serde(default)]
        output: Option<serde_json::Value>,
    },
    /// Container failed with error.
    Failed {
        /// When the status was updated.
        updated_at: DateTime<Utc>,
        /// Error message.
        error: String,
    },
    /// Container was cancelled.
    Cancelled {
        /// When the status was updated.
        updated_at: DateTime<Utc>,
    },
}

impl ContainerStatus {
    /// Check if this is a terminal status
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            ContainerStatus::Completed { .. }
                | ContainerStatus::Failed { .. }
                | ContainerStatus::Cancelled { .. }
        )
    }

    /// Get the status string
    pub fn status_str(&self) -> &'static str {
        match self {
            ContainerStatus::Running { .. } => "running",
            ContainerStatus::Completed { .. } => "completed",
            ContainerStatus::Failed { .. } => "failed",
            ContainerStatus::Cancelled { .. } => "cancelled",
        }
    }

    /// Get the updated_at timestamp
    pub fn updated_at(&self) -> DateTime<Utc> {
        match self {
            ContainerStatus::Running { updated_at } => *updated_at,
            ContainerStatus::Completed { updated_at, .. } => *updated_at,
            ContainerStatus::Failed { updated_at, .. } => *updated_at,
            ContainerStatus::Cancelled { updated_at } => *updated_at,
        }
    }
}

/// Container registry client for PostgreSQL operations
pub struct ContainerRegistry {
    pool: PgPool,
}

impl ContainerRegistry {
    /// Create a new registry client
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Register a container as running
    ///
    /// Should be called BEFORE spawning the container process.
    pub async fn register(&self, info: &ContainerInfo) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO container_registry (
                container_id, instance_id, tenant_id, binary_path, bundle_path,
                started_at, pid, timeout_seconds
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
            ON CONFLICT (instance_id) DO UPDATE SET
                container_id = EXCLUDED.container_id,
                binary_path = EXCLUDED.binary_path,
                bundle_path = EXCLUDED.bundle_path,
                started_at = EXCLUDED.started_at,
                pid = EXCLUDED.pid,
                timeout_seconds = EXCLUDED.timeout_seconds
            "#,
        )
        .bind(&info.container_id)
        .bind(&info.instance_id)
        .bind(&info.tenant_id)
        .bind(&info.binary_path)
        .bind(&info.bundle_path)
        .bind(info.started_at)
        .bind(info.pid)
        .bind(info.timeout_seconds)
        .execute(&self.pool)
        .await?;

        tracing::info!(
            container_id = %info.container_id,
            instance_id = %info.instance_id,
            tenant_id = %info.tenant_id,
            "Registered container in registry"
        );

        Ok(())
    }

    /// Unregister a container (on completion or cleanup)
    pub async fn unregister(&self, instance_id: &str) -> Result<()> {
        sqlx::query("DELETE FROM container_registry WHERE instance_id = $1")
            .bind(instance_id)
            .execute(&self.pool)
            .await?;

        tracing::debug!(
            instance_id = %instance_id,
            "Unregistered container from registry"
        );

        Ok(())
    }

    /// List all registered containers for a tenant
    pub async fn list_registered(&self, tenant_id: &str) -> Result<Vec<ContainerInfo>> {
        let containers = sqlx::query_as::<_, ContainerInfo>(
            "SELECT * FROM container_registry WHERE tenant_id = $1",
        )
        .bind(tenant_id)
        .fetch_all(&self.pool)
        .await?;

        Ok(containers)
    }

    /// List all registered containers (all tenants)
    pub async fn list_all_registered(&self) -> Result<Vec<ContainerInfo>> {
        let containers = sqlx::query_as::<_, ContainerInfo>("SELECT * FROM container_registry")
            .fetch_all(&self.pool)
            .await?;

        Ok(containers)
    }

    /// Get a specific container's info
    pub async fn get(&self, instance_id: &str) -> Result<Option<ContainerInfo>> {
        let container = sqlx::query_as::<_, ContainerInfo>(
            "SELECT * FROM container_registry WHERE instance_id = $1",
        )
        .bind(instance_id)
        .fetch_optional(&self.pool)
        .await?;

        Ok(container)
    }

    /// Update container PID after spawn
    pub async fn update_pid(&self, instance_id: &str, pid: i32) -> Result<()> {
        sqlx::query("UPDATE container_registry SET pid = $1 WHERE instance_id = $2")
            .bind(pid)
            .bind(instance_id)
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    // ===== Cancellation =====

    /// Request cancellation of a container
    pub async fn request_cancellation(
        &self,
        instance_id: &str,
        grace_period: Duration,
        reason: &str,
    ) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO container_cancellations (instance_id, requested_at, grace_period_seconds, reason)
            VALUES ($1, $2, $3, $4)
            ON CONFLICT (instance_id) DO UPDATE SET
                requested_at = EXCLUDED.requested_at,
                grace_period_seconds = EXCLUDED.grace_period_seconds,
                reason = EXCLUDED.reason
            "#,
        )
        .bind(instance_id)
        .bind(Utc::now())
        .bind(grace_period.as_secs() as i32)
        .bind(reason)
        .execute(&self.pool)
        .await?;

        tracing::info!(
            instance_id = %instance_id,
            grace_period_secs = grace_period.as_secs(),
            reason = %reason,
            "Wrote cancellation token"
        );

        Ok(())
    }

    /// Check if cancellation has been requested for a container
    pub async fn check_cancellation(
        &self,
        instance_id: &str,
    ) -> Result<Option<CancellationRequest>> {
        let request = sqlx::query_as::<_, CancellationRequest>(
            "SELECT * FROM container_cancellations WHERE instance_id = $1",
        )
        .bind(instance_id)
        .fetch_optional(&self.pool)
        .await?;

        Ok(request)
    }

    /// Remove cancellation token (after handling)
    pub async fn clear_cancellation(&self, instance_id: &str) -> Result<()> {
        sqlx::query("DELETE FROM container_cancellations WHERE instance_id = $1")
            .bind(instance_id)
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    // ===== Status Reporting =====

    /// Report container status
    pub async fn report_status(&self, instance_id: &str, status: &ContainerStatus) -> Result<()> {
        let status_json = serde_json::to_value(status)?;

        sqlx::query(
            r#"
            INSERT INTO container_status (instance_id, status, updated_at)
            VALUES ($1, $2, $3)
            ON CONFLICT (instance_id) DO UPDATE SET
                status = EXCLUDED.status,
                updated_at = EXCLUDED.updated_at
            "#,
        )
        .bind(instance_id)
        .bind(&status_json)
        .bind(status.updated_at())
        .execute(&self.pool)
        .await?;

        tracing::debug!(
            instance_id = %instance_id,
            status = %status.status_str(),
            "Reported container status"
        );

        Ok(())
    }

    /// Get container status
    pub async fn get_status(&self, instance_id: &str) -> Result<Option<ContainerStatus>> {
        let row: Option<(serde_json::Value,)> =
            sqlx::query_as("SELECT status FROM container_status WHERE instance_id = $1")
                .bind(instance_id)
                .fetch_optional(&self.pool)
                .await?;

        match row {
            Some((status_json,)) => {
                let status: ContainerStatus = serde_json::from_value(status_json)?;
                Ok(Some(status))
            }
            None => Ok(None),
        }
    }

    /// Clear container status
    pub async fn clear_status(&self, instance_id: &str) -> Result<()> {
        sqlx::query("DELETE FROM container_status WHERE instance_id = $1")
            .bind(instance_id)
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    // ===== Heartbeat =====

    /// Send heartbeat
    pub async fn send_heartbeat(&self, instance_id: &str) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO container_heartbeats (instance_id, last_heartbeat)
            VALUES ($1, $2)
            ON CONFLICT (instance_id) DO UPDATE SET
                last_heartbeat = EXCLUDED.last_heartbeat
            "#,
        )
        .bind(instance_id)
        .bind(Utc::now())
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Check if container has a recent heartbeat (within the last 60 seconds)
    pub async fn has_heartbeat(&self, instance_id: &str) -> Result<bool> {
        let cutoff = Utc::now() - chrono::Duration::seconds(60);

        let count: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM container_heartbeats WHERE instance_id = $1 AND last_heartbeat > $2",
        )
        .bind(instance_id)
        .bind(cutoff)
        .fetch_one(&self.pool)
        .await?;

        Ok(count.0 > 0)
    }

    /// Get last heartbeat timestamp
    pub async fn get_heartbeat(&self, instance_id: &str) -> Result<Option<DateTime<Utc>>> {
        let row: Option<(DateTime<Utc>,)> = sqlx::query_as(
            "SELECT last_heartbeat FROM container_heartbeats WHERE instance_id = $1",
        )
        .bind(instance_id)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(|(ts,)| ts))
    }

    /// Clear heartbeat entry
    pub async fn clear_heartbeat(&self, instance_id: &str) -> Result<()> {
        sqlx::query("DELETE FROM container_heartbeats WHERE instance_id = $1")
            .bind(instance_id)
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    // ===== Cleanup =====

    /// Full cleanup for a container (registry, status, heartbeat, cancellation)
    pub async fn cleanup(&self, instance_id: &str) -> Result<()> {
        // Use a transaction to ensure atomicity
        let mut tx = self.pool.begin().await?;

        sqlx::query("DELETE FROM container_registry WHERE instance_id = $1")
            .bind(instance_id)
            .execute(&mut *tx)
            .await?;

        sqlx::query("DELETE FROM container_status WHERE instance_id = $1")
            .bind(instance_id)
            .execute(&mut *tx)
            .await?;

        sqlx::query("DELETE FROM container_cancellations WHERE instance_id = $1")
            .bind(instance_id)
            .execute(&mut *tx)
            .await?;

        sqlx::query("DELETE FROM container_heartbeats WHERE instance_id = $1")
            .bind(instance_id)
            .execute(&mut *tx)
            .await?;

        tx.commit().await?;

        tracing::debug!(
            instance_id = %instance_id,
            "Cleaned up container from registry"
        );

        Ok(())
    }

    /// Clean up stale entries (containers that haven't sent heartbeat in > 24 hours)
    pub async fn cleanup_stale(&self) -> Result<u64> {
        let cutoff = Utc::now() - chrono::Duration::hours(24);

        // Find stale instances
        let stale: Vec<(String,)> = sqlx::query_as(
            r#"
            SELECT r.instance_id
            FROM container_registry r
            LEFT JOIN container_heartbeats h ON r.instance_id = h.instance_id
            WHERE h.last_heartbeat IS NULL OR h.last_heartbeat < $1
            "#,
        )
        .bind(cutoff)
        .fetch_all(&self.pool)
        .await?;

        let count = stale.len() as u64;

        for (instance_id,) in stale {
            let _ = self.cleanup(&instance_id).await;
        }

        if count > 0 {
            tracing::info!(count = count, "Cleaned up stale container entries");
        }

        Ok(count)
    }
}
