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

    /// Bulk delete instances (soft delete)
    pub async fn bulk_delete_instances(
        &self,
        instance_ids: Vec<String>,
        schema_id: &str,
        tenant_id: &str,
        connection_id: Option<&str>,
    ) -> Result<usize, ServiceError> {
        let store = get_store(&self.manager, Some(&self.facade), connection_id, tenant_id).await?;

        // Fetch schema first to get its name
        let schema = store
            .get_schema_by_id(schema_id)
            .await
            .map_err(|e| ServiceError::DatabaseError(e.to_string()))?
            .ok_or_else(|| ServiceError::NotFound("Schema not found".to_string()))?;

        let mut deleted = 0u64;
        for id in &instance_ids {
            match store.delete_instance(&schema.name, id).await {
                Ok(()) => deleted += 1,
                Err(e) if e.to_string().contains("not found") => {
                    // Instance not found, skip it
                }
                Err(e) => return Err(ServiceError::DatabaseError(e.to_string())),
            }
        }

        Ok(deleted as usize)
    }
}

// ============================================================================
// Service Errors
// ============================================================================

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
