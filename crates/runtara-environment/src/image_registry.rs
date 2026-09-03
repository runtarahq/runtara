// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Image Registry
//!
//! Manages "images" - runnable units that can be launched as instances.
//! An image represents a compiled workflow or other executable that can be run.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::error::Result;

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
    /// When the image was created
    pub created_at: DateTime<Utc>,
    /// When the image was last updated
    pub updated_at: DateTime<Utc>,
    /// Optional metadata (JSON)
    pub metadata: Option<serde_json::Value>,
}

impl Image {
    /// Whether this image was registered as a compiled workflow rather than an
    /// arbitrary executable component.
    ///
    /// This deliberately keys on the established `workflow` metadata envelope
    /// written by the server, not on a filename or on `wasi:cli/run`: generic
    /// agent components retain their own ABI and must not be rejected merely
    /// because they are not lifecycle-invokable.
    pub fn requires_lifecycle_invoke(&self) -> bool {
        self.metadata
            .as_ref()
            .and_then(|metadata| metadata.get("workflow"))
            .is_some_and(serde_json::Value::is_object)
    }

    /// Immutable SHA-256 identity recorded for a generated direct workflow.
    ///
    /// Generic components deliberately have no such requirement. A workflow
    /// envelope without this value is treated as an unsupported legacy image
    /// by durable preparation instead of falling back to `wasi:cli/run`.
    pub fn workflow_binary_checksum(&self) -> Option<&str> {
        self.metadata
            .as_ref()?
            .pointer("/workflow/binaryChecksum")?
            .as_str()
            .filter(|checksum| !checksum.is_empty())
    }
}

/// Reject a compiled workflow image that does not export the current lifecycle
/// entrypoint. This is called before `launch_detached`, so an old direct
/// `wasi:cli/run` workflow cannot take a runner permit or receive a container
/// registry entry. Images not identified as compiled workflows are intentionally
/// left alone for generic component compatibility.
pub async fn require_current_workflow_entrypoint(image: &Image) -> Result<()> {
    if !image.requires_lifecycle_invoke() {
        return Ok(());
    }

    let binary_path = image.binary_path.clone();
    tokio::task::spawn_blocking(move || {
        runtara_component_host::lifecycle::require_lifecycle_invoke_file(&binary_path)
    })
    .await
    .map_err(|error| {
        crate::error::Error::Other(format!("workflow ABI inspection task panicked: {error}"))
    })?
    .map_err(|error| crate::error::Error::Other(format!("unsupported workflow image: {error:#}")))
}

/// How long a read of the `images` row is reused.
///
/// A launch reads its image on the way in, and the same image backs every
/// launch of that workflow version, so this is one read per burst instead of
/// one per instance. Short rather than permanent because the row is not truly
/// immutable: `register` upserts on `(tenant_id, name)`, so re-registering a
/// workflow rewrites the row a live id already points at.
const IMAGE_CACHE_TTL: Duration = Duration::from_secs(5);

type ImageCache = std::sync::Mutex<HashMap<String, (Instant, Image)>>;

fn image_cache() -> &'static ImageCache {
    static CACHE: std::sync::OnceLock<ImageCache> = std::sync::OnceLock::new();
    CACHE.get_or_init(|| std::sync::Mutex::new(HashMap::new()))
}

fn cached_image(image_id: &str) -> Option<Image> {
    let cache = image_cache().lock().ok()?;
    cache
        .get(image_id)
        .filter(|(at, _)| at.elapsed() < IMAGE_CACHE_TTL)
        .map(|(_, image)| image.clone())
}

fn cache_image(image_id: &str, image: &Image) {
    if let Ok(mut cache) = image_cache().lock() {
        cache.insert(image_id.to_string(), (Instant::now(), image.clone()));
    }
}

/// Drop every cached row.
///
/// Called on any write to `images`. Registration upserts by name, so the id
/// whose row it rewrote is not known here — and registration is rare next to
/// launching, so clearing everything is both correct and cheap.
fn invalidate_image_cache() {
    if let Ok(mut cache) = image_cache().lock() {
        cache.clear();
    }
}

/// Image registry - manages available images in the database.
pub struct ImageRegistry {
    pool: PgPool,
}

/// The stable image ID claimed for a tenant-scoped name.
///
/// `images` preserves its original primary key when a name is re-registered,
/// because `instance_images` refers to that ID.  Callers which need to create
/// the directory before registering the final metadata must therefore claim
/// the name first rather than speculating that a fresh UUID will win the
/// unique `(tenant_id, name)` race.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageNameClaim {
    /// The canonical image ID for this name.
    pub image_id: String,
    /// Whether this caller inserted the row which established the name.
    pub created: bool,
}

impl ImageRegistry {
    /// Create a new image registry
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Atomically claim the stable ID for an image name.
    ///
    /// The first writer inserts a minimal row using `candidate_image_id`; all
    /// subsequent writers read that exact ID.  Keeping this as an insert plus
    /// a new-statement read (rather than a read followed by an insert) is
    /// important: PostgreSQL's uniqueness check observes concurrent inserts
    /// which the earlier statement snapshot cannot yet see.
    ///
    /// The placeholder is deliberately retained if filesystem work later
    /// fails.  Removing it from a failed request could race another uploader
    /// which has already resolved the same canonical ID.  It is harmless to
    /// normal callers because no workflow stores the ID until registration
    /// succeeds, and the regular stale-image cleanup eventually reclaims an
    /// abandoned placeholder.
    pub async fn claim_name(
        &self,
        tenant_id: &str,
        name: &str,
        candidate_image_id: &str,
        binary_path: &str,
    ) -> Result<ImageNameClaim> {
        // A concurrent delete after a losing insert is extraordinarily rare,
        // but retrying makes the read-after-conflict path robust without ever
        // returning a speculative ID.
        for _ in 0..3 {
            let inserted: Option<String> = sqlx::query_scalar(
                r#"
                INSERT INTO images (
                    image_id, tenant_id, name, binary_path, created_at, updated_at
                ) VALUES ($1, $2, $3, $4, NOW(), NOW())
                ON CONFLICT (tenant_id, name) DO NOTHING
                RETURNING image_id
                "#,
            )
            .bind(candidate_image_id)
            .bind(tenant_id)
            .bind(name)
            .bind(binary_path)
            .fetch_optional(&self.pool)
            .await?;

            if let Some(image_id) = inserted {
                invalidate_image_cache();
                return Ok(ImageNameClaim {
                    image_id,
                    created: true,
                });
            }

            let existing: Option<String> = sqlx::query_scalar(
                "SELECT image_id FROM images WHERE tenant_id = $1 AND name = $2",
            )
            .bind(tenant_id)
            .bind(name)
            .fetch_optional(&self.pool)
            .await?;

            if let Some(image_id) = existing {
                return Ok(ImageNameClaim {
                    image_id,
                    created: false,
                });
            }
        }

        Err(crate::error::Error::Other(format!(
            "image name '{name}' for tenant '{tenant_id}' was repeatedly removed while being claimed"
        )))
    }

    /// Register a new image
    pub async fn register(&self, image: &Image) -> Result<()> {
        invalidate_image_cache();
        sqlx::query(
            r#"
            INSERT INTO images (
                image_id, tenant_id, name, description, binary_path,
                created_at, updated_at, metadata
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
            ON CONFLICT (tenant_id, name) DO UPDATE SET
                description = EXCLUDED.description,
                binary_path = EXCLUDED.binary_path,
                updated_at = EXCLUDED.updated_at,
                metadata = EXCLUDED.metadata
            "#,
        )
        .bind(&image.image_id)
        .bind(&image.tenant_id)
        .bind(&image.name)
        .bind(&image.description)
        .bind(&image.binary_path)
        .bind(image.created_at)
        .bind(image.updated_at)
        .bind(&image.metadata)
        .execute(&self.pool)
        .await?;

        tracing::info!(
            image_id = %image.image_id,
            name = %image.name,
            "Registered image"
        );

        Ok(())
    }

    /// Get an image by ID
    pub async fn get(&self, image_id: &str) -> Result<Option<Image>> {
        if let Some(hit) = cached_image(image_id) {
            return Ok(Some(hit));
        }

        let row: Option<ImageRow> = sqlx::query_as(
            r#"
            SELECT image_id, tenant_id, name, description, binary_path,
                   created_at, updated_at, metadata
            FROM images
            WHERE image_id = $1
            "#,
        )
        .bind(image_id)
        .fetch_optional(&self.pool)
        .await?;

        let image: Option<Image> = row.map(|r| r.into());
        if let Some(image) = &image {
            cache_image(image_id, image);
        }
        Ok(image)
    }

    /// Get an image by name for a tenant
    pub async fn get_by_name(&self, tenant_id: &str, name: &str) -> Result<Option<Image>> {
        let row: Option<ImageRow> = sqlx::query_as(
            r#"
            SELECT image_id, tenant_id, name, description, binary_path,
                   created_at, updated_at, metadata
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
            SELECT image_id, tenant_id, name, description, binary_path,
                   created_at, updated_at, metadata
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
            SELECT image_id, tenant_id, name, description, binary_path,
                   created_at, updated_at, metadata
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
            SELECT image_id, tenant_id, name, description, binary_path,
                   created_at, updated_at, metadata
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
        invalidate_image_cache();
        let result = sqlx::query("DELETE FROM images WHERE image_id = $1")
            .bind(image_id)
            .execute(&self.pool)
            .await?;

        Ok(result.rows_affected() > 0)
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
            created_at: now,
            updated_at: now,
            metadata: self.metadata,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_compiled_workflow_images_require_lifecycle_invoke() {
        let workflow = ImageBuilder::new("tenant", "workflow", "/tmp/workflow.wasm")
            .metadata(serde_json::json!({
                "workflow": {
                    "compilerMode": "direct-wasm",
                    "directWasm": { "entryAbi": "invoke" }
                }
            }))
            .build();
        assert!(workflow.requires_lifecycle_invoke());

        let generic_agent = ImageBuilder::new("tenant", "agent", "/tmp/agent.wasm")
            .metadata(serde_json::json!({ "agent": { "id": "custom" } }))
            .build();
        assert!(!generic_agent.requires_lifecycle_invoke());

        let legacy_without_workflow_metadata =
            ImageBuilder::new("tenant", "old-agent", "/tmp/agent.wasm").build();
        assert!(!legacy_without_workflow_metadata.requires_lifecycle_invoke());
    }
}
