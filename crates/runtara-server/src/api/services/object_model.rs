//! Object Model Service
//!
//! Business logic for schema and instance management.
//! Wraps repositories and provides the API expected by handlers.

use crate::api::dto::object_model::*;
use crate::api::repositories::object_model::ObjectStoreManager;
use runtara_connections::ConnectionsFacade;
use runtara_object_store::ObjectStore;
use std::sync::Arc;

// ============================================================================
// Connection Resolution Helper
// ============================================================================

/// Resolves a connection_id to a database URL, or returns None for default database.
pub(crate) async fn resolve_database_url(
    facade: Option<&ConnectionsFacade>,
    connection_id: Option<&str>,
    tenant_id: &str,
) -> Result<Option<String>, ServiceError> {
    match connection_id {
        Some(conn_id) => {
            let facade = facade.ok_or_else(|| {
                ServiceError::ValidationError(
                    "ConnectionsFacade required when connection_id is provided".to_string(),
                )
            })?;
            let conn = facade
                .get_with_parameters(conn_id, tenant_id)
                .await
                .map_err(|e| {
                    ServiceError::NotFound(format!(
                        "Connection '{}' lookup failed: {:?}",
                        conn_id, e
                    ))
                })?
                .ok_or_else(|| {
                    ServiceError::NotFound(format!("Connection '{}' not found", conn_id))
                })?;

            // Verify connection type
            let integration_id = conn.integration_id.as_deref().unwrap_or("");
            if integration_id != "postgres" {
                return Err(ServiceError::ValidationError(format!(
                    "Connection '{}' has type '{}', expected 'postgres'",
                    conn_id, integration_id
                )));
            }

            let params = conn.connection_parameters.as_ref().ok_or_else(|| {
                ServiceError::ValidationError(format!("Connection '{}' has no parameters", conn_id))
            })?;

            let db_url = params
                .get("database_url")
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    ServiceError::ValidationError(format!(
                        "Connection '{}' missing 'database_url' parameter",
                        conn_id
                    ))
                })?;

            Ok(Some(db_url.to_string()))
        }
        None => Ok(None), // Use default database
    }
}

/// Gets the appropriate ObjectStore based on connection_id or default.
pub(crate) async fn get_store(
    manager: &ObjectStoreManager,
    facade: Option<&ConnectionsFacade>,
    connection_id: Option<&str>,
    tenant_id: &str,
) -> Result<Arc<ObjectStore>, ServiceError> {
    let database_url = resolve_database_url(facade, connection_id, tenant_id).await?;

    match database_url {
        Some(url) => manager.get_store_by_url(&url).await.map_err(|e| {
            ServiceError::DatabaseError(format!("Failed to connect to database: {}", e))
        }),
        None => manager.get_store(tenant_id).await.map_err(|e| {
            ServiceError::DatabaseError(format!("Failed to get default store: {}", e))
        }),
    }
}

// ============================================================================
// Schema Service
// ============================================================================

pub struct SchemaService {
    manager: Arc<ObjectStoreManager>,
    facade: Arc<ConnectionsFacade>,
}

impl SchemaService {
    pub fn new(manager: Arc<ObjectStoreManager>, facade: Arc<ConnectionsFacade>) -> Self {
        Self { manager, facade }
    }

    /// Create a new schema
    pub async fn create_schema(
        &self,
        request: CreateSchemaRequest,
        tenant_id: &str,
        connection_id: Option<&str>,
    ) -> Result<String, ServiceError> {
        // Validation: validate table name and columns using SchemaValidator
        use crate::api::services::schema_validator::SchemaValidator;

        SchemaValidator::validate_schema(&request.table_name, &request.columns, &request.indexes)
            .map_err(|e| ServiceError::ValidationError(e.to_string()))?;

        let store = get_store(&self.manager, Some(&self.facade), connection_id, tenant_id).await?;

        let store_request = runtara_object_store::CreateSchemaRequest {
            name: request.name.clone(),
            description: request.description.clone(),
            table_name: request.table_name.clone(),
            columns: request.columns.iter().map(|c| c.clone().into()).collect(),
            indexes: request
                .indexes
                .as_ref()
                .map(|idxs| idxs.iter().map(|i| i.clone().into()).collect()),
        };

        let store_schema = store.create_schema(store_request).await.map_err(|e| {
            if e.to_string().contains("already exists") || e.to_string().contains("duplicate") {
                ServiceError::Conflict(e.to_string())
            } else {
                ServiceError::DatabaseError(e.to_string())
            }
        })?;

        Ok(store_schema.id)
    }

    /// List schemas with pagination
    pub async fn list_schemas(
        &self,
        tenant_id: &str,
        offset: i64,
        limit: i64,
        connection_id: Option<&str>,
    ) -> Result<(Vec<Schema>, i64), ServiceError> {
        let store = get_store(&self.manager, Some(&self.facade), connection_id, tenant_id).await?;

        let all_schemas = store
            .list_schemas()
            .await
            .map_err(|e| ServiceError::DatabaseError(e.to_string()))?;

        let total_count = all_schemas.len() as i64;

        let schemas: Vec<Schema> = all_schemas
            .into_iter()
            .skip(offset as usize)
            .take(limit as usize)
            .map(|s| Schema::from_store(s, tenant_id.to_string()))
            .collect();

        Ok((schemas, total_count))
    }

    /// Get schema by ID
    pub async fn get_schema_by_id(
        &self,
        id: &str,
        tenant_id: &str,
        connection_id: Option<&str>,
    ) -> Result<Schema, ServiceError> {
        let store = get_store(&self.manager, Some(&self.facade), connection_id, tenant_id).await?;

        let store_schema = store
            .get_schema_by_id(id)
            .await
            .map_err(|e| ServiceError::DatabaseError(e.to_string()))?;

        store_schema
            .map(|s| Schema::from_store(s, tenant_id.to_string()))
            .ok_or_else(|| ServiceError::NotFound("Schema not found".to_string()))
    }

    /// Get schema by name
    pub async fn get_schema_by_name(
        &self,
        name: &str,
        tenant_id: &str,
        connection_id: Option<&str>,
    ) -> Result<Schema, ServiceError> {
        let store = get_store(&self.manager, Some(&self.facade), connection_id, tenant_id).await?;

        let store_schema = store
            .get_schema(name)
            .await
            .map_err(|e| ServiceError::DatabaseError(e.to_string()))?;

        store_schema
            .map(|s| Schema::from_store(s, tenant_id.to_string()))
            .ok_or_else(|| ServiceError::NotFound("Schema not found".to_string()))
    }

    /// Update schema
    pub async fn update_schema(
        &self,
        id: &str,
        tenant_id: &str,
        request: UpdateSchemaRequest,
        connection_id: Option<&str>,
    ) -> Result<(), ServiceError> {
        // Validation: if columns are provided, validate them
        if let Some(ref columns) = request.columns {
            use std::collections::HashSet;
            let mut seen_names = HashSet::new();
            for col in columns {
                if !seen_names.insert(&col.name) {
                    return Err(ServiceError::ValidationError(format!(
                        "Duplicate column name: {}",
                        col.name
                    )));
                }
                // Validate enum values if applicable
                if let crate::api::dto::object_model::ColumnType::Enum { values } = &col.column_type
                    && values.is_empty()
                {
                    return Err(ServiceError::ValidationError(
                        "Enum type must have at least one value".to_string(),
                    ));
                }
            }
        }

        let store = get_store(&self.manager, Some(&self.facade), connection_id, tenant_id).await?;

        // First get the schema to find its name (ObjectStore.update_schema uses name)
        let existing = store
            .get_schema_by_id(id)
            .await
            .map_err(|e| ServiceError::DatabaseError(e.to_string()))?
            .ok_or_else(|| ServiceError::NotFound("Schema not found".to_string()))?;

        let store_request = runtara_object_store::UpdateSchemaRequest {
            name: request.name.clone(),
            description: request.description.clone(),
            columns: request
                .columns
                .as_ref()
                .map(|cols| cols.iter().map(|c| c.clone().into()).collect()),
            indexes: request
                .indexes
                .as_ref()
                .map(|idxs| idxs.iter().map(|i| i.clone().into()).collect()),
        };

        store
            .update_schema(&existing.name, store_request)
            .await
            .map_err(|e| {
                if e.to_string().contains("not found") {
                    ServiceError::NotFound(e.to_string())
                } else if e.to_string().contains("already exists")
                    || e.to_string().contains("duplicate")
                {
                    ServiceError::Conflict(e.to_string())
                } else {
                    ServiceError::DatabaseError(e.to_string())
                }
            })?;

        Ok(())
    }

    /// Delete schema
    pub async fn delete_schema(
        &self,
        id: &str,
        tenant_id: &str,
        connection_id: Option<&str>,
    ) -> Result<(), ServiceError> {
        let store = get_store(&self.manager, Some(&self.facade), connection_id, tenant_id).await?;

        // First get the schema to find its name
        let existing = store
            .get_schema_by_id(id)
            .await
            .map_err(|e| ServiceError::DatabaseError(e.to_string()))?
            .ok_or_else(|| ServiceError::NotFound("Schema not found".to_string()))?;

        store.delete_schema(&existing.name).await.map_err(|e| {
            if e.to_string().contains("not found") {
                ServiceError::NotFound(e.to_string())
            } else {
                ServiceError::DatabaseError(e.to_string())
            }
        })
    }
}

// ============================================================================
// Instance Service
// ============================================================================

pub struct InstanceService {
    manager: Arc<ObjectStoreManager>,
    facade: Arc<ConnectionsFacade>,
}

impl InstanceService {
    pub fn new(manager: Arc<ObjectStoreManager>, facade: Arc<ConnectionsFacade>) -> Self {
        Self { manager, facade }
    }

    /// Create a new instance
    pub async fn create_instance(
        &self,
        request: CreateInstanceRequest,
        tenant_id: &str,
        connection_id: Option<&str>,
    ) -> Result<String, ServiceError> {
        // Validation: properties should be a valid JSON object
        if !request.properties.is_object() {
            return Err(ServiceError::ValidationError(
                "properties must be a JSON object".to_string(),
            ));
        }

        let store = get_store(&self.manager, Some(&self.facade), connection_id, tenant_id).await?;

        // Lookup schema by ID or name to get the schema name
        let schema_name = match (&request.schema_id, &request.schema_name) {
            (Some(id), _) => {
                // Prefer schema_id if provided
                let schema = store
                    .get_schema_by_id(id)
                    .await
                    .map_err(|e| ServiceError::DatabaseError(e.to_string()))?
                    .ok_or_else(|| {
                        ServiceError::NotFound(format!("Schema with ID '{}' not found", id))
                    })?;
                schema.name
            }
            (None, Some(name)) => {
                // Verify schema exists
                store
                    .get_schema(name)
                    .await
                    .map_err(|e| ServiceError::DatabaseError(e.to_string()))?
                    .ok_or_else(|| {
                        ServiceError::NotFound(format!("Schema '{}' not found", name))
                    })?;
                name.clone()
            }
            (None, None) => {
                return Err(ServiceError::ValidationError(
                    "Either schemaId or schemaName must be provided".to_string(),
                ));
            }
        };

        // Create instance in schema's table
        let instance_id = store
            .create_instance(&schema_name, request.properties.clone())
            .await
            .map_err(|e| {
                if e.to_string().contains("validation") || e.to_string().contains("type") {
                    ServiceError::ValidationError(e.to_string())
                } else {
                    ServiceError::DatabaseError(e.to_string())
                }
            })?;

        Ok(instance_id)
    }

    /// Get instances by schema ID
    pub async fn get_instances_by_schema(
        &self,
        schema_id: &str,
        tenant_id: &str,
        offset: i64,
        limit: i64,
        connection_id: Option<&str>,
    ) -> Result<(Vec<Instance>, i64), ServiceError> {
        let store = get_store(&self.manager, Some(&self.facade), connection_id, tenant_id).await?;

        // Lookup schema by ID
        let schema = store
            .get_schema_by_id(schema_id)
            .await
            .map_err(|e| ServiceError::DatabaseError(e.to_string()))?
            .ok_or_else(|| ServiceError::NotFound("Schema not found".to_string()))?;

        let filter = runtara_object_store::SimpleFilter::new(&schema.name)
            .with_offset(offset as i32)
            .with_limit(limit as i32);

        let (store_instances, total) = store
            .query_instances(filter)
            .await
            .map_err(|e| ServiceError::DatabaseError(e.to_string()))?;

        let instances: Vec<Instance> = store_instances
            .into_iter()
            .map(|i| Instance::from_store(i, tenant_id.to_string()))
            .collect();

        Ok((instances, total))
    }

    /// Get instances by schema name
    pub async fn get_instances_by_schema_name(
        &self,
        schema_name: &str,
        tenant_id: &str,
        offset: i64,
        limit: i64,
        connection_id: Option<&str>,
    ) -> Result<(Vec<Instance>, i64), ServiceError> {
        let store = get_store(&self.manager, Some(&self.facade), connection_id, tenant_id).await?;

        let filter = runtara_object_store::SimpleFilter::new(schema_name)
            .with_offset(offset as i32)
            .with_limit(limit as i32);

        let (store_instances, total) = store
            .query_instances(filter)
            .await
            .map_err(|e| ServiceError::DatabaseError(e.to_string()))?;

        let instances: Vec<Instance> = store_instances
            .into_iter()
            .map(|i| Instance::from_store(i, tenant_id.to_string()))
            .collect();

        Ok((instances, total))
    }

    /// Filter instances with condition for a specific schema
    pub async fn filter_instances_by_schema(
        &self,
        tenant_id: &str,
        schema_name: &str,
        filter_request: FilterRequest,
        connection_id: Option<&str>,
    ) -> Result<(Vec<Instance>, i64), ServiceError> {
        let store = get_store(&self.manager, Some(&self.facade), connection_id, tenant_id).await?;

        let store_filter = runtara_object_store::FilterRequest {
            condition: filter_request.condition.map(|c| c.into()),
            offset: filter_request.offset,
            limit: filter_request.limit,
            sort_by: filter_request.sort_by,
            sort_order: filter_request.sort_order,
        };

        let (store_instances, total) = store
            .filter_instances(schema_name, store_filter)
            .await
            .map_err(|e| {
                if e.to_string().contains("validation") || e.to_string().contains("Invalid") {
                    ServiceError::ValidationError(format!("Invalid condition: {}", e))
                } else if e.to_string().contains("not found") {
                    ServiceError::NotFound(e.to_string())
                } else {
                    ServiceError::DatabaseError(e.to_string())
                }
            })?;

        let instances: Vec<Instance> = store_instances
            .into_iter()
            .map(|i| Instance::from_store(i, tenant_id.to_string()))
            .collect();

        Ok((instances, total))
    }

    /// Get a single instance by ID
    pub async fn get_instance_by_id(
        &self,
        instance_id: &str,
        schema_id: &str,
        tenant_id: &str,
        connection_id: Option<&str>,
    ) -> Result<Option<Instance>, ServiceError> {
        let store = get_store(&self.manager, Some(&self.facade), connection_id, tenant_id).await?;

        // Fetch schema first to get its name
        let schema = store
            .get_schema_by_id(schema_id)
            .await
            .map_err(|e| ServiceError::DatabaseError(e.to_string()))?
            .ok_or_else(|| ServiceError::NotFound("Schema not found".to_string()))?;

        let store_instance = store
            .get_instance(&schema.name, instance_id)
            .await
            .map_err(|e| ServiceError::DatabaseError(e.to_string()))?;

        Ok(store_instance.map(|i| Instance::from_store(i, tenant_id.to_string())))
    }

    /// Update an existing instance
    pub async fn update_instance(
        &self,
        instance_id: &str,
        schema_id: &str,
        tenant_id: &str,
        properties: serde_json::Value,
        connection_id: Option<&str>,
    ) -> Result<(), ServiceError> {
        let store = get_store(&self.manager, Some(&self.facade), connection_id, tenant_id).await?;

        // Fetch schema first to get its name
        let schema = store
            .get_schema_by_id(schema_id)
            .await
            .map_err(|e| ServiceError::DatabaseError(e.to_string()))?
            .ok_or_else(|| ServiceError::NotFound("Schema not found".to_string()))?;

        store
            .update_instance(&schema.name, instance_id, properties)
            .await
            .map_err(|e| {
                if e.to_string().contains("not found") {
                    ServiceError::NotFound(e.to_string())
                } else if e.to_string().contains("validation") || e.to_string().contains("type") {
                    ServiceError::ValidationError(e.to_string())
                } else {
                    ServiceError::DatabaseError(e.to_string())
                }
            })
    }

    /// Delete an instance (soft delete)
    pub async fn delete_instance(
        &self,
        instance_id: &str,
        schema_id: &str,
        tenant_id: &str,
        connection_id: Option<&str>,
    ) -> Result<(), ServiceError> {
        let store = get_store(&self.manager, Some(&self.facade), connection_id, tenant_id).await?;

        // Fetch schema first to get its name
        let schema = store
            .get_schema_by_id(schema_id)
            .await
            .map_err(|e| ServiceError::DatabaseError(e.to_string()))?
            .ok_or_else(|| ServiceError::NotFound("Schema not found".to_string()))?;

        store
            .delete_instance(&schema.name, instance_id)
            .await
            .map_err(|e| {
                if e.to_string().contains("not found") {
                    ServiceError::NotFound(e.to_string())
                } else {
                    ServiceError::DatabaseError(e.to_string())
                }
            })
    }

    /// Bulk delete instances by ID (soft delete). Delegates to the store's
    /// transactional `delete_instances(condition)` — all deletes happen in one
    /// transaction and are rolled back on failure.
    pub async fn bulk_delete_instances(
        &self,
        instance_ids: Vec<String>,
        schema_id: &str,
        tenant_id: &str,
        connection_id: Option<&str>,
    ) -> Result<usize, ServiceError> {
        if instance_ids.is_empty() {
            return Ok(0);
        }

        let store = get_store(&self.manager, Some(&self.facade), connection_id, tenant_id).await?;

        let limit = store.config().bulk_request_limit;
        if instance_ids.len() > limit {
            return Err(ServiceError::ValidationError(format!(
                "bulk request size {} exceeds limit of {}",
                instance_ids.len(),
                limit
            )));
        }

        let schema = store
            .get_schema_by_id(schema_id)
            .await
            .map_err(|e| ServiceError::DatabaseError(e.to_string()))?
            .ok_or_else(|| ServiceError::NotFound("Schema not found".to_string()))?;

        let id_values: Vec<serde_json::Value> = instance_ids
            .into_iter()
            .map(serde_json::Value::String)
            .collect();
        let condition = runtara_object_store::Condition::r#in("id", id_values);

        let deleted = store
            .delete_instances(&schema.name, condition)
            .await
            .map_err(|e| ServiceError::DatabaseError(e.to_string()))?;

        Ok(deleted as usize)
    }

    /// Bulk create multiple instances in one transaction, with opt-in
    /// conflict and validation handling (see `BulkCreateRequest`).
    pub async fn bulk_create_instances(
        &self,
        schema_id: &str,
        request: BulkCreateRequest,
        tenant_id: &str,
        connection_id: Option<&str>,
    ) -> Result<runtara_object_store::BulkCreateResult, ServiceError> {
        let store = get_store(&self.manager, Some(&self.facade), connection_id, tenant_id).await?;

        let schema = store
            .get_schema_by_id(schema_id)
            .await
            .map_err(|e| ServiceError::DatabaseError(e.to_string()))?
            .ok_or_else(|| ServiceError::NotFound("Schema not found".to_string()))?;

        let opts = bulk_create_options_from_request(&request)?;
        let instances = normalize_bulk_create_instances(&request, &schema)?;

        store
            .create_instances_extended(&schema.name, instances, opts)
            .await
            .map_err(|e| {
                if e.to_string().contains("validation") || e.to_string().contains("Invalid") {
                    ServiceError::ValidationError(e.to_string())
                } else {
                    ServiceError::DatabaseError(e.to_string())
                }
            })
    }

    /// Bulk update all rows matching `condition` with the same `properties`.
    pub async fn bulk_update_instances_by_condition(
        &self,
        schema_id: &str,
        properties: serde_json::Value,
        condition: Condition,
        tenant_id: &str,
        connection_id: Option<&str>,
    ) -> Result<i64, ServiceError> {
        let store = get_store(&self.manager, Some(&self.facade), connection_id, tenant_id).await?;

        let schema = store
            .get_schema_by_id(schema_id)
            .await
            .map_err(|e| ServiceError::DatabaseError(e.to_string()))?
            .ok_or_else(|| ServiceError::NotFound("Schema not found".to_string()))?;

        store
            .update_instances(&schema.name, properties, condition.into())
            .await
            .map_err(|e| {
                if e.to_string().contains("validation") || e.to_string().contains("Invalid") {
                    ServiceError::ValidationError(e.to_string())
                } else {
                    ServiceError::DatabaseError(e.to_string())
                }
            })
    }

    /// Bulk update by ID list, each entry with its own `properties`.
    pub async fn bulk_update_instances_by_ids(
        &self,
        schema_id: &str,
        updates: Vec<BulkUpdateByIdEntry>,
        tenant_id: &str,
        connection_id: Option<&str>,
    ) -> Result<i64, ServiceError> {
        let store = get_store(&self.manager, Some(&self.facade), connection_id, tenant_id).await?;

        let schema = store
            .get_schema_by_id(schema_id)
            .await
            .map_err(|e| ServiceError::DatabaseError(e.to_string()))?
            .ok_or_else(|| ServiceError::NotFound("Schema not found".to_string()))?;

        let store_updates: Vec<(String, serde_json::Value)> =
            updates.into_iter().map(|u| (u.id, u.properties)).collect();

        store
            .update_instances_by_ids(&schema.name, store_updates)
            .await
            .map_err(|e| {
                if e.to_string().contains("validation") || e.to_string().contains("Invalid") {
                    ServiceError::ValidationError(e.to_string())
                } else {
                    ServiceError::DatabaseError(e.to_string())
                }
            })
    }
}

// ============================================================================
// Service Errors
// ============================================================================

/// Normalize the two supported bulk-create shapes (object form vs columnar
/// form) into a single `Vec<Value>` of record objects that the store accepts.
///
/// Object form passes through unchanged. Columnar form pairs each row with
/// `columns` (length-matched), merges `constants` as defaults, and — if
/// `nullifyEmptyStrings` is set — replaces `""` in non-string columns with
/// `null` before type validation runs.
///
/// Rules:
/// - Exactly one of `instances` or (`columns` + `rows`) must be provided.
/// - Row length must equal `columns.len()`.
/// - Row values take precedence over `constants` on key overlap.
pub(crate) fn normalize_bulk_create_instances(
    request: &BulkCreateRequest,
    schema: &runtara_object_store::Schema,
) -> Result<Vec<serde_json::Value>, ServiceError> {
    normalize_bulk_create_inputs(
        request.instances.as_deref(),
        request.columns.as_deref(),
        request.rows.as_deref(),
        &request.constants,
        request.nullify_empty_strings,
        schema,
    )
}

/// Lower-level normalizer used by both the public service path (via
/// [`normalize_bulk_create_instances`]) and the internal-handler path (which
/// doesn't construct a `BulkCreateRequest`).
pub(crate) fn normalize_bulk_create_inputs(
    instances: Option<&[serde_json::Value]>,
    columns: Option<&[String]>,
    rows: Option<&[Vec<serde_json::Value>]>,
    constants: &serde_json::Map<String, serde_json::Value>,
    nullify_empty_strings: bool,
    schema: &runtara_object_store::Schema,
) -> Result<Vec<serde_json::Value>, ServiceError> {
    match (instances, columns, rows) {
        (Some(inst), None, None) => Ok(inst.to_vec()),

        (None, Some(cols), Some(rows)) => {
            build_columnar_instances(cols, rows, constants, nullify_empty_strings, schema)
        }

        (None, Some(_), None) | (None, None, Some(_)) => Err(ServiceError::ValidationError(
            "columnar form requires both `columns` and `rows`".to_string(),
        )),

        (Some(_), Some(_), _) | (Some(_), _, Some(_)) => Err(ServiceError::ValidationError(
            "provide either `instances` or `columns` + `rows`, not both".to_string(),
        )),

        (None, None, None) => Err(ServiceError::ValidationError(
            "must provide either `instances` or `columns` + `rows`".to_string(),
        )),
    }
}

fn build_columnar_instances(
    columns: &[String],
    rows: &[Vec<serde_json::Value>],
    constants: &serde_json::Map<String, serde_json::Value>,
    nullify_empty_strings: bool,
    schema: &runtara_object_store::Schema,
) -> Result<Vec<serde_json::Value>, ServiceError> {
    // Pre-compute which columns should nullify empty strings (non-string,
    // non-enum columns). Only populated when the flag is on.
    let nullify_cols: std::collections::HashSet<&str> = if nullify_empty_strings {
        schema
            .columns
            .iter()
            .filter(|c| {
                !matches!(
                    c.column_type,
                    runtara_object_store::ColumnType::String
                        | runtara_object_store::ColumnType::Enum { .. }
                )
            })
            .map(|c| c.name.as_str())
            .collect()
    } else {
        std::collections::HashSet::new()
    };

    let mut result = Vec::with_capacity(rows.len());
    for (idx, row) in rows.iter().enumerate() {
        if row.len() != columns.len() {
            return Err(ServiceError::ValidationError(format!(
                "row {} has {} cells, expected {} to match `columns`",
                idx,
                row.len(),
                columns.len()
            )));
        }
        let mut obj = constants.clone();
        for (col, val) in columns.iter().zip(row.iter()) {
            let val = if nullify_cols.contains(col.as_str()) {
                match val {
                    serde_json::Value::String(s) if s.is_empty() => serde_json::Value::Null,
                    other => other.clone(),
                }
            } else {
                val.clone()
            };
            obj.insert(col.clone(), val);
        }
        result.push(serde_json::Value::Object(obj));
    }
    Ok(result)
}

/// Build store-side bulk-create options from the DTO request, validating
/// that `conflict_columns` is provided whenever `onConflict` is `skip` or
/// `upsert` (the user explicitly chose this over silently defaulting to `id`).
fn bulk_create_options_from_request(
    request: &BulkCreateRequest,
) -> Result<runtara_object_store::BulkCreateOptions, ServiceError> {
    use runtara_object_store::{
        BulkCreateOptions, ConflictMode as StoreConflictMode, ValidationMode,
    };

    let conflict_mode = match request.on_conflict {
        BulkConflictMode::Error => StoreConflictMode::Error,
        BulkConflictMode::Skip => {
            if request.conflict_columns.is_empty() {
                return Err(ServiceError::ValidationError(
                    "`conflictColumns` is required when `onConflict` is 'skip'".to_string(),
                ));
            }
            StoreConflictMode::Skip {
                conflict_columns: request.conflict_columns.clone(),
            }
        }
        BulkConflictMode::Upsert => {
            if request.conflict_columns.is_empty() {
                return Err(ServiceError::ValidationError(
                    "`conflictColumns` is required when `onConflict` is 'upsert'".to_string(),
                ));
            }
            StoreConflictMode::Upsert {
                conflict_columns: request.conflict_columns.clone(),
            }
        }
    };

    let validation_mode = match request.on_error {
        BulkValidationMode::Stop => ValidationMode::Stop,
        BulkValidationMode::Skip => ValidationMode::Skip,
    };

    Ok(BulkCreateOptions {
        conflict_mode,
        validation_mode,
    })
}

#[derive(Debug)]
#[allow(dead_code)]
pub enum ServiceError {
    ValidationError(String),
    NotFound(String),
    Conflict(String),
    DatabaseError(String),
}

impl std::fmt::Display for ServiceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ServiceError::ValidationError(msg) => write!(f, "Validation error: {}", msg),
            ServiceError::NotFound(msg) => write!(f, "Not found: {}", msg),
            ServiceError::Conflict(msg) => write!(f, "Conflict: {}", msg),
            ServiceError::DatabaseError(msg) => write!(f, "Database error: {}", msg),
        }
    }
}

impl std::error::Error for ServiceError {}

#[cfg(test)]
mod normalize_tests {
    use super::*;
    use runtara_object_store::{ColumnDefinition, ColumnType, Schema};

    fn schema_with(cols: Vec<ColumnDefinition>) -> Schema {
        Schema::new("sid", "Test", "test_table", cols)
    }

    fn cd(name: &str, t: ColumnType) -> ColumnDefinition {
        ColumnDefinition::new(name, t)
    }

    #[test]
    fn object_form_passes_through() {
        let schema = schema_with(vec![cd("sku", ColumnType::String)]);
        let instances = vec![
            serde_json::json!({"sku": "A"}),
            serde_json::json!({"sku": "B"}),
        ];
        let out = normalize_bulk_create_inputs(
            Some(&instances),
            None,
            None,
            &serde_json::Map::new(),
            false,
            &schema,
        )
        .unwrap();
        assert_eq!(out, instances);
    }

    #[test]
    fn columnar_form_pairs_with_constants() {
        let schema = schema_with(vec![
            cd("sku", ColumnType::String),
            cd("qty", ColumnType::Integer),
            cd("snapshot_date", ColumnType::String),
        ]);
        let cols = vec!["sku".to_string(), "qty".to_string()];
        let rows = vec![
            vec![serde_json::json!("A"), serde_json::json!(1)],
            vec![serde_json::json!("B"), serde_json::json!(2)],
        ];
        let mut constants = serde_json::Map::new();
        constants.insert("snapshot_date".into(), serde_json::json!("2026-04-18"));

        let out = normalize_bulk_create_inputs(
            None,
            Some(&cols),
            Some(&rows),
            &constants,
            false,
            &schema,
        )
        .unwrap();

        assert_eq!(out.len(), 2);
        assert_eq!(out[0]["sku"], serde_json::json!("A"));
        assert_eq!(out[0]["qty"], serde_json::json!(1));
        assert_eq!(out[0]["snapshot_date"], serde_json::json!("2026-04-18"));
    }

    #[test]
    fn row_cells_win_over_constants_on_overlap() {
        let schema = schema_with(vec![cd("sku", ColumnType::String)]);
        let cols = vec!["sku".to_string()];
        let rows = vec![vec![serde_json::json!("row-value")]];
        let mut constants = serde_json::Map::new();
        constants.insert("sku".into(), serde_json::json!("constant-value"));

        let out = normalize_bulk_create_inputs(
            None,
            Some(&cols),
            Some(&rows),
            &constants,
            false,
            &schema,
        )
        .unwrap();

        assert_eq!(out[0]["sku"], serde_json::json!("row-value"));
    }

    #[test]
    fn nullify_empty_strings_only_hits_non_string_columns() {
        let schema = schema_with(vec![
            cd("name", ColumnType::String),
            cd("qty", ColumnType::Integer),
            cd("when", ColumnType::Timestamp),
        ]);
        let cols = vec!["name".to_string(), "qty".to_string(), "when".to_string()];
        let rows = vec![vec![
            serde_json::json!(""),
            serde_json::json!(""),
            serde_json::json!(""),
        ]];

        let out = normalize_bulk_create_inputs(
            None,
            Some(&cols),
            Some(&rows),
            &serde_json::Map::new(),
            true, // nullify on
            &schema,
        )
        .unwrap();

        // String column keeps its empty string; Integer + Timestamp become null.
        assert_eq!(out[0]["name"], serde_json::json!(""));
        assert!(out[0]["qty"].is_null());
        assert!(out[0]["when"].is_null());
    }

    #[test]
    fn nullify_off_leaves_empty_strings_alone() {
        let schema = schema_with(vec![cd("qty", ColumnType::Integer)]);
        let cols = vec!["qty".to_string()];
        let rows = vec![vec![serde_json::json!("")]];
        let out = normalize_bulk_create_inputs(
            None,
            Some(&cols),
            Some(&rows),
            &serde_json::Map::new(),
            false,
            &schema,
        )
        .unwrap();
        assert_eq!(out[0]["qty"], serde_json::json!(""));
    }

    #[test]
    fn mismatched_row_length_errors() {
        let schema = schema_with(vec![
            cd("a", ColumnType::String),
            cd("b", ColumnType::String),
        ]);
        let cols = vec!["a".to_string(), "b".to_string()];
        let rows = vec![vec![serde_json::json!("x")]]; // 1 cell, expected 2
        let err = normalize_bulk_create_inputs(
            None,
            Some(&cols),
            Some(&rows),
            &serde_json::Map::new(),
            false,
            &schema,
        )
        .unwrap_err();
        match err {
            ServiceError::ValidationError(msg) => {
                assert!(msg.contains("row 0"));
                assert!(msg.contains("1 cells, expected 2"));
            }
            other => panic!("expected validation error, got {:?}", other),
        }
    }

    #[test]
    fn providing_both_shapes_errors() {
        let schema = schema_with(vec![cd("sku", ColumnType::String)]);
        let instances = vec![serde_json::json!({"sku": "A"})];
        let cols = vec!["sku".to_string()];
        let rows = vec![vec![serde_json::json!("B")]];
        let err = normalize_bulk_create_inputs(
            Some(&instances),
            Some(&cols),
            Some(&rows),
            &serde_json::Map::new(),
            false,
            &schema,
        )
        .unwrap_err();
        assert!(matches!(err, ServiceError::ValidationError(_)));
    }

    #[test]
    fn providing_neither_shape_errors() {
        let schema = schema_with(vec![cd("sku", ColumnType::String)]);
        let err =
            normalize_bulk_create_inputs(None, None, None, &serde_json::Map::new(), false, &schema)
                .unwrap_err();
        assert!(matches!(err, ServiceError::ValidationError(_)));
    }

    #[test]
    fn columnar_without_rows_errors() {
        let schema = schema_with(vec![cd("sku", ColumnType::String)]);
        let cols = vec!["sku".to_string()];
        let err = normalize_bulk_create_inputs(
            None,
            Some(&cols),
            None,
            &serde_json::Map::new(),
            false,
            &schema,
        )
        .unwrap_err();
        assert!(matches!(err, ServiceError::ValidationError(_)));
    }
}
