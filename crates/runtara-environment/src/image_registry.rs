// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Image Registry
//!
//! Manages "images" - runnable units that can be launched as instances.
//! An image represents a compiled scenario or other executable that can be run.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;

use crate::error::Result;

/// Type of runner that should be used for an image.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "text", rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]
pub enum RunnerType {
    /// OCI container runner (crun)
    #[default]
    Oci,
    /// Native process runner (direct execution)
    Native,
    /// WebAssembly runner
    Wasm,
}

impl std::fmt::Display for RunnerType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RunnerType::Oci => write!(f, "oci"),
            RunnerType::Native => write!(f, "native"),
            RunnerType::Wasm => write!(f, "wasm"),
        }
    }
}

impl std::str::FromStr for RunnerType {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "oci" => Ok(RunnerType::Oci),
            "native" => Ok(RunnerType::Native),
            "wasm" => Ok(RunnerType::Wasm),
            _ => Err(format!("Unknown runner type: {}", s)),
        }
    }
}

/// An image that can be launched as an instance.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Image {
    /// Unique image ID (any non-empty string)
    pub image_id: String,
    /// Tenant that owns this image
    pub tenant_id: String,
    /// Human-readable name
    pub name: String,
    /// Optional description
    pub description: Option<String>,
    /// Path to the executable binary
    pub binary_path: String,
    /// Path to OCI bundle (for OCI runner)
    pub bundle_path: Option<String>,
    /// Type of runner to use
    pub runner_type: RunnerType,
    /// When the image was created
    pub created_at: DateTime<Utc>,
    /// When the image was last updated
    pub updated_at: DateTime<Utc>,
    /// Optional metadata (JSON)
    pub metadata: Option<serde_json::Value>,
}

/// Image registry - manages available images in the database.
pub struct ImageRegistry {
    pool: PgPool,
}

impl ImageRegistry {
    /// Create a new image registry
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Register a new image
    pub async fn register(&self, image: &Image) -> Result<()> {
        let runner_type_str = image.runner_type.to_string();

        sqlx::query(
            r#"
            INSERT INTO images (
                image_id, tenant_id, name, description, binary_path, bundle_path,
                runner_type, created_at, updated_at, metadata
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
            ON CONFLICT (tenant_id, name) DO UPDATE SET
                image_id = EXCLUDED.image_id,
                description = EXCLUDED.description,
                binary_path = EXCLUDED.binary_path,
                bundle_path = EXCLUDED.bundle_path,
                runner_type = EXCLUDED.runner_type,
                updated_at = EXCLUDED.updated_at,
                metadata = EXCLUDED.metadata
            "#,
        )
        .bind(&image.image_id)
        .bind(&image.tenant_id)
        .bind(&image.name)
        .bind(&image.description)
        .bind(&image.binary_path)
        .bind(&image.bundle_path)
        .bind(&runner_type_str)
        .bind(image.created_at)
        .bind(image.updated_at)
        .bind(&image.metadata)
        .execute(&self.pool)
        .await?;

        tracing::info!(
            image_id = %image.image_id,
            name = %image.name,
            runner_type = %runner_type_str,
            "Registered image"
        );

        Ok(())
    }

    /// Get an image by ID
    pub async fn get(&self, image_id: &str) -> Result<Option<Image>> {
        let row: Option<ImageRow> = sqlx::query_as(
            r#"
            SELECT image_id, tenant_id, name, description, binary_path, bundle_path,
                   runner_type, created_at, updated_at, metadata
            FROM images
            WHERE image_id = $1
            "#,
        )
        .bind(image_id)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(|r| r.into()))
    }

    /// Get an image by name for a tenant
    pub async fn get_by_name(&self, tenant_id: &str, name: &str) -> Result<Option<Image>> {
        let row: Option<ImageRow> = sqlx::query_as(
            r#"
            SELECT image_id, tenant_id, name, description, binary_path, bundle_path,
                   runner_type, created_at, updated_at, metadata
            FROM images
            WHERE tenant_id = $1 AND name = $2
            "#,
        )
        .bind(tenant_id)
        .bind(name)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(|r| r.into()))
    }

    /// List images for a tenant
    pub async fn list(&self, tenant_id: &str) -> Result<Vec<Image>> {
        let rows: Vec<ImageRow> = sqlx::query_as(
            r#"
            SELECT image_id, tenant_id, name, description, binary_path, bundle_path,
                   runner_type, created_at, updated_at, metadata
            FROM images
            WHERE tenant_id = $1
            ORDER BY name
            "#,
        )
        .bind(tenant_id)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.into_iter().map(|r| r.into()).collect())
    }

    /// List images for a tenant with pagination
    pub async fn list_by_tenant(
        &self,
        tenant_id: &str,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<Image>> {
        let rows: Vec<ImageRow> = sqlx::query_as(
            r#"
            SELECT image_id, tenant_id, name, description, binary_path, bundle_path,
                   runner_type, created_at, updated_at, metadata
            FROM images
            WHERE tenant_id = $1
            ORDER BY created_at DESC
            LIMIT $2 OFFSET $3
            "#,
        )
        .bind(tenant_id)
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.into_iter().map(|r| r.into()).collect())
    }

    /// List all images with pagination
    pub async fn list_all(&self, limit: i64, offset: i64) -> Result<Vec<Image>> {
        let rows: Vec<ImageRow> = sqlx::query_as(
            r#"
            SELECT image_id, tenant_id, name, description, binary_path, bundle_path,
                   runner_type, created_at, updated_at, metadata
            FROM images
            ORDER BY created_at DESC
            LIMIT $1 OFFSET $2
            "#,
        )
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.into_iter().map(|r| r.into()).collect())
    }

    /// Delete an image
    pub async fn delete(&self, image_id: &str) -> Result<bool> {
        let result = sqlx::query("DELETE FROM images WHERE image_id = $1")
            .bind(image_id)
            .execute(&self.pool)
            .await?;

        Ok(result.rows_affected() > 0)
    }

    /// Update image binary path and bundle path
    pub async fn update_paths(
        &self,
        image_id: &str,
        binary_path: &str,
        bundle_path: Option<&str>,
    ) -> Result<()> {
        sqlx::query(
            r#"
            UPDATE images
            SET binary_path = $2, bundle_path = $3, updated_at = $4
            WHERE image_id = $1
            "#,
        )
        .bind(image_id)
        .bind(binary_path)
        .bind(bundle_path)
        .bind(Utc::now())
        .execute(&self.pool)
        .await?;

        Ok(())
    }
}

/// Internal row type for database queries
#[derive(sqlx::FromRow)]
struct ImageRow {
    image_id: String,
    tenant_id: String,
    name: String,
    description: Option<String>,
    binary_path: String,
    bundle_path: Option<String>,
    runner_type: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    metadata: Option<serde_json::Value>,
}

impl From<ImageRow> for Image {
    fn from(row: ImageRow) -> Self {
        Image {
            image_id: row.image_id,
            tenant_id: row.tenant_id,
            name: row.name,
            description: row.description,
            binary_path: row.binary_path,
            bundle_path: row.bundle_path,
            runner_type: row.runner_type.parse().unwrap_or_default(),
            created_at: row.created_at,
            updated_at: row.updated_at,
            metadata: row.metadata,
        }
    }
}

/// Builder for creating images
pub struct ImageBuilder {
    image_id: Option<String>,
    tenant_id: String,
    name: String,
    description: Option<String>,
    binary_path: String,
    bundle_path: Option<String>,
    runner_type: RunnerType,
    metadata: Option<serde_json::Value>,
}

impl ImageBuilder {
    /// Create a new image builder
    pub fn new(
        tenant_id: impl Into<String>,
        name: impl Into<String>,
        binary_path: impl Into<String>,
    ) -> Self {
        Self {
            image_id: None,
            tenant_id: tenant_id.into(),
            name: name.into(),
            description: None,
            binary_path: binary_path.into(),
            bundle_path: None,
            runner_type: RunnerType::Oci,
            metadata: None,
        }
    }

    /// Set custom image ID (defaults to UUID if not set)
    pub fn image_id(mut self, image_id: impl Into<String>) -> Self {
        self.image_id = Some(image_id.into());
        self
    }

    /// Set description
    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Set bundle path
    pub fn bundle_path(mut self, bundle_path: impl Into<String>) -> Self {
        self.bundle_path = Some(bundle_path.into());
        self
    }

    /// Set runner type
    pub fn runner_type(mut self, runner_type: RunnerType) -> Self {
        self.runner_type = runner_type;
        self
    }

    /// Set metadata
    pub fn metadata(mut self, metadata: serde_json::Value) -> Self {
        self.metadata = Some(metadata);
        self
    }

    /// Build the image
    pub fn build(self) -> Image {
        let now = Utc::now();
        Image {
            image_id: self
                .image_id
                .unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
            tenant_id: self.tenant_id,
            name: self.name,
            description: self.description,
            binary_path: self.binary_path,
            bundle_path: self.bundle_path,
            runner_type: self.runner_type,
            created_at: now,
            updated_at: now,
            metadata: self.metadata,
        }
    }
}
