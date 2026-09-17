// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Container Registry
//!
//! PostgreSQL-based registry for tracking running containers/instances.
//! Enables fire-and-forget launching, runtime restart recovery, and distributed cancellation.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;

use crate::error::Result;

/// Container registry entry stored in PostgreSQL
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct ContainerInfo {
    /// Container/handle ID used with the runner
    pub container_id: String,
    /// Durable launch queue generation for this runner handle.
    ///
    /// An instance can park and resume under the same durable ID. This value
    /// distinguishes separate launches. A pre-start retry can reuse it, so
    /// exact physical ownership also requires `container_id`.
    pub launch_id: String,
    /// Execution instance ID (UUID)
    pub instance_id: String,
    /// Tenant ID
    pub tenant_id: String,
    /// Path to the executable binary
    pub binary_path: String,
    /// When the container was started
    pub started_at: DateTime<Utc>,
    /// Execution timeout in seconds
    pub timeout_seconds: Option<i64>,
}

impl ContainerInfo {
    /// Reconstruct the persisted physical control handle, without live metrics.
    pub fn runner_handle(&self) -> crate::runner::RunnerHandle {
        crate::runner::RunnerHandle {
            launch_id: self.launch_id.clone(),
            handle_id: self.container_id.clone(),
            instance_id: self.instance_id.clone(),
            tenant_id: self.tenant_id.clone(),
            started_at: self.started_at,
            metrics: None,
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
                container_id, launch_id, instance_id, tenant_id, binary_path,
                started_at, timeout_seconds
            ) VALUES ($1, $2, $3, $4, $5, $6, $7)
            ON CONFLICT (instance_id) DO UPDATE SET
                container_id = EXCLUDED.container_id,
                launch_id = EXCLUDED.launch_id,
                binary_path = EXCLUDED.binary_path,
                started_at = EXCLUDED.started_at,
                timeout_seconds = EXCLUDED.timeout_seconds,
                abort_deadline_at = CASE WHEN container_registry.container_id = EXCLUDED.container_id
                    AND container_registry.launch_id = EXCLUDED.launch_id
                    THEN container_registry.abort_deadline_at ELSE NULL END,
                abort_armed_deadline_at = CASE WHEN container_registry.container_id = EXCLUDED.container_id
                    AND container_registry.launch_id = EXCLUDED.launch_id
                    THEN container_registry.abort_armed_deadline_at ELSE NULL END
            "#,
        )
        .bind(&info.container_id)
        .bind(&info.launch_id)
        .bind(&info.instance_id)
        .bind(&info.tenant_id)
        .bind(&info.binary_path)
        .bind(info.started_at)
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

    /// List all registered containers (all tenants)
    pub async fn list_all_registered(&self) -> Result<Vec<ContainerInfo>> {
        let containers = sqlx::query_as::<_, ContainerInfo>("SELECT * FROM container_registry")
            .fetch_all(&self.pool)
            .await?;

        Ok(containers)
    }

    /// Bounded candidates whose running owner lease expired. Recovery still
    /// locks and rechecks each claim before changing any lifecycle state.
    pub async fn expired_running_owners(&self) -> Result<Vec<ContainerInfo>> {
        Ok(sqlx::query_as::<_, ContainerInfo>(
            "SELECT cr.* FROM instance_launches launch \
             JOIN container_registry cr ON cr.launch_id = launch.launch_id \
                 AND cr.instance_id = launch.instance_id \
             WHERE launch.state = 'running' AND launch.lease_owner IS NOT NULL \
                 AND launch.lease_expires_at <= clock_timestamp() \
             ORDER BY launch.lease_expires_at LIMIT 256",
        )
        .fetch_all(&self.pool)
        .await?)
    }

    /// Get a specific container's info
    /// Just the instance ids this process is tracking.
    ///
    /// The heartbeat monitor wants a membership set, not the rows, and used to
    /// read the table itself for it.
    pub async fn tracked_instance_ids(&self) -> Result<Vec<String>> {
        Ok(
            sqlx::query_scalar::<_, String>("SELECT instance_id FROM container_registry")
                .fetch_all(&self.pool)
                .await?,
        )
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

    /// Persist emergency grace for an exact physical run with a live owner.
    /// Map the caller's remaining monotonic budget using the database clock;
    /// sampling/transport time is subtracted, never a fresh grace interval.
    pub async fn request_abort(
        &self,
        handle: &crate::runner::RunnerHandle,
        deadline: tokio::time::Instant,
    ) -> Result<Option<DateTime<Utc>>> {
        let database_now: DateTime<Utc> = sqlx::query_scalar("SELECT clock_timestamp()")
            .fetch_one(&self.pool)
            .await?;
        let remaining = chrono::Duration::from_std(
            deadline.saturating_duration_since(tokio::time::Instant::now()),
        )
        .map_err(|_| {
            crate::error::Error::InvalidRequest("Cancellation deadline is out of range".into())
        })?;
        let requested = database_now.checked_add_signed(remaining).ok_or_else(|| {
            crate::error::Error::InvalidRequest("Cancellation deadline is out of range".into())
        })?;
        Ok(sqlx::query_scalar(
            "UPDATE container_registry cr SET abort_deadline_at = LEAST(cr.abort_deadline_at, $4) \
             WHERE cr.instance_id = $1 AND cr.launch_id = $2 AND cr.container_id = $3 \
               AND EXISTS (SELECT 1 FROM instance_launches launch \
                   WHERE launch.launch_id = cr.launch_id AND launch.instance_id = cr.instance_id \
                     AND launch.state = 'running' AND launch.lease_owner IS NOT NULL \
                     AND launch.lease_expires_at > clock_timestamp()) \
             RETURNING abort_deadline_at",
        )
        .bind(&handle.instance_id)
        .bind(&handle.launch_id)
        .bind(&handle.handle_id)
        .bind(requested)
        .fetch_optional(&self.pool)
        .await?)
    }

    /// An acknowledgement is about a native timer, not guest cleanup or outcome.
    /// Earlier timers satisfy later requests; stale physical handles never do.
    pub async fn abort_is_armed(
        &self,
        handle: &crate::runner::RunnerHandle,
        deadline: DateTime<Utc>,
    ) -> Result<bool> {
        Ok(sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM container_registry \
             WHERE instance_id = $1 AND launch_id = $2 AND container_id = $3 \
               AND abort_armed_deadline_at <= $4)",
        )
        .bind(&handle.instance_id)
        .bind(&handle.launch_id)
        .bind(&handle.handle_id)
        .bind(deadline)
        .fetch_one(&self.pool)
        .await?)
    }

    /// Deliver pending whole-run deadlines from the existing dispatcher pass.
    /// No per-step tasks or new worker are introduced. Record delivery only
    /// after the exact local runner accepted its monotonic timer.
    pub async fn deliver_abort_requests(
        &self,
        owner: &str,
        runner: &dyn crate::runner::Runner,
        limit: usize,
    ) -> Result<usize> {
        #[derive(sqlx::FromRow)]
        struct Request {
            #[sqlx(flatten)]
            container: ContainerInfo,
            abort_deadline_at: DateTime<Utc>,
            remaining_us: i64,
        }
        let observed_at = tokio::time::Instant::now();
        let requests = sqlx::query_as::<_, Request>(
            "SELECT cr.*, GREATEST(0, EXTRACT(EPOCH FROM \
                 (cr.abort_deadline_at - clock_timestamp())) * 1000000)::bigint AS remaining_us \
             FROM container_registry cr JOIN instance_launches launch \
               ON launch.launch_id = cr.launch_id AND launch.instance_id = cr.instance_id \
             WHERE launch.lease_owner = $1 AND launch.state = 'running' \
               AND launch.lease_expires_at > clock_timestamp() \
               AND cr.abort_deadline_at IS NOT NULL \
               AND (cr.abort_armed_deadline_at IS NULL OR cr.abort_armed_deadline_at > cr.abort_deadline_at) \
             ORDER BY cr.abort_deadline_at LIMIT $2",
        )
        .bind(owner)
        .bind(i64::try_from(limit).unwrap_or(i64::MAX))
        .fetch_all(&self.pool)
        .await?;
        let mut delivered = 0;
        for request in requests {
            let handle = request.container.runner_handle();
            // Request-start time is before the database sample; this converts
            // transit/row-wait latency into an earlier timer, never extra grace.
            let deadline = observed_at
                .checked_add(std::time::Duration::from_micros(
                    request.remaining_us as u64,
                ))
                .ok_or_else(|| {
                    crate::error::Error::InvalidRequest(
                        "Cancellation deadline is out of range".into(),
                    )
                })?;
            if runner.schedule_abort(&handle, deadline).await? {
                sqlx::query(
                    "UPDATE container_registry SET abort_armed_deadline_at = LEAST(abort_armed_deadline_at, $4) \
                     WHERE instance_id = $1 AND launch_id = $2 AND container_id = $3",
                )
                .bind(&handle.instance_id)
                .bind(&handle.launch_id)
                .bind(&handle.handle_id)
                .bind(request.abort_deadline_at)
                .execute(&self.pool)
                .await?;
                delivered += 1;
            }
        }
        Ok(delivered)
    }

    // ===== Cleanup =====

    /// Remove a container only if the registry still holds this exact
    /// `launch_id`, reporting whether it did.
    ///
    /// A generation guard. Anything that selected a container and then acts on
    /// it later is racing a wake: the instance can be relaunched in between,
    /// which writes a fresh row with a new `launch_id`. Deleting by instance
    /// alone would throw away the live run's row, so this doubles as an
    /// ownership claim — `false` means a newer run owns the instance and the
    /// caller must leave it alone.
    pub async fn cleanup_generation(&self, instance_id: &str, launch_id: &str) -> Result<bool> {
        let result =
            sqlx::query("DELETE FROM container_registry WHERE instance_id = $1 AND launch_id = $2")
                .bind(instance_id)
                .bind(launch_id)
                .execute(&self.pool)
                .await?;

        let removed = result.rows_affected() == 1;
        tracing::debug!(
            instance_id = %instance_id,
            launch_id = %launch_id,
            removed = removed,
            "Generation-guarded container cleanup"
        );
        Ok(removed)
    }

    /// Remove a runner record only if the exact physical handle still owns
    /// the row.
    ///
    /// A durable launch id can be retried after a pre-guest recovery, so it
    /// is not by itself a sufficient fence for a monitor that already has a
    /// stale handle. Callers that possess the handle id must use this stronger
    /// form before changing paired durable state.
    pub async fn cleanup_handle(
        &self,
        instance_id: &str,
        launch_id: &str,
        container_id: &str,
    ) -> Result<bool> {
        let result = sqlx::query(
            "DELETE FROM container_registry \
             WHERE instance_id = $1 AND launch_id = $2 AND container_id = $3",
        )
        .bind(instance_id)
        .bind(launch_id)
        .bind(container_id)
        .execute(&self.pool)
        .await?;

        let removed = result.rows_affected() == 1;
        tracing::debug!(
            instance_id = %instance_id,
            launch_id = %launch_id,
            container_id = %container_id,
            removed = removed,
            "Handle-guarded container cleanup"
        );
        Ok(removed)
    }

    /// Drop a container's registry entry, once it has reached a terminal state.
    pub async fn cleanup(&self, instance_id: &str) -> Result<()> {
        sqlx::query("DELETE FROM container_registry WHERE instance_id = $1")
            .bind(instance_id)
            .execute(&self.pool)
            .await?;

        tracing::debug!(
            instance_id = %instance_id,
            "Cleaned up container from registry"
        );

        Ok(())
    }
}
