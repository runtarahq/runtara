use chrono::{DateTime, Utc};
use serde_json::Value;
use sha2::Digest;
use sqlx::{PgPool, Row};

use crate::api::dto::workflows::{Note, WorkflowDto, WorkflowVersionInfoDto};
use crate::types::MemoryTier;

type WorkflowVersionRow = (
    i32,                   // version
    DateTime<Utc>,         // created_at
    DateTime<Utc>,         // updated_at
    bool,                  // track_events
    Option<DateTime<Utc>>, // compiled_at
    Option<i32>,           // current_version
    Option<i32>,           // latest_version
);

pub fn workflow_definition_checksum(definition: &Value) -> String {
    let bytes = serde_json::to_vec(definition).unwrap_or_default();
    hex::encode(sha2::Sha256::digest(&bytes))
}

pub struct CompilationSuccessRecord<'a> {
    pub tenant_id: &'a str,
    pub workflow_id: &'a str,
    pub version: i32,
    pub build_dir: &'a std::path::Path,
    pub binary_size: i32,
    pub binary_checksum: &'a str,
    pub source_checksum: &'a str,
}

/// Repository for workflow CRUD operations
#[allow(dead_code)]
pub struct WorkflowRepository {
    pool: PgPool,
}

#[allow(dead_code)]
impl WorkflowRepository {
    /// Create a new WorkflowRepository
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Get a reference to the database pool
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    // ============================================================================
    // Workflow Metadata Operations
    // ============================================================================

    /// Create a new workflow metadata entry
    /// Returns (created_at, updated_at) timestamps
    /// Note: name/description are now stored in the execution graph, not in the workflows table
    pub async fn create(
        &self,
        tenant_id: &str,
        workflow_id: &str,
    ) -> Result<(DateTime<Utc>, DateTime<Utc>), sqlx::Error> {
        let row = sqlx::query!(
            r#"
            INSERT INTO workflows (tenant_id, workflow_id, version_count, latest_version)
            VALUES ($1, $2, 0, 0)
            ON CONFLICT (tenant_id, workflow_id) DO UPDATE
            SET updated_at = NOW()
            RETURNING created_at, updated_at
            "#,
            tenant_id,
            workflow_id
        )
        .fetch_one(&self.pool)
        .await?;

        Ok((row.created_at, row.updated_at))
    }

    /// Create an initial version (version 1) for a new workflow with an empty graph
    /// The name and description are embedded in the execution graph
    pub async fn create_initial_version(
        &self,
        tenant_id: &str,
        workflow_id: &str,
        name: &str,
        description: &str,
        memory_tier: MemoryTier,
        track_events: bool,
    ) -> Result<(), sqlx::Error> {
        let initial_definition = serde_json::json!({
            "name": name,
            "description": description,
            "steps": {},
            "executionPlan": [],
            "entryPoint": null
        });
        let definition_bytes = serde_json::to_vec(&initial_definition).unwrap_or_default();
        let file_size = definition_bytes.len();

        sqlx::query(
            r#"
            INSERT INTO workflow_definitions (tenant_id, workflow_id, version, definition, file_size, memory_tier, track_events)
            VALUES ($1, $2, 1, $3, $4, $5, $6)
            "#,
        )
        .bind(tenant_id)
        .bind(workflow_id)
        .bind(initial_definition)
        .bind(file_size as i32)
        .bind(memory_tier as MemoryTier)
        .bind(track_events)
        .execute(&self.pool)
        .await?;

        // Point the workflow row at the version we just inserted. Without this,
        // `workflows.latest_version` stays at 0 (the value `create()` set) and
        // `get_current_or_latest_version` then asks for definition v=0, which
        // doesn't exist — every subsequent read 404s.
        sqlx::query!(
            r#"
            UPDATE workflows
            SET latest_version = 1,
                version_count = 1,
                updated_at = NOW()
            WHERE tenant_id = $1 AND workflow_id = $2
            "#,
            tenant_id,
            workflow_id
        )
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Get the latest version number for a workflow
    pub async fn get_latest_version(
        &self,
        tenant_id: &str,
        workflow_id: &str,
    ) -> Result<Option<i32>, sqlx::Error> {
        let row = sqlx::query!(
            r#"
            SELECT latest_version
            FROM workflows
            WHERE tenant_id = $1 AND workflow_id = $2 AND deleted_at IS NULL
            "#,
            tenant_id,
            workflow_id
        )
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.and_then(|r| r.latest_version))
    }

    /// Get the current (active) version number for a workflow
    /// Falls back to latest_version if current_version is not set
    pub async fn get_current_or_latest_version(
        &self,
        tenant_id: &str,
        workflow_id: &str,
    ) -> Result<Option<i32>, sqlx::Error> {
        let row = sqlx::query!(
            r#"
            SELECT COALESCE(current_version, latest_version) as "version"
            FROM workflows
            WHERE tenant_id = $1 AND workflow_id = $2 AND deleted_at IS NULL
            "#,
            tenant_id,
            workflow_id
        )
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.and_then(|r| r.version))
    }

    /// Check if a workflow exists (not soft-deleted)
    pub async fn exists(&self, tenant_id: &str, workflow_id: &str) -> Result<bool, sqlx::Error> {
        let row = sqlx::query!(
            r#"
            SELECT 1 as "exists!"
            FROM workflows
            WHERE tenant_id = $1 AND workflow_id = $2 AND deleted_at IS NULL
            "#,
            tenant_id,
            workflow_id
        )
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.is_some())
    }

    /// Soft delete a workflow (marks as deleted, does not remove from database)
    /// Returns the number of rows affected
    pub async fn delete(&self, tenant_id: &str, workflow_id: &str) -> Result<u64, sqlx::Error> {
        // Use a transaction to ensure atomicity - both tables must be updated together
        let mut tx = self.pool.begin().await?;

        // First, mark all workflow definitions as deleted
        sqlx::query!(
            r#"
            UPDATE workflow_definitions
            SET deleted_at = NOW()
            WHERE tenant_id = $1 AND workflow_id = $2 AND deleted_at IS NULL
            "#,
            tenant_id,
            workflow_id
        )
        .execute(&mut *tx)
        .await?;

        // Then mark the workflow metadata as deleted
        let result = sqlx::query!(
            r#"
            UPDATE workflows
            SET deleted_at = NOW()
            WHERE tenant_id = $1 AND workflow_id = $2 AND deleted_at IS NULL
            "#,
            tenant_id,
            workflow_id
        )
        .execute(&mut *tx)
        .await?;

        // Commit the transaction
        tx.commit().await?;

        Ok(result.rows_affected())
    }

    /// List all workflows (metadata only) with pagination and optional folder filtering
    /// Returns (list of workflows, total count)
    /// Note: name/description are extracted from the execution graph (definition)
    ///
    /// # Arguments
    /// * `path` - Optional folder path filter. If None, returns all workflows (backward compatible).
    ///   If Some, filters by exact path match (recursive=false) or prefix match (recursive=true).
    /// * `recursive` - If true and path is provided, includes workflows in subfolders.
    #[allow(clippy::type_complexity)]
    pub async fn list(
        &self,
        tenant_id: &str,
        page: i32,
        page_size: i32,
        path: Option<&str>,
        recursive: bool,
        search: Option<&str>,
    ) -> Result<(Vec<WorkflowDto>, i64), sqlx::Error> {
        let offset = (page - 1) * page_size;

        // Build path filter value
        let (_, _, path_value) = match (path, recursive) {
            (None, _) => ("".to_string(), "".to_string(), None),
            (Some(p), false) => (
                " AND s.path = $2".to_string(),
                " AND s.path = $4".to_string(),
                Some(p.to_string()),
            ),
            (Some(p), true) => (
                " AND s.path LIKE $2".to_string(),
                " AND s.path LIKE $4".to_string(),
                Some(format!("{}%", p)),
            ),
        };

        // Build search filter — searches workflow name in the definition JSON
        let search_value = search
            .filter(|s| !s.trim().is_empty())
            .map(|s| format!("%{}%", s.to_lowercase()));

        // Collect all bind values in order for dynamic query building
        // Count query always starts with $1 = tenant_id
        let mut count_conditions = String::new();
        let mut main_conditions = String::new();
        let mut count_bind_idx = 2u32; // next available param for count
        let mut main_bind_idx = 4u32; // next available param for main (after $1=tenant, $2=limit, $3=offset)

        // Both queries need the definition join when search is used
        let count_join = if search_value.is_some() {
            " LEFT JOIN workflow_definitions sd ON s.tenant_id = sd.tenant_id AND s.workflow_id = sd.workflow_id AND COALESCE(s.current_version, s.latest_version) = sd.version"
        } else {
            ""
        };

        if path_value.is_some() {
            count_conditions.push_str(&format!(
                " AND s.path {} ${}",
                if recursive { "LIKE" } else { "=" },
                count_bind_idx
            ));
            count_bind_idx += 1;
            main_conditions.push_str(&format!(
                " AND s.path {} ${}",
                if recursive { "LIKE" } else { "=" },
                main_bind_idx
            ));
            main_bind_idx += 1;
        }
        if search_value.is_some() {
            count_conditions.push_str(&format!(
                " AND LOWER(sd.definition->>'name') LIKE ${}",
                count_bind_idx
            ));
            // count_bind_idx += 1; // last param
            main_conditions.push_str(&format!(
                " AND LOWER(sd.definition->>'name') LIKE ${}",
                main_bind_idx
            ));
            // main_bind_idx += 1; // last param
        }

        // Get total count
        let count_start = std::time::Instant::now();
        tracing::debug!("repo.list: starting count query");

        let count_query = format!(
            r#"
            SELECT COUNT(*) as count
            FROM workflows s{}
            WHERE s.tenant_id = $1 AND s.deleted_at IS NULL{}
            "#,
            count_join, count_conditions
        );

        let mut count_q = sqlx::query_scalar(&count_query).bind(tenant_id);
        if let Some(ref pv) = path_value {
            count_q = count_q.bind(pv);
        }
        if let Some(ref sv) = search_value {
            count_q = count_q.bind(sv);
        }
        let total_count: i64 = count_q.fetch_one(&self.pool).await?;

        tracing::debug!(
            duration_ms = count_start.elapsed().as_millis(),
            "repo.list: count query completed"
        );

        // Get paginated results
        let main_query_start = std::time::Instant::now();
        tracing::debug!("repo.list: starting main query");

        let main_query = format!(
            r#"
            SELECT s.workflow_id, s.latest_version, s.current_version, s.created_at, s.updated_at,
                   sd.memory_tier, sd.track_events,
                   sd.definition,
                   s.path
            FROM workflows s
            LEFT JOIN workflow_definitions sd ON s.tenant_id = sd.tenant_id
                AND s.workflow_id = sd.workflow_id
                AND COALESCE(s.current_version, s.latest_version) = sd.version
            WHERE s.tenant_id = $1 AND s.deleted_at IS NULL{}
            ORDER BY s.updated_at DESC
            LIMIT $2 OFFSET $3
            "#,
            main_conditions
        );

        let mut main_q = sqlx::query_as(&main_query)
            .bind(tenant_id)
            .bind(page_size as i64)
            .bind(offset as i64);
        if let Some(ref pv) = path_value {
            main_q = main_q.bind(pv);
        }
        if let Some(ref sv) = search_value {
            main_q = main_q.bind(sv);
        }

        let rows: Vec<(
            String,         // workflow_id
            Option<i32>,    // latest_version
            Option<i32>,    // current_version
            DateTime<Utc>,  // created_at
            DateTime<Utc>,  // updated_at
            Option<String>, // memory_tier
            Option<bool>,   // track_events
            Option<Value>,  // definition (execution_graph)
            String,         // path
        )> = main_q.fetch_all(&self.pool).await?;

        tracing::debug!(
            duration_ms = main_query_start.elapsed().as_millis(),
            row_count = rows.len(),
            "repo.list: main query completed"
        );

        let workflows =
            rows.iter()
                .map(|row| {
                    let memory_tier = row
                        .5
                        .as_ref()
                        .and_then(|s| MemoryTier::parse(s))
                        .unwrap_or_default();
                    let current_version = row.2.unwrap_or_else(|| row.1.unwrap_or(0));

                    // Extract name, description, schemas, variables, and execution_timeout from execution_graph
                    let execution_graph = row.7.clone().unwrap_or(serde_json::json!({}));
                    let name = execution_graph
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let description = execution_graph
                        .get("description")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let input_schema = execution_graph
                        .get("inputSchema")
                        .cloned()
                        .unwrap_or(serde_json::json!({}));
                    let output_schema = execution_graph
                        .get("outputSchema")
                        .cloned()
                        .unwrap_or(serde_json::json!({}));
                    let variables = execution_graph
                        .get("variables")
                        .cloned()
                        .unwrap_or(serde_json::json!([]));
                    let execution_timeout = execution_graph
                        .get("executionTimeoutSeconds")
                        .and_then(|v| {
                            v.as_i64()
                                .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
                        });

                    WorkflowDto {
                        id: row.0.clone(),
                        created: row.3.to_rfc3339(),
                        updated: row.4.to_rfc3339(),
                        started: None,
                        finished: None,
                        execution_time: None,
                        execution_timeout,
                        name,
                        description,
                        execution_graph: serde_json::json!({}),
                        input_schema,
                        output_schema,
                        variables,
                        current_version_number: current_version,
                        last_version_number: row.1.unwrap_or(0),
                        memory_tier,
                        track_events: row.6.unwrap_or(false),
                        notes: Vec::new(), // Empty for list view (no execution graph loaded)
                        path: row.8.clone(),
                    }
                })
                .collect();

        Ok((workflows, total_count))
    }

    /// Get a specific workflow by ID and optional version
    /// If version is None, returns the current version (or latest if current is not set)
    /// Note: name/description are extracted from the execution graph (definition)
    #[allow(clippy::type_complexity)]
    pub async fn get_by_id(
        &self,
        tenant_id: &str,
        workflow_id: &str,
        version: Option<i32>,
    ) -> Result<Option<WorkflowDto>, sqlx::Error> {
        // Determine which version to fetch
        let version_to_fetch = if let Some(v) = version {
            v
        } else {
            // Get current version (falls back to latest if not set)
            match self
                .get_current_or_latest_version(tenant_id, workflow_id)
                .await?
            {
                Some(v) => v,
                None => return Ok(None), // Workflow not found
            }
        };

        // Query workflow definition and metadata
        // The definition column in workflow_definitions IS the execution_graph
        // name/description are now extracted from the definition JSON
        let row: Option<(
            Value,         // definition (execution_graph)
            DateTime<Utc>, // created_at
            DateTime<Utc>, // updated_at
            String,        // memory_tier
            bool,          // track_events
            Option<i32>,   // latest_version
            Option<i32>,   // current_version
            String,        // path
        )> = sqlx::query_as(
            r#"
            SELECT sd.definition, sd.created_at, sd.updated_at, sd.memory_tier, sd.track_events,
                   s.latest_version, s.current_version, s.path
            FROM workflow_definitions sd
            JOIN workflows s ON sd.tenant_id = s.tenant_id AND sd.workflow_id = s.workflow_id
            WHERE sd.tenant_id = $1 AND sd.workflow_id = $2 AND sd.version = $3
            AND sd.deleted_at IS NULL AND s.deleted_at IS NULL
            "#,
        )
        .bind(tenant_id)
        .bind(workflow_id)
        .bind(version_to_fetch)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(|r| {
            let memory_tier = MemoryTier::parse(&r.3).unwrap_or_default();
            let current_version = r.6.unwrap_or_else(|| r.5.unwrap_or(0));

            // Extract name, description, schemas, variables, and execution_timeout from the definition (which is the execution_graph)
            let execution_graph = &r.0;
            let name = execution_graph
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let description = execution_graph
                .get("description")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let input_schema = execution_graph
                .get("inputSchema")
                .cloned()
                .unwrap_or(serde_json::json!({}));
            let output_schema = execution_graph
                .get("outputSchema")
                .cloned()
                .unwrap_or(serde_json::json!({}));
            let variables = execution_graph
                .get("variables")
                .cloned()
                .unwrap_or(serde_json::json!([]));
            let execution_timeout = execution_graph
                .get("executionTimeoutSeconds")
                .and_then(|v| {
                    v.as_i64()
                        .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
                });

            WorkflowDto {
                id: workflow_id.to_string(),
                created: r.1.to_rfc3339(),
                updated: r.2.to_rfc3339(),
                started: None,
                finished: None,
                execution_time: None,
                execution_timeout,
                name,
                description,
                execution_graph: r.0.clone(),
                input_schema,
                output_schema,
                variables,
                current_version_number: current_version,
                last_version_number: r.5.unwrap_or(0),
                memory_tier,
                track_events: r.4,
                notes: Note::extract_from_execution_graph(&r.0),
                path: r.7.clone(),
            }
        }))
    }

    // ============================================================================
    // Workflow Definition (Version) Operations
    // ============================================================================

    /// Create a new version of a workflow
    /// Returns the new version number
    pub async fn create_version(
        &self,
        tenant_id: &str,
        workflow_id: &str,
        definition: &Value,
    ) -> Result<i32, sqlx::Error> {
        // Calculate file size from JSON definition
        let definition_bytes = serde_json::to_vec(definition).unwrap_or_default();
        let file_size = definition_bytes.len();

        // Get the next version number
        let next_version_row = sqlx::query!(
            r#"
            SELECT COALESCE(MAX(version), 0) + 1 as "next_version!"
            FROM workflow_definitions
            WHERE tenant_id = $1 AND workflow_id = $2
            "#,
            tenant_id,
            workflow_id
        )
        .fetch_one(&self.pool)
        .await?;

        let version_num = next_version_row.next_version;

        // Insert workflow definition
        sqlx::query!(
            r#"
            INSERT INTO workflow_definitions (tenant_id, workflow_id, version, definition, file_size)
            VALUES ($1, $2, $3, $4, $5)
            "#,
            tenant_id,
            workflow_id,
            version_num,
            definition,
            file_size as i32
        )
        .execute(&self.pool)
        .await?;

        Ok(version_num)
    }

    /// Get a specific version of a workflow definition
    pub async fn get_definition(
        &self,
        tenant_id: &str,
        workflow_id: &str,
        version: i32,
    ) -> Result<Option<Value>, sqlx::Error> {
        let row = sqlx::query!(
            r#"
            SELECT definition
            FROM workflow_definitions
            WHERE tenant_id = $1 AND workflow_id = $2 AND version = $3 AND deleted_at IS NULL
            "#,
            tenant_id,
            workflow_id,
            version
        )
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(|r| r.definition))
    }

    /// Get workflow definition and track-events mode.
    pub async fn get_definition_with_track_events(
        &self,
        tenant_id: &str,
        workflow_id: &str,
        version: i32,
    ) -> Result<Option<(Value, bool)>, sqlx::Error> {
        let row: Option<(Value, bool)> = sqlx::query_as(
            r#"
            SELECT definition, track_events
            FROM workflow_definitions
            WHERE tenant_id = $1 AND workflow_id = $2 AND version = $3 AND deleted_at IS NULL
            "#,
        )
        .bind(tenant_id)
        .bind(workflow_id)
        .bind(version)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row)
    }

    /// Get workflow definition, memory tier, and track-events mode.
    pub async fn get_definition_with_memory_tier(
        &self,
        tenant_id: &str,
        workflow_id: &str,
        version: i32,
    ) -> Result<Option<(Value, MemoryTier, bool)>, sqlx::Error> {
        let row: Option<(Value, String, bool)> = sqlx::query_as(
            r#"
            SELECT definition, memory_tier, track_events
            FROM workflow_definitions
            WHERE tenant_id = $1 AND workflow_id = $2 AND version = $3 AND deleted_at IS NULL
            "#,
        )
        .bind(tenant_id)
        .bind(workflow_id)
        .bind(version)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(|r| {
            let memory_tier = MemoryTier::parse(&r.1).unwrap_or_default();
            (r.0, memory_tier, r.2)
        }))
    }

    /// Check if a specific version exists
    pub async fn version_exists(
        &self,
        tenant_id: &str,
        workflow_id: &str,
        version: i32,
    ) -> Result<bool, sqlx::Error> {
        let row = sqlx::query!(
            r#"
            SELECT 1 as "exists!"
            FROM workflow_definitions
            WHERE tenant_id = $1 AND workflow_id = $2 AND version = $3 AND deleted_at IS NULL
            "#,
            tenant_id,
            workflow_id,
            version
        )
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.is_some())
    }

    /// List all versions of a workflow with compilation status
    pub async fn list_versions(
        &self,
        tenant_id: &str,
        workflow_id: &str,
    ) -> Result<Vec<WorkflowVersionInfoDto>, sqlx::Error> {
        let rows: Vec<WorkflowVersionRow> = sqlx::query_as(
            r#"
            SELECT
                sd.version,
                sd.created_at,
                sd.updated_at,
                sd.track_events,
                sc.compiled_at as "compiled_at?",
                s.current_version,
                s.latest_version
            FROM workflow_definitions sd
            LEFT JOIN workflow_compilations sc
                ON sd.tenant_id = sc.tenant_id
                AND sd.workflow_id = sc.workflow_id
                AND sd.version = sc.version
                AND sc.compilation_status = 'success'
            JOIN workflows s
                ON sd.tenant_id = s.tenant_id
                AND sd.workflow_id = s.workflow_id
            WHERE sd.tenant_id = $1
                AND sd.workflow_id = $2
                AND sd.deleted_at IS NULL
            ORDER BY sd.version ASC
            "#,
        )
        .bind(tenant_id)
        .bind(workflow_id)
        .fetch_all(&self.pool)
        .await?;

        let versions = rows
            .iter()
            .map(|row| {
                // Determine current version (current_version takes precedence, fallback to latest_version)
                let current_version = row.5.unwrap_or_else(|| row.6.unwrap_or(0));
                let is_active = row.0 == current_version;

                WorkflowVersionInfoDto {
                    workflow_id: workflow_id.to_string(),
                    version_id: format!("{}-v{}", workflow_id, row.0),
                    version_number: row.0,
                    created_at: row.1.to_rfc3339(),
                    updated_at: row.2.to_rfc3339(),
                    track_events: row.3,
                    is_active,
                    compiled: row.4.is_some(),
                    compiled_at: row.4.map(|t| t.to_rfc3339()),
                }
            })
            .collect();

        Ok(versions)
    }

    // ============================================================================
    // Folder Operations
    // ============================================================================

    /// Update the path (folder) of a workflow
    pub async fn update_path(
        &self,
        tenant_id: &str,
        workflow_id: &str,
        path: &str,
    ) -> Result<(), sqlx::Error> {
        sqlx::query!(
            r#"
            UPDATE workflows
            SET path = $3, updated_at = NOW()
            WHERE tenant_id = $1 AND workflow_id = $2 AND deleted_at IS NULL
            "#,
            tenant_id,
            workflow_id,
            path
        )
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// List all distinct folder paths for a tenant
    /// Returns paths in alphabetical order
    pub async fn list_folders(&self, tenant_id: &str) -> Result<Vec<String>, sqlx::Error> {
        let rows = sqlx::query!(
            r#"
            SELECT DISTINCT path
            FROM workflows
            WHERE tenant_id = $1 AND deleted_at IS NULL
            ORDER BY path
            "#,
            tenant_id
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.into_iter().map(|r| r.path).collect())
    }

    /// Rename a folder by updating all workflow paths that start with the old path
    /// Returns the number of workflows updated
    pub async fn rename_folder(
        &self,
        tenant_id: &str,
        old_path: &str,
        new_path: &str,
    ) -> Result<u64, sqlx::Error> {
        // Update all paths that start with old_path
        // For example: renaming /Sales/ to /Revenue/ will update:
        // - /Sales/ -> /Revenue/
        // - /Sales/Shopify/ -> /Revenue/Shopify/
        let result = sqlx::query!(
            r#"
            UPDATE workflows
            SET path = $3 || SUBSTRING(path FROM LENGTH($2) + 1),
                updated_at = NOW()
            WHERE tenant_id = $1 AND path LIKE $2 || '%' AND deleted_at IS NULL
            "#,
            tenant_id,
            old_path,
            new_path
        )
        .execute(&self.pool)
        .await?;

        Ok(result.rows_affected())
    }

    // ============================================================================
    // Workflow Cloning
    // ============================================================================

    /// Clone a workflow with all its versions to a new workflow ID
    /// The new name is embedded in the cloned execution graphs
    /// Returns the number of versions copied
    pub async fn clone(
        &self,
        tenant_id: &str,
        source_id: &str,
        new_id: &str,
        new_name: &str,
    ) -> Result<usize, sqlx::Error> {
        // Fetch all versions of the source workflow
        let source_versions = sqlx::query!(
            r#"
            SELECT version, definition, file_size
            FROM workflow_definitions
            WHERE tenant_id = $1 AND workflow_id = $2 AND deleted_at IS NULL
            ORDER BY version ASC
            "#,
            tenant_id,
            source_id
        )
        .fetch_all(&self.pool)
        .await?;

        if source_versions.is_empty() {
            return Ok(0);
        }

        // Create the new workflow metadata
        let latest_version = source_versions.last().map(|v| v.version).unwrap_or(0);
        sqlx::query!(
            r#"
            INSERT INTO workflows (tenant_id, workflow_id, version_count, latest_version)
            VALUES ($1, $2, $3, $4)
            "#,
            tenant_id,
            new_id,
            source_versions.len() as i32,
            latest_version
        )
        .execute(&self.pool)
        .await?;

        // Clone all versions, updating the name in each execution graph
        let mut versions_copied = 0;
        for version_data in &source_versions {
            // Update name in the cloned execution graph
            let mut cloned_definition = version_data.definition.clone();
            if let Some(obj) = cloned_definition.as_object_mut() {
                obj.insert("name".to_string(), serde_json::json!(new_name));
            }
            let cloned_bytes = serde_json::to_vec(&cloned_definition).unwrap_or_default();
            let cloned_size = cloned_bytes.len() as i32;

            sqlx::query!(
                r#"
                INSERT INTO workflow_definitions (tenant_id, workflow_id, version, definition, file_size)
                VALUES ($1, $2, $3, $4, $5)
                "#,
                tenant_id,
                new_id,
                version_data.version,
                &cloned_definition,
                cloned_size
            )
            .execute(&self.pool)
            .await?;

            versions_copied += 1;
        }

        Ok(versions_copied)
    }

    // ============================================================================
    // Compilation Operations
    // ============================================================================

    /// Record successful compilation in the database
    ///
    /// Stores binary path, checksum, and metadata in workflow_compilations table.
    /// The binary itself is stored on filesystem, not in the database.
    pub async fn record_compilation_success(
        &self,
        record: CompilationSuccessRecord<'_>,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            r#"
            INSERT INTO workflow_compilations
                (tenant_id, workflow_id, version, compiled_at, translated_path, compilation_status, wasm_size, wasm_checksum, runtara_version, source_checksum)
            VALUES ($1, $2, $3, NOW(), $4, 'success', $5, $6, $7, $8)
            ON CONFLICT (tenant_id, workflow_id, version)
            DO UPDATE SET
                compiled_at = NOW(),
                translated_path = $4,
                compilation_status = 'success',
                error_message = NULL,
                wasm_size = $5,
                wasm_checksum = $6,
                runtara_version = $7,
                source_checksum = $8
            "#,
        )
        .bind(record.tenant_id)
        .bind(record.workflow_id)
        .bind(record.version)
        .bind(record.build_dir.to_string_lossy().to_string())
        .bind(record.binary_size)
        .bind(record.binary_checksum)
        .bind(env!("BUILD_VERSION"))
        .bind(record.source_checksum)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Record the registered image ID from runtara-environment
    ///
    /// Upserts the workflow_compilations table with the image_id returned
    /// from runtara-environment registration. Creates a record if one doesn't exist
    /// (e.g., when recovering an orphaned image from runtara-environment).
    pub async fn record_registered_image_id(
        &self,
        tenant_id: &str,
        workflow_id: &str,
        version: i32,
        image_id: &str,
        source_checksum: Option<&str>,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            r#"
            INSERT INTO workflow_compilations
                (tenant_id, workflow_id, version, compiled_at, translated_path, compilation_status, registered_image_id, runtara_version, source_checksum)
            VALUES ($1, $2, $3, NOW(), '', 'success', $4, $5, $6)
            ON CONFLICT (tenant_id, workflow_id, version)
            DO UPDATE SET
                registered_image_id = $4,
                compilation_status = 'success',
                error_message = NULL,
                runtara_version = $5,
                source_checksum = COALESCE($6, workflow_compilations.source_checksum)
            "#,
        )
        .bind(tenant_id)
        .bind(workflow_id)
        .bind(version)
        .bind(image_id)
        .bind(env!("BUILD_VERSION"))
        .bind(source_checksum)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Get the registered image ID for a compiled workflow
    ///
    /// Returns the UUID image_id that was assigned by runtara-environment
    /// during image registration. This is the ID that must be used for
    /// start_instance calls.
    pub async fn get_registered_image_id(
        &self,
        tenant_id: &str,
        workflow_id: &str,
        version: i32,
    ) -> Result<Option<String>, sqlx::Error> {
        let result = sqlx::query!(
            r#"
            SELECT registered_image_id
            FROM workflow_compilations
            WHERE tenant_id = $1 AND workflow_id = $2 AND version = $3
                AND compilation_status = 'success'
            "#,
            tenant_id,
            workflow_id,
            version
        )
        .fetch_optional(&self.pool)
        .await?;

        Ok(result.and_then(|r| r.registered_image_id))
    }

    pub async fn get_fresh_registered_image_id(
        &self,
        tenant_id: &str,
        workflow_id: &str,
        version: i32,
    ) -> Result<Option<String>, sqlx::Error> {
        let row = sqlx::query(
            r#"
            SELECT sc.registered_image_id, sc.source_checksum, wd.definition
            FROM workflow_definitions wd
            LEFT JOIN workflow_compilations sc
              ON sc.tenant_id = wd.tenant_id
             AND sc.workflow_id = wd.workflow_id
             AND sc.version = wd.version
             AND sc.compilation_status = 'success'
            WHERE wd.tenant_id = $1
              AND wd.workflow_id = $2
              AND wd.version = $3
              AND wd.deleted_at IS NULL
            "#,
        )
        .bind(tenant_id)
        .bind(workflow_id)
        .bind(version)
        .fetch_optional(&self.pool)
        .await?;

        let Some(row) = row else {
            return Ok(None);
        };

        let image_id: Option<String> = row.try_get("registered_image_id")?;
        let stored_checksum: Option<String> = row.try_get("source_checksum")?;
        let definition: Value = row.try_get("definition")?;
        let current_checksum = workflow_definition_checksum(&definition);

        Ok(match (image_id, stored_checksum) {
            (Some(image_id), Some(stored)) if stored == current_checksum => Some(image_id),
            _ => None,
        })
    }

    /// Update workflow by creating a new version
    ///
    /// Creates a new version of the workflow.
    /// Note: name/description are now stored in the execution graph (definition), not in the workflows table.
    /// Returns the new version number.
    pub async fn update_workflow(
        &self,
        tenant_id: &str,
        workflow_id: &str,
        definition: &Value,
        memory_tier: Option<MemoryTier>,
        track_events: Option<bool>,
    ) -> Result<i32, sqlx::Error> {
        let definition_bytes = serde_json::to_vec(definition).unwrap_or_default();
        let file_size = definition_bytes.len() as i32;

        // Get the next version number, current memory tier, and track-events value if not provided
        let current: (i32, Option<String>, Option<bool>) = sqlx::query_as(
            r#"
            SELECT (COALESCE(MAX(version), 0) + 1)::INT as next_version,
                   (SELECT memory_tier FROM workflow_definitions
                    WHERE tenant_id = $1 AND workflow_id = $2
                    ORDER BY version DESC LIMIT 1) as current_memory_tier,
                   (SELECT track_events FROM workflow_definitions
                    WHERE tenant_id = $1 AND workflow_id = $2
                    ORDER BY version DESC LIMIT 1) as current_track_events
            FROM workflow_definitions
            WHERE tenant_id = $1 AND workflow_id = $2
            "#,
        )
        .bind(tenant_id)
        .bind(workflow_id)
        .fetch_one(&self.pool)
        .await?;

        let next_version = current.0;
        // Use provided memory tier or fall back to current tier or default
        let tier = memory_tier
            .or_else(|| {
                current
                    .1
                    .as_ref()
                    .and_then(|tier_str| MemoryTier::parse(tier_str))
            })
            .unwrap_or_default();

        // Use provided track_events or fall back to current value or default (true)
        let track_events_value = track_events.or(current.2).unwrap_or(true);

        // Insert new version
        sqlx::query(
            r#"
            INSERT INTO workflow_definitions (tenant_id, workflow_id, version, definition, file_size, memory_tier, track_events)
            VALUES ($1, $2, $3, $4, $5, $6, $7)
            "#,
        )
        .bind(tenant_id)
        .bind(workflow_id)
        .bind(next_version)
        .bind(definition)
        .bind(file_size)
        .bind(tier as MemoryTier)
        .bind(track_events_value)
        .execute(&self.pool)
        .await?;

        // Update workflows table metadata (only version info, name/description are in execution graph)
        sqlx::query!(
            r#"
            UPDATE workflows
            SET latest_version = $1,
                version_count = version_count + 1,
                updated_at = NOW()
            WHERE tenant_id = $2 AND workflow_id = $3
            "#,
            next_version,
            tenant_id,
            workflow_id
        )
        .execute(&self.pool)
        .await?;

        Ok(next_version)
    }

    /// Update track-events mode for a specific workflow version.
    /// Uses the legacy `track_events` column for compatibility.
    /// Compilation is only invalidated if the value actually changed.
    pub async fn update_track_events(
        &self,
        tenant_id: &str,
        workflow_id: &str,
        version: i32,
        track_events: bool,
    ) -> Result<(), sqlx::Error> {
        // Fetch current value to detect whether it actually changes
        let current_track_events = self
            .get_track_events(tenant_id, workflow_id, version)
            .await?
            .unwrap_or(false);

        // Update track_events column in workflow_definitions
        sqlx::query(
            r#"
            UPDATE workflow_definitions
            SET track_events = $4, updated_at = NOW()
            WHERE tenant_id = $1 AND workflow_id = $2 AND version = $3 AND deleted_at IS NULL
            "#,
        )
        .bind(tenant_id)
        .bind(workflow_id)
        .bind(version)
        .bind(track_events)
        .execute(&self.pool)
        .await?;

        // Only invalidate compilation if track-events mode actually changed.
        // Calling toggle_track_events with the same value (e.g. from UI post-compile refresh)
        // must NOT wipe the freshly created compilation record.
        if current_track_events != track_events {
            self.invalidate_compilation(tenant_id, workflow_id, version)
                .await?;
        }

        Ok(())
    }

    /// Update a version's execution graph in-place (no new version created).
    /// Invalidates any existing compilation for this version.
    /// Returns the number of rows affected (0 if version not found).
    pub async fn update_version_graph(
        &self,
        tenant_id: &str,
        workflow_id: &str,
        version: i32,
        definition: &Value,
    ) -> Result<u64, sqlx::Error> {
        let definition_bytes = serde_json::to_vec(definition).unwrap_or_default();
        let file_size = definition_bytes.len() as i32;

        let result = sqlx::query!(
            r#"
            UPDATE workflow_definitions
            SET definition = $4, file_size = $5, updated_at = NOW()
            WHERE tenant_id = $1 AND workflow_id = $2 AND version = $3 AND deleted_at IS NULL
            "#,
            tenant_id,
            workflow_id,
            version,
            definition,
            file_size
        )
        .execute(&self.pool)
        .await?;

        if result.rows_affected() > 0 {
            // Invalidate compilation since the graph changed
            self.invalidate_compilation(tenant_id, workflow_id, version)
                .await?;
        }

        Ok(result.rows_affected())
    }

    /// Invalidate (delete) the compiled binary for a workflow version
    /// This forces recompilation on next execution
    pub async fn invalidate_compilation(
        &self,
        tenant_id: &str,
        workflow_id: &str,
        version: i32,
    ) -> Result<(), sqlx::Error> {
        sqlx::query!(
            r#"
            DELETE FROM workflow_compilations
            WHERE tenant_id = $1 AND workflow_id = $2 AND version = $3
            "#,
            tenant_id,
            workflow_id,
            version
        )
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Invalidate all compilations for a tenant
    /// This forces recompilation of all workflows on next execution
    /// Useful on server startup when code changes may require recompilation
    pub async fn invalidate_all_compilations(
        pool: &PgPool,
        tenant_id: &str,
    ) -> Result<u64, sqlx::Error> {
        let result = sqlx::query!(
            r#"
            DELETE FROM workflow_compilations
            WHERE tenant_id = $1
            "#,
            tenant_id
        )
        .execute(pool)
        .await?;

        Ok(result.rows_affected())
    }

    /// Get track-events mode for a specific workflow version.
    pub async fn get_track_events(
        &self,
        tenant_id: &str,
        workflow_id: &str,
        version: i32,
    ) -> Result<Option<bool>, sqlx::Error> {
        let row = sqlx::query_scalar::<_, bool>(
            r#"
            SELECT track_events
            FROM workflow_definitions
            WHERE tenant_id = $1 AND workflow_id = $2 AND version = $3 AND deleted_at IS NULL
            "#,
        )
        .bind(tenant_id)
        .bind(workflow_id)
        .bind(version)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row)
    }

    /// Soft delete a workflow and all its versions
    ///
    /// Marks the workflow and all its definitions as deleted.
    /// Returns the number of definitions deleted.
    pub async fn delete_workflow(
        &self,
        tenant_id: &str,
        workflow_id: &str,
    ) -> Result<u64, sqlx::Error> {
        // Delete all definitions
        let definitions_result = sqlx::query!(
            r#"
            UPDATE workflow_definitions
            SET deleted_at = NOW()
            WHERE tenant_id = $1 AND workflow_id = $2 AND deleted_at IS NULL
            "#,
            tenant_id,
            workflow_id
        )
        .execute(&self.pool)
        .await?;

        // Delete workflow metadata
        sqlx::query!(
            r#"
            UPDATE workflows
            SET deleted_at = NOW()
            WHERE tenant_id = $1 AND workflow_id = $2 AND deleted_at IS NULL
            "#,
            tenant_id,
            workflow_id
        )
        .execute(&self.pool)
        .await?;

        Ok(definitions_result.rows_affected())
    }

    /// Clone a workflow to a new workflow ID with a new name
    ///
    /// Copies only the current version (or latest if current not set) to a new workflow.
    /// The cloned workflow starts at version 1.
    /// The new name is embedded in the cloned execution graph.
    /// Returns 1 on success, 0 if source not found.
    pub async fn clone_workflow(
        &self,
        tenant_id: &str,
        source_workflow_id: &str,
        new_workflow_id: &str,
        new_name: &str,
    ) -> Result<i32, sqlx::Error> {
        // Get the current or latest version number
        let version_to_clone = match self
            .get_current_or_latest_version(tenant_id, source_workflow_id)
            .await?
        {
            Some(v) => v,
            None => return Ok(0), // Source workflow not found
        };

        // Fetch the version definition to clone
        let source_version: Option<(Value, i32, String, bool)> = sqlx::query_as(
            r#"
            SELECT definition, file_size, memory_tier, track_events
            FROM workflow_definitions
            WHERE tenant_id = $1 AND workflow_id = $2 AND version = $3 AND deleted_at IS NULL
            "#,
        )
        .bind(tenant_id)
        .bind(source_workflow_id)
        .bind(version_to_clone)
        .fetch_optional(&self.pool)
        .await?;

        let source_version = match source_version {
            Some(v) => v,
            None => return Ok(0), // Version not found
        };

        // Update name in the cloned execution graph
        let mut cloned_definition = source_version.0.clone();
        if let Some(obj) = cloned_definition.as_object_mut() {
            obj.insert("name".to_string(), serde_json::json!(new_name));
        }
        let cloned_bytes = serde_json::to_vec(&cloned_definition).unwrap_or_default();
        let cloned_size = cloned_bytes.len() as i32;

        // Use a transaction to ensure atomicity - all three operations must succeed together
        let mut tx = self.pool.begin().await?;

        // Create new workflow metadata WITHOUT current_version first
        // (FK constraint requires versions to exist before setting current_version)
        // Clone starts at version 1
        sqlx::query!(
            r#"
            INSERT INTO workflows (
                tenant_id, workflow_id,
                latest_version, version_count, created_at, updated_at
            )
            VALUES ($1, $2, 1, 1, NOW(), NOW())
            "#,
            tenant_id,
            new_workflow_id
        )
        .execute(&mut *tx)
        .await?;

        // Clone the current version as version 1 with updated name
        sqlx::query(
            r#"
            INSERT INTO workflow_definitions (
                tenant_id, workflow_id, version, definition, file_size, memory_tier, track_events, created_at
            )
            VALUES ($1, $2, 1, $3, $4, $5, $6, NOW())
            "#,
        )
        .bind(tenant_id)
        .bind(new_workflow_id)
        .bind(&cloned_definition)
        .bind(cloned_size)
        .bind(&source_version.2)
        .bind(source_version.3)
        .execute(&mut *tx)
        .await?;

        // Set current_version to 1 (after version exists due to FK constraint)
        sqlx::query!(
            r#"
            UPDATE workflows
            SET current_version = 1, updated_at = NOW()
            WHERE tenant_id = $1 AND workflow_id = $2
            "#,
            tenant_id,
            new_workflow_id
        )
        .execute(&mut *tx)
        .await?;

        // Commit the transaction
        tx.commit().await?;

        Ok(1)
    }

    /// Set the current version for a workflow
    ///
    /// Updates which version is marked as "current" for execution.
    /// Note: Requires database migration to add current_version column.
    pub async fn set_current_version(
        &self,
        tenant_id: &str,
        workflow_id: &str,
        version_number: i32,
    ) -> Result<(), sqlx::Error> {
        // Verify version exists
        let version_exists = sqlx::query!(
            r#"
            SELECT 1 as "exists!"
            FROM workflow_definitions
            WHERE tenant_id = $1 AND workflow_id = $2 AND version = $3 AND deleted_at IS NULL
            "#,
            tenant_id,
            workflow_id,
            version_number
        )
        .fetch_optional(&self.pool)
        .await?;

        if version_exists.is_none() {
            return Err(sqlx::Error::RowNotFound);
        }

        // Update current version (this will fail if migration hasn't been run)
        sqlx::query!(
            r#"
            UPDATE workflows
            SET current_version = $1,
                updated_at = NOW()
            WHERE tenant_id = $2 AND workflow_id = $3 AND deleted_at IS NULL
            "#,
            version_number,
            tenant_id,
            workflow_id
        )
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    // ============================================================================
    // Workflow Dependency Operations
    // ============================================================================

    /// Record a dependency between parent and child workflows
    #[allow(clippy::too_many_arguments)]
    pub async fn create_dependency(
        &self,
        tenant_id: &str,
        parent_workflow_id: &str,
        parent_version: i32,
        child_workflow_id: &str,
        child_version_requested: &str,
        child_version_resolved: i32,
        step_id: &str,
    ) -> Result<(), sqlx::Error> {
        sqlx::query!(
            r#"
            INSERT INTO workflow_dependencies (
                parent_tenant_id,
                parent_workflow_id,
                parent_version,
                child_workflow_id,
                child_version_requested,
                child_version_resolved,
                step_id
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7)
            ON CONFLICT (parent_tenant_id, parent_workflow_id, parent_version, step_id)
            DO UPDATE SET
                child_workflow_id = $4,
                child_version_requested = $5,
                child_version_resolved = $6,
                created_at = NOW()
            "#,
            tenant_id,
            parent_workflow_id,
            parent_version,
            child_workflow_id,
            child_version_requested,
            child_version_resolved,
            step_id
        )
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Delete all dependencies for a specific parent version
    pub async fn delete_dependencies_for_version(
        &self,
        tenant_id: &str,
        parent_workflow_id: &str,
        parent_version: i32,
    ) -> Result<(), sqlx::Error> {
        sqlx::query!(
            r#"
            DELETE FROM workflow_dependencies
            WHERE parent_tenant_id = $1
                AND parent_workflow_id = $2
                AND parent_version = $3
            "#,
            tenant_id,
            parent_workflow_id,
            parent_version
        )
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Get all dependencies for a parent workflow (all versions or specific version)
    pub async fn get_dependencies(
        &self,
        tenant_id: &str,
        parent_workflow_id: &str,
        parent_version: Option<i32>,
    ) -> Result<Vec<(i32, String, String, i32, String)>, sqlx::Error> {
        let rows = sqlx::query!(
            r#"
            SELECT parent_version, child_workflow_id, child_version_requested, child_version_resolved, step_id
            FROM workflow_dependencies
            WHERE parent_tenant_id = $1
                AND parent_workflow_id = $2
                AND ($3::INTEGER IS NULL OR parent_version = $3)
            ORDER BY CASE WHEN $3 IS NULL THEN parent_version ELSE 0 END DESC, step_id
            "#,
            tenant_id,
            parent_workflow_id,
            parent_version
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|row| {
                (
                    row.parent_version,
                    row.child_workflow_id,
                    row.child_version_requested,
                    row.child_version_resolved,
                    row.step_id,
                )
            })
            .collect())
    }

    /// Get all parent workflows that depend on a child workflow
    pub async fn get_dependents(
        &self,
        tenant_id: &str,
        child_workflow_id: &str,
        child_version: Option<i32>,
    ) -> Result<Vec<(String, i32, i32, String)>, sqlx::Error> {
        let rows = sqlx::query!(
            r#"
            SELECT parent_workflow_id, parent_version, child_version_resolved, step_id
            FROM workflow_dependencies
            WHERE parent_tenant_id = $1
                AND child_workflow_id = $2
                AND ($3::INTEGER IS NULL OR child_version_resolved = $3)
            ORDER BY parent_workflow_id, parent_version DESC
            "#,
            tenant_id,
            child_workflow_id,
            child_version
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|row| {
                (
                    row.parent_workflow_id,
                    row.parent_version,
                    row.child_version_resolved,
                    row.step_id,
                )
            })
            .collect())
    }

    // ============================================================================
    // Workflow Schema Operations
    // ============================================================================

    /// Get schemas and variables from a specific workflow version's execution graph
    ///
    /// Returns (input_schema, output_schema, variables) extracted from the execution_graph JSON.
    pub async fn get_version_schemas(
        &self,
        tenant_id: &str,
        workflow_id: &str,
        version: i32,
    ) -> Result<Option<(Value, Value, Value)>, sqlx::Error> {
        let row: Option<(Value,)> = sqlx::query_as(
            r#"
            SELECT sd.definition
            FROM workflow_definitions sd
            JOIN workflows s ON s.tenant_id = sd.tenant_id AND s.workflow_id = sd.workflow_id
            WHERE sd.tenant_id = $1 AND sd.workflow_id = $2 AND sd.version = $3
            AND sd.deleted_at IS NULL AND s.deleted_at IS NULL
            "#,
        )
        .bind(tenant_id)
        .bind(workflow_id)
        .bind(version)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(|(execution_graph,)| {
            let input_schema = execution_graph
                .get("inputSchema")
                .cloned()
                .unwrap_or(Value::Null);
            let output_schema = execution_graph
                .get("outputSchema")
                .cloned()
                .unwrap_or(Value::Null);
            let variables = execution_graph
                .get("variables")
                .cloned()
                .unwrap_or(Value::Array(vec![]));
            (input_schema, output_schema, variables)
        }))
    }

    /// Get execution timeout for a workflow version from the executionGraph
    pub async fn get_execution_timeout(
        &self,
        tenant_id: &str,
        workflow_id: &str,
        version: i32,
    ) -> Result<Option<i32>, sqlx::Error> {
        let row = sqlx::query!(
            r#"
            SELECT definition
            FROM workflow_definitions
            WHERE tenant_id = $1 AND workflow_id = $2 AND version = $3 AND deleted_at IS NULL
            "#,
            tenant_id,
            workflow_id,
            version
        )
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.and_then(|r| {
            r.definition
                .get("executionTimeoutSeconds")
                .and_then(|v| {
                    v.as_i64()
                        .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
                })
                .map(|v| v as i32)
        }))
    }

    /// Get workflow names for multiple workflow IDs in bulk
    ///
    /// Returns a HashMap mapping workflow_id -> (name, current_version).
    /// This is used to enrich execution listings with workflow metadata.
    /// The name is extracted from the execution graph of the current (or latest) version.
    #[allow(clippy::type_complexity)]
    pub async fn get_workflow_names_bulk(
        &self,
        tenant_id: &str,
        workflow_ids: &[String],
    ) -> Result<std::collections::HashMap<String, (String, i32)>, sqlx::Error> {
        if workflow_ids.is_empty() {
            return Ok(std::collections::HashMap::new());
        }

        // Query workflow names from the execution graph (definition)
        // Join with workflow_definitions to get the name from current/latest version
        let rows: Vec<(String, Option<Value>, Option<i32>, Option<i32>)> = sqlx::query_as(
            r#"
            SELECT s.workflow_id, sd.definition, s.current_version, s.latest_version
            FROM workflows s
            LEFT JOIN workflow_definitions sd ON s.tenant_id = sd.tenant_id
                AND s.workflow_id = sd.workflow_id
                AND COALESCE(s.current_version, s.latest_version) = sd.version
                AND sd.deleted_at IS NULL
            WHERE s.tenant_id = $1 AND s.workflow_id = ANY($2) AND s.deleted_at IS NULL
            "#,
        )
        .bind(tenant_id)
        .bind(workflow_ids)
        .fetch_all(&self.pool)
        .await?;

        let mut result = std::collections::HashMap::new();
        for (workflow_id, definition, current_version, latest_version) in rows {
            let name = definition
                .as_ref()
                .and_then(|d| d.get("name"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let version = current_version.or(latest_version).unwrap_or(0);
            result.insert(workflow_id, (name, version));
        }

        Ok(result)
    }

    /// Look up workflow info by registered image IDs (UUIDs from runtara-environment)
    ///
    /// Returns a HashMap mapping registered_image_id -> (workflow_id, version, name).
    /// This is used to enrich execution listings when we only have the image UUID from Runtara.
    pub async fn get_workflow_info_by_image_ids(
        &self,
        tenant_id: &str,
        image_ids: &[String],
    ) -> Result<std::collections::HashMap<String, (String, i32, String)>, sqlx::Error> {
        if image_ids.is_empty() {
            return Ok(std::collections::HashMap::new());
        }

        // Query workflow info by registered_image_id
        // Join with workflow_definitions to get the workflow name from the definition
        let rows: Vec<(String, String, i32, Option<Value>)> = sqlx::query_as(
            r#"
            SELECT sc.registered_image_id, sc.workflow_id, sc.version, sd.definition
            FROM workflow_compilations sc
            JOIN workflow_definitions sd ON sc.tenant_id = sd.tenant_id
                AND sc.workflow_id = sd.workflow_id
                AND sc.version = sd.version
                AND sd.deleted_at IS NULL
            WHERE sc.tenant_id = $1
                AND sc.registered_image_id = ANY($2)
                AND sc.compilation_status = 'success'
            "#,
        )
        .bind(tenant_id)
        .bind(image_ids)
        .fetch_all(&self.pool)
        .await?;

        let mut result = std::collections::HashMap::new();
        for (image_id, workflow_id, version, definition) in rows {
            let name = definition
                .as_ref()
                .and_then(|d| d.get("name"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            result.insert(image_id, (workflow_id, version, name));
        }

        Ok(result)
    }

    /// Check the compilation readiness state for a workflow version.
    ///
    /// This consolidates the logic that both the async `ExecutionEngine` and the
    /// synchronous execution path used to duplicate. Behaviour:
    /// - If the compilation row is `success` AND has a registered image id, returns
    ///   `CompilationStatus::Ready { translated_path, registered_image_id }`.
    /// - If the row is `failed`, logs the stored error message at `error`, deletes
    ///   the stale record so the next request can retry, and returns
    ///   `CompilationStatus::Failed { error }`.
    /// - Otherwise (no row, `pending`, or `success` without a registered image)
    ///   returns `CompilationStatus::NotReady`.
    pub async fn ensure_compilation_ready(
        &self,
        tenant_id: &str,
        workflow_id: &str,
        version: i32,
    ) -> Result<CompilationStatus, sqlx::Error> {
        let compilation_record = sqlx::query(
            r#"
            SELECT sc.compilation_status,
                   sc.translated_path,
                   sc.registered_image_id,
                   sc.error_message,
                   sc.source_checksum,
                   wd.definition
            FROM workflow_definitions wd
            LEFT JOIN workflow_compilations sc
              ON sc.tenant_id = wd.tenant_id
             AND sc.workflow_id = wd.workflow_id
             AND sc.version = wd.version
            WHERE wd.tenant_id = $1
              AND wd.workflow_id = $2
              AND wd.version = $3
              AND wd.deleted_at IS NULL
            "#,
        )
        .bind(tenant_id)
        .bind(workflow_id)
        .bind(version)
        .fetch_optional(&self.pool)
        .await?;

        match compilation_record {
            Some(record) => {
                let compilation_status: Option<String> = record.try_get("compilation_status")?;
                let translated_path: Option<String> = record.try_get("translated_path")?;
                let registered_image_id: Option<String> = record.try_get("registered_image_id")?;
                let error_message: Option<String> = record.try_get("error_message")?;
                let source_checksum: Option<String> = record.try_get("source_checksum")?;
                let definition: Value = record.try_get("definition")?;
                let current_checksum = workflow_definition_checksum(&definition);

                if compilation_status.as_deref() == Some("success")
                    && registered_image_id.is_some()
                    && source_checksum.as_deref() == Some(current_checksum.as_str())
                {
                    return Ok(CompilationStatus::Ready {
                        translated_path: translated_path.unwrap_or_default(),
                        registered_image_id: registered_image_id.unwrap_or_default(),
                    });
                }

                if compilation_status.as_deref() == Some("failed") {
                    let error_msg =
                        error_message.unwrap_or_else(|| "Unknown compilation error".to_string());
                    // Log at ERROR level so it's visible in logs
                    tracing::error!(
                        tenant_id = %tenant_id,
                        workflow_id = %workflow_id,
                        version = version,
                        compilation_error = %error_msg,
                        "COMPILATION FAILED - deleting record for retry"
                    );
                    // Delete failed record so it can be retried
                    let _ = sqlx::query(
                        "DELETE FROM workflow_compilations WHERE tenant_id = $1 AND workflow_id = $2 AND version = $3",
                    )
                    .bind(tenant_id)
                    .bind(workflow_id)
                    .bind(version)
                    .execute(&self.pool)
                    .await;
                    return Ok(CompilationStatus::Failed { error: error_msg });
                }

                Ok(CompilationStatus::NotReady)
            }
            _ => Ok(CompilationStatus::NotReady),
        }
    }
}

/// Compilation readiness status returned by
/// [`WorkflowRepository::ensure_compilation_ready`].
#[derive(Debug, Clone)]
pub enum CompilationStatus {
    /// Compilation succeeded and is registered with runtara-environment.
    Ready {
        /// Filesystem path of the translated workflow build directory.
        translated_path: String,
        /// Image id registered in runtara-environment.
        registered_image_id: String,
    },
    /// Previous compilation attempt recorded a failure. The stale record has
    /// been deleted so the caller may queue a retry.
    Failed { error: String },
    /// No successful compilation is available yet (no record, pending, or
    /// partially recorded).
    NotReady,
}
