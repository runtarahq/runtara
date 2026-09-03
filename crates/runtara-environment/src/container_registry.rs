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
    /// Immutable physical launch generation for this runner handle.
    ///
    /// An instance can park and resume under the same durable ID. This value
    /// is the fence that lets cleanup prove it still owns the observed run.
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
                timeout_seconds = EXCLUDED.timeout_seconds
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
