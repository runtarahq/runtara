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
                container_id, instance_id, tenant_id, binary_path,
                started_at, timeout_seconds
            ) VALUES ($1, $2, $3, $4, $5, $6)
            ON CONFLICT (instance_id) DO UPDATE SET
                container_id = EXCLUDED.container_id,
                binary_path = EXCLUDED.binary_path,
                started_at = EXCLUDED.started_at,
                timeout_seconds = EXCLUDED.timeout_seconds
            "#,
        )
        .bind(&info.container_id)
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

    /// Full cleanup for a container (registry, status, heartbeat)
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
}
