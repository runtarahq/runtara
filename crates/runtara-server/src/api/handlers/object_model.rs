//! Object Model HTTP Handlers
//!
//! Thin HTTP layer that extracts request data and delegates to SchemaService and InstanceService

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::Json,
};
use serde_json::{Value, json};
use sqlx::PgPool;
use std::sync::Arc;

use crate::api::dto::object_model::*;
use crate::api::repositories::object_model::ObjectStoreManager;
use crate::api::services::object_model::{InstanceService, SchemaService, ServiceError};

// ============================================================================
// Combined State for Object Model Handlers
// ============================================================================

/// Combined state for object model handlers
///
/// Bundles ObjectStoreManager with the connection pool needed for
/// resolving connection IDs to database URLs.
#[derive(Clone)]
pub struct ObjectModelState {
    /// Manager for ObjectStore instances (caches by tenant/URL)
    pub manager: Arc<ObjectStoreManager>,
    /// Database pool for connection resolution
    pub pool: PgPool,
    /// Connections facade for resolving connection IDs
    pub connections: Arc<runtara_connections::ConnectionsFacade>,
}

// ============================================================================
// Schema Handlers
// ============================================================================

/// Create a new schema
///
/// Creates a new object schema with typed columns and indexes. Each schema generates a dedicated
/// PostgreSQL table in the object model database with automatic tenant isolation.
///
/// **Supported Column Types:**
/// - `string` - Unlimited text (TEXT)
/// - `integer` - 64-bit integer (BIGINT)
/// - `decimal` - Fixed-point decimal with precision/scale (NUMERIC)
/// - `boolean` - True/false (BOOLEAN)
/// - `timestamp` - UTC timestamp (TIMESTAMP WITH TIME ZONE)
/// - `json` - Binary JSON (JSONB)
/// - `enum` - String with allowed values (TEXT with CHECK constraint)
///
/// **Auto-managed Columns:**
/// Every table automatically includes: id, created_at, updated_at, deleted
#[utoipa::path(
    post,
    path = "/api/runtime/object-model/schemas",
    params(
        ("connectionId" = Option<String>, Query, description = "Optional connection ID for database selection")
    ),
    request_body(
        content = CreateSchemaRequest,
        description = "Schema definition with columns and optional indexes",
        example = json!({
            "name": "products",
            "description": "Product catalog",
            "tableName": "products",
            "columns": [
                {
                    "name": "sku",
                    "type": "string",
                    "nullable": false,
                    "unique": true
                },
                {
                    "name": "price",
                    "type": "decimal",
                    "precision": 10,
                    "scale": 2,
                    "nullable": false,
                    "default": "0.00"
                },
                {
                    "name": "status",
                    "type": "enum",
                    "values": ["active", "inactive"],
                    "nullable": false,
                    "default": "'active'"
                }
            ],
            "indexes": [
                {
                    "name": "idx_sku",
                    "columns": ["sku"],
                    "unique": true
                }
            ]
        })
    ),
    responses(
        (status = 201, description = "Schema created successfully", body = CreateSchemaResponse,
            example = json!({
                "success": true,
                "schemaId": "550e8400-e29b-41d4-a716-446655440000",
                "message": "Schema created successfully"
            })
        ),
        (status = 400, description = "Invalid request - validation error", body = Value,
            example = json!({
                "success": false,
                "error": "Enum type must have at least one value"
            })
        ),
        (status = 409, description = "Schema with this name already exists", body = Value,
            example = json!({
                "success": false,
                "error": "Schema with name 'products' already exists"
            })
        ),
        (status = 500, description = "Internal server error", body = Value),
    ),
    tag = "object-model",
    security(
        ("tenant_auth" = [])
    )
)]
pub async fn create_schema(
    crate::middleware::tenant_auth::OrgId(tenant_id): crate::middleware::tenant_auth::OrgId,
    State(state): State<Arc<ObjectModelState>>,
    Query(params): Query<ConnectionQueryParams>,
    Json(request): Json<CreateSchemaRequest>,
) -> Result<(StatusCode, Json<CreateSchemaResponse>), (StatusCode, Json<Value>)> {
    let service = SchemaService::new(state.manager.clone(), state.connections.clone());

    match service
        .create_schema(request, &tenant_id, params.connection_id.as_deref())
        .await
    {
        Ok(schema_id) => Ok((
            StatusCode::CREATED,
            Json(CreateSchemaResponse {
                success: true,
                schema_id,
                message: "Schema created successfully".to_string(),
            }),
        )),
        Err(ServiceError::ValidationError(msg)) => Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"success": false, "error": msg})),
        )),
        Err(ServiceError::Conflict(msg)) => Err((
            StatusCode::CONFLICT,
            Json(json!({"success": false, "error": msg})),
        )),
        Err(ServiceError::DatabaseError(msg)) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"success": false, "error": msg})),
        )),
        Err(ServiceError::NotFound(msg)) => Err((
            StatusCode::NOT_FOUND,
            Json(json!({"success": false, "error": msg})),
        )),
    }
}

/// List all schemas with pagination
#[utoipa::path(
    get,
    path = "/api/runtime/object-model/schemas",
    params(
        ("offset" = Option<i64>, Query, description = "Pagination offset (default: 0)"),
        ("limit" = Option<i64>, Query, description = "Pagination limit (default: 100)"),
        ("connectionId" = Option<String>, Query, description = "Optional connection ID for database selection")
    ),
    responses(
        (status = 200, description = "Schemas retrieved successfully", body = ListSchemasResponse),
        (status = 500, description = "Internal server error", body = Value),
    ),
    tag = "object-model"
)]
pub async fn list_schemas(
    crate::middleware::tenant_auth::OrgId(tenant_id): crate::middleware::tenant_auth::OrgId,
    State(state): State<Arc<ObjectModelState>>,
    Query(params): Query<ObjectModelQueryParams>,
) -> Result<(StatusCode, Json<ListSchemasResponse>), (StatusCode, Json<Value>)> {
    let service = SchemaService::new(state.manager.clone(), state.connections.clone());

    match service
        .list_schemas(
            &tenant_id,
            params.offset,
            params.limit,
            params.connection_id.as_deref(),
        )
        .await
    {
        Ok((schemas, total_count)) => Ok((
            StatusCode::OK,
            Json(ListSchemasResponse {
                success: true,
                schemas,
                total_count,
                offset: params.offset,
                limit: params.limit,
            }),
        )),
        Err(ServiceError::DatabaseError(msg)) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"success": false, "error": msg})),
        )),
        Err(ServiceError::ValidationError(msg)) => Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"success": false, "error": msg})),
        )),
        Err(ServiceError::NotFound(msg)) => Err((
            StatusCode::NOT_FOUND,
            Json(json!({"success": false, "error": msg})),
        )),
        Err(ServiceError::Conflict(msg)) => Err((
            StatusCode::CONFLICT,
            Json(json!({"success": false, "error": msg})),
        )),
    }
}

/// Get a single instance by ID
///
/// Retrieves a specific instance by its ID. Requires the schema ID to locate the correct table.
#[utoipa::path(
    get,
    path = "/api/runtime/object-model/instances/{schema_id}/{instance_id}",
    params(
        ("schema_id" = String, Path, description = "Schema ID"),
        ("instance_id" = String, Path, description = "Instance ID"),
        ("connectionId" = Option<String>, Query, description = "Optional connection ID for database selection")
    ),
    responses(
        (status = 200, description = "Instance retrieved successfully", body = GetInstanceResponse,
            example = json!({
                "success": true,
                "instance": {
                    "id": "660e8400-e29b-41d4-a716-446655440001",
                    "tenantId": "my-tenant",
                    "schemaId": "550e8400-e29b-41d4-a716-446655440000",
                    "schemaName": "products",
                    "properties": {
                        "sku": "PROD-001",
                        "title": "Widget",
                        "price": 29.99,
                        "stock": 100,
                        "status": "active"
                    },
                    "createdAt": "2025-01-15T10:00:00Z",
                    "updatedAt": "2025-01-15T10:00:00Z"
                }
            })
        ),
        (status = 404, description = "Instance or schema not found", body = Value,
            example = json!({
                "success": false,
                "error": "Instance not found"
            })
        ),
        (status = 500, description = "Internal server error", body = Value),
    ),
    tag = "object-model",
    security(
        ("tenant_auth" = [])
    )
)]
pub async fn get_instance_by_id(
    crate::middleware::tenant_auth::OrgId(tenant_id): crate::middleware::tenant_auth::OrgId,
    State(state): State<Arc<ObjectModelState>>,
    Path((schema_id, instance_id)): Path<(String, String)>,
    Query(params): Query<ConnectionQueryParams>,
) -> Result<(StatusCode, Json<GetInstanceResponse>), (StatusCode, Json<Value>)> {
    let service = InstanceService::new(state.manager.clone(), state.connections.clone());

    match service
        .get_instance_by_id(
            &instance_id,
            &schema_id,
            &tenant_id,
            params.connection_id.as_deref(),
        )
        .await
    {
        Ok(Some(instance)) => Ok((
            StatusCode::OK,
            Json(GetInstanceResponse {
                success: true,
                instance,
            }),
        )),
        Ok(None) => Err((
            StatusCode::NOT_FOUND,
            Json(json!({"success": false, "error": "Instance not found"})),
        )),
        Err(ServiceError::NotFound(msg)) => Err((
            StatusCode::NOT_FOUND,
            Json(json!({"success": false, "error": msg})),
        )),
        Err(ServiceError::DatabaseError(msg)) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"success": false, "error": msg})),
        )),
        Err(ServiceError::ValidationError(msg)) => Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"success": false, "error": msg})),
        )),
        Err(ServiceError::Conflict(msg)) => Err((
            StatusCode::CONFLICT,
            Json(json!({"success": false, "error": msg})),
        )),
    }
}

/// Update an instance
///
/// Updates an existing instance with type-validated properties. Only provided fields are updated.
#[utoipa::path(
    put,
    path = "/api/runtime/object-model/instances/{schema_id}/{instance_id}",
    params(
        ("schema_id" = String, Path, description = "Schema ID"),
        ("instance_id" = String, Path, description = "Instance ID"),
        ("connectionId" = Option<String>, Query, description = "Optional connection ID for database selection")
    ),
    request_body(
        content = UpdateInstanceRequest,
        description = "Updated properties (partial update supported)",
        example = json!({
            "properties": {
                "price": 39.99,
                "stock": 95
            }
        })
    ),
    responses(
        (status = 200, description = "Instance updated successfully", body = UpdateInstanceResponse,
            example = json!({
                "success": true,
                "message": "Instance updated successfully"
            })
        ),
        (status = 400, description = "Invalid request or validation failed", body = Value,
            example = json!({
                "success": false,
                "error": "Invalid value for column 'price': Type mismatch"
            })
        ),
        (status = 404, description = "Instance or schema not found", body = Value,
            example = json!({
                "success": false,
                "error": "Instance not found"
            })
        ),
        (status = 500, description = "Internal server error", body = Value),
    ),
    tag = "object-model",
    security(
        ("tenant_auth" = [])
    )
)]
pub async fn update_instance(
    crate::middleware::tenant_auth::OrgId(tenant_id): crate::middleware::tenant_auth::OrgId,
    State(state): State<Arc<ObjectModelState>>,
    Path((schema_id, instance_id)): Path<(String, String)>,
    Query(params): Query<ConnectionQueryParams>,
    Json(request): Json<UpdateInstanceRequest>,
) -> Result<(StatusCode, Json<UpdateInstanceResponse>), (StatusCode, Json<Value>)> {
    let service = InstanceService::new(state.manager.clone(), state.connections.clone());

    match service
        .update_instance(
            &instance_id,
            &schema_id,
            &tenant_id,
            request.properties,
            params.connection_id.as_deref(),
        )
        .await
    {
        Ok(_) => Ok((
            StatusCode::OK,
            Json(UpdateInstanceResponse {
                success: true,
                message: "Instance updated successfully".to_string(),
            }),
        )),
        Err(ServiceError::NotFound(msg)) => Err((
            StatusCode::NOT_FOUND,
            Json(json!({"success": false, "error": msg})),
        )),
        Err(ServiceError::ValidationError(msg)) => Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"success": false, "error": msg})),
        )),
        Err(ServiceError::DatabaseError(msg)) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"success": false, "error": msg})),
        )),
        Err(ServiceError::Conflict(msg)) => Err((
            StatusCode::CONFLICT,
            Json(json!({"success": false, "error": msg})),
        )),
    }
}

/// Delete an instance
///
/// Soft deletes an instance (sets deleted flag to true). The instance can be recovered.
#[utoipa::path(
    delete,
    path = "/api/runtime/object-model/instances/{schema_id}/{instance_id}",
    params(
        ("schema_id" = String, Path, description = "Schema ID"),
        ("instance_id" = String, Path, description = "Instance ID"),
        ("connectionId" = Option<String>, Query, description = "Optional connection ID for database selection")
    ),
    responses(
        (status = 200, description = "Instance deleted successfully", body = Value,
            example = json!({
                "success": true,
                "message": "Instance deleted successfully"
            })
        ),
        (status = 404, description = "Instance or schema not found", body = Value,
            example = json!({
                "success": false,
                "error": "Instance not found"
            })
        ),
        (status = 500, description = "Internal server error", body = Value),
    ),
    tag = "object-model",
    security(
        ("tenant_auth" = [])
    )
)]
pub async fn delete_instance(
    crate::middleware::tenant_auth::OrgId(tenant_id): crate::middleware::tenant_auth::OrgId,
    State(state): State<Arc<ObjectModelState>>,
    Path((schema_id, instance_id)): Path<(String, String)>,
    Query(params): Query<ConnectionQueryParams>,
) -> Result<(StatusCode, Json<Value>), (StatusCode, Json<Value>)> {
    let service = InstanceService::new(state.manager.clone(), state.connections.clone());

    match service
        .delete_instance(
            &instance_id,
            &schema_id,
            &tenant_id,
            params.connection_id.as_deref(),
        )
        .await
    {
        Ok(_) => Ok((
            StatusCode::OK,
            Json(json!({
                "success": true,
                "message": "Instance deleted successfully"
            })),
        )),
        Err(ServiceError::NotFound(msg)) => Err((
            StatusCode::NOT_FOUND,
            Json(json!({"success": false, "error": msg})),
        )),
        Err(ServiceError::DatabaseError(msg)) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"success": false, "error": msg})),
        )),
        Err(ServiceError::ValidationError(msg)) => Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"success": false, "error": msg})),
        )),
        Err(ServiceError::Conflict(msg)) => Err((
            StatusCode::CONFLICT,
            Json(json!({"success": false, "error": msg})),
        )),
    }
}

/// Bulk delete instances
///
/// Soft deletes multiple instances in a single operation.
#[utoipa::path(
    delete,
    path = "/api/runtime/object-model/instances/{schema_id}/bulk",
    params(
        ("schema_id" = String, Path, description = "Schema ID"),
        ("connectionId" = Option<String>, Query, description = "Optional connection ID for database selection")
    ),
    request_body(
        content = BulkDeleteRequest,
        description = "List of instance IDs to delete",
        example = json!({
            "instanceIds": [
                "660e8400-e29b-41d4-a716-446655440001",
                "660e8400-e29b-41d4-a716-446655440002",
                "660e8400-e29b-41d4-a716-446655440003"
            ]
        })
    ),
    responses(
        (status = 200, description = "Instances deleted successfully", body = BulkDeleteResponse,
            example = json!({
                "success": true,
                "deletedCount": 3,
                "message": "3 instances deleted successfully"
            })
        ),
        (status = 404, description = "Schema not found", body = Value,
            example = json!({
                "success": false,
                "error": "Schema not found"
            })
        ),
        (status = 500, description = "Internal server error", body = Value),
    ),
    tag = "object-model",
    security(
        ("tenant_auth" = [])
    )
)]
pub async fn bulk_delete_instances(
    crate::middleware::tenant_auth::OrgId(tenant_id): crate::middleware::tenant_auth::OrgId,
    State(state): State<Arc<ObjectModelState>>,
    Path(schema_id): Path<String>,
    Query(params): Query<ConnectionQueryParams>,
    Json(request): Json<BulkDeleteRequest>,
) -> Result<(StatusCode, Json<BulkDeleteResponse>), (StatusCode, Json<Value>)> {
    let service = InstanceService::new(state.manager.clone(), state.connections.clone());

    match service
        .bulk_delete_instances(
            request.instance_ids,
            &schema_id,
            &tenant_id,
            params.connection_id.as_deref(),
        )
        .await
    {
        Ok(deleted_count) => Ok((
            StatusCode::OK,
            Json(BulkDeleteResponse {
                success: true,
                deleted_count,
                message: format!("{} instances deleted successfully", deleted_count),
            }),
        )),
        Err(ServiceError::NotFound(msg)) => Err((
            StatusCode::NOT_FOUND,
            Json(json!({"success": false, "error": msg})),
        )),
        Err(ServiceError::DatabaseError(msg)) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"success": false, "error": msg})),
        )),
        Err(ServiceError::ValidationError(msg)) => Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"success": false, "error": msg})),
        )),
        Err(ServiceError::Conflict(msg)) => Err((
            StatusCode::CONFLICT,
            Json(json!({"success": false, "error": msg})),
        )),
    }
}

/// Bulk create instances
///
/// Creates multiple instances in a single transaction. If any validation fails,
/// no rows are inserted.
#[utoipa::path(
    post,
    path = "/api/runtime/object-model/instances/{schema_id}/bulk",
    params(
        ("schema_id" = String, Path, description = "Schema ID"),
        ("connectionId" = Option<String>, Query, description = "Optional connection ID for database selection")
    ),
    request_body(
        content = BulkCreateRequest,
        description = "Array of JSON objects to insert",
        example = json!({
            "instances": [
                {"sku": "A", "quantity": 1},
                {"sku": "B", "quantity": 2}
            ]
        })
    ),
    responses(
        (status = 201, description = "Instances created", body = BulkCreateResponse),
        (status = 400, description = "Validation error", body = Value),
        (status = 404, description = "Schema not found", body = Value),
        (status = 500, description = "Internal server error", body = Value),
    ),
    tag = "object-model",
    security(
        ("tenant_auth" = [])
    )
)]
pub async fn bulk_create_instances(
    crate::middleware::tenant_auth::OrgId(tenant_id): crate::middleware::tenant_auth::OrgId,
    State(state): State<Arc<ObjectModelState>>,
    Path(schema_id): Path<String>,
    Query(params): Query<ConnectionQueryParams>,
    Json(request): Json<BulkCreateRequest>,
) -> Result<(StatusCode, Json<BulkCreateResponse>), (StatusCode, Json<Value>)> {
    let service = InstanceService::new(state.manager.clone(), state.connections.clone());

    match service
        .bulk_create_instances(
            &schema_id,
            request,
            &tenant_id,
            params.connection_id.as_deref(),
        )
        .await
    {
        Ok(result) => Ok((
            StatusCode::CREATED,
            Json(BulkCreateResponse {
                success: true,
                created_count: result.created_count,
                skipped_count: result.skipped_count,
                errors: result
                    .errors
                    .into_iter()
                    .map(|e| BulkRowError {
                        index: e.index,
                        reason: e.reason,
                    })
                    .collect(),
                message: format!(
                    "{} created, {} skipped",
                    result.created_count, result.skipped_count
                ),
            }),
        )),
        Err(ServiceError::NotFound(msg)) => Err((
            StatusCode::NOT_FOUND,
            Json(json!({"success": false, "error": msg})),
        )),
        Err(ServiceError::ValidationError(msg)) => Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"success": false, "error": msg})),
        )),
        Err(ServiceError::Conflict(msg)) => Err((
            StatusCode::CONFLICT,
            Json(json!({"success": false, "error": msg})),
        )),
        Err(ServiceError::DatabaseError(msg)) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"success": false, "error": msg})),
        )),
    }
}

/// Bulk update instances
///
/// Updates multiple instances in a single transaction. Supports two modes:
/// `byCondition` applies the same properties to every row matching the condition;
/// `byIds` applies per-row properties to each listed id.
#[utoipa::path(
    patch,
    path = "/api/runtime/object-model/instances/{schema_id}/bulk",
    params(
        ("schema_id" = String, Path, description = "Schema ID"),
        ("connectionId" = Option<String>, Query, description = "Optional connection ID for database selection")
    ),
    request_body(
        content = BulkUpdateRequest,
        description = "Bulk update payload (mode=byCondition or byIds)",
        example = json!({
            "mode": "byCondition",
            "properties": {"status": "archived"},
            "condition": {"op": "IN", "arguments": ["id", ["id1", "id2"]]}
        })
    ),
    responses(
        (status = 200, description = "Instances updated", body = BulkUpdateResponse),
        (status = 400, description = "Validation error", body = Value),
        (status = 404, description = "Schema not found", body = Value),
        (status = 500, description = "Internal server error", body = Value),
    ),
    tag = "object-model",
    security(
        ("tenant_auth" = [])
    )
)]
pub async fn bulk_update_instances(
    crate::middleware::tenant_auth::OrgId(tenant_id): crate::middleware::tenant_auth::OrgId,
    State(state): State<Arc<ObjectModelState>>,
    Path(schema_id): Path<String>,
    Query(params): Query<ConnectionQueryParams>,
    Json(request): Json<BulkUpdateRequest>,
) -> Result<(StatusCode, Json<BulkUpdateResponse>), (StatusCode, Json<Value>)> {
    let service = InstanceService::new(state.manager.clone(), state.connections.clone());

    let result = match request {
        BulkUpdateRequest::ByCondition {
            properties,
            condition,
        } => {
            service
                .bulk_update_instances_by_condition(
                    &schema_id,
                    properties,
                    condition,
                    &tenant_id,
                    params.connection_id.as_deref(),
                )
                .await
        }
        BulkUpdateRequest::ByIds { updates } => {
            service
                .bulk_update_instances_by_ids(
                    &schema_id,
                    updates,
                    &tenant_id,
                    params.connection_id.as_deref(),
                )
                .await
        }
    };

    match result {
        Ok(updated_count) => Ok((
            StatusCode::OK,
            Json(BulkUpdateResponse {
                success: true,
                updated_count,
                message: format!("{} instances updated successfully", updated_count),
            }),
        )),
        Err(ServiceError::NotFound(msg)) => Err((
            StatusCode::NOT_FOUND,
            Json(json!({"success": false, "error": msg})),
        )),
        Err(ServiceError::ValidationError(msg)) => Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"success": false, "error": msg})),
        )),
        Err(ServiceError::Conflict(msg)) => Err((
            StatusCode::CONFLICT,
            Json(json!({"success": false, "error": msg})),
        )),
        Err(ServiceError::DatabaseError(msg)) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"success": false, "error": msg})),
        )),
    }
}

/// Get a schema by ID
#[utoipa::path(
    get,
    path = "/api/runtime/object-model/schemas/{id}",
    params(
        ("id" = String, Path, description = "Schema ID"),
        ("connectionId" = Option<String>, Query, description = "Optional connection ID for database selection")
    ),
    responses(
        (status = 200, description = "Schema retrieved successfully", body = GetSchemaResponse),
        (status = 404, description = "Schema not found", body = Value),
        (status = 500, description = "Internal server error", body = Value),
    ),
    tag = "object-model"
)]
pub async fn get_schema_by_id(
    crate::middleware::tenant_auth::OrgId(tenant_id): crate::middleware::tenant_auth::OrgId,
    State(state): State<Arc<ObjectModelState>>,
    Path(id): Path<String>,
    Query(params): Query<ConnectionQueryParams>,
) -> Result<(StatusCode, Json<GetSchemaResponse>), (StatusCode, Json<Value>)> {
    let service = SchemaService::new(state.manager.clone(), state.connections.clone());

    match service
        .get_schema_by_id(&id, &tenant_id, params.connection_id.as_deref())
        .await
    {
        Ok(schema) => Ok((
            StatusCode::OK,
            Json(GetSchemaResponse {
                success: true,
                schema,
            }),
        )),
        Err(ServiceError::NotFound(msg)) => Err((
            StatusCode::NOT_FOUND,
            Json(json!({"success": false, "error": msg})),
        )),
        Err(ServiceError::DatabaseError(msg)) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"success": false, "error": msg})),
        )),
        Err(ServiceError::ValidationError(msg)) => Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"success": false, "error": msg})),
        )),
        Err(ServiceError::Conflict(msg)) => Err((
            StatusCode::CONFLICT,
            Json(json!({"success": false, "error": msg})),
        )),
    }
}

/// Get a schema by name
#[utoipa::path(
    get,
    path = "/api/runtime/object-model/schemas/name/{name}",
    params(
        ("name" = String, Path, description = "Schema name"),
        ("connectionId" = Option<String>, Query, description = "Optional connection ID for database selection")
    ),
    responses(
        (status = 200, description = "Schema retrieved successfully", body = GetSchemaResponse),
        (status = 404, description = "Schema not found", body = Value),
        (status = 500, description = "Internal server error", body = Value),
    ),
    tag = "object-model"
)]
pub async fn get_schema_by_name(
    crate::middleware::tenant_auth::OrgId(tenant_id): crate::middleware::tenant_auth::OrgId,
    State(state): State<Arc<ObjectModelState>>,
    Path(name): Path<String>,
    Query(params): Query<ConnectionQueryParams>,
) -> Result<(StatusCode, Json<GetSchemaResponse>), (StatusCode, Json<Value>)> {
    let service = SchemaService::new(state.manager.clone(), state.connections.clone());

    match service
        .get_schema_by_name(&name, &tenant_id, params.connection_id.as_deref())
        .await
    {
        Ok(schema) => Ok((
            StatusCode::OK,
            Json(GetSchemaResponse {
                success: true,
                schema,
            }),
        )),
        Err(ServiceError::NotFound(msg)) => Err((
            StatusCode::NOT_FOUND,
            Json(json!({"success": false, "error": msg})),
        )),
        Err(ServiceError::DatabaseError(msg)) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"success": false, "error": msg})),
        )),
        Err(ServiceError::ValidationError(msg)) => Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"success": false, "error": msg})),
        )),
        Err(ServiceError::Conflict(msg)) => Err((
            StatusCode::CONFLICT,
            Json(json!({"success": false, "error": msg})),
        )),
    }
}

/// Update a schema
#[utoipa::path(
    put,
    path = "/api/runtime/object-model/schemas/{id}",
    request_body = UpdateSchemaRequest,
    params(
        ("id" = String, Path, description = "Schema ID"),
        ("connectionId" = Option<String>, Query, description = "Optional connection ID for database selection")
    ),
    responses(
        (status = 200, description = "Schema updated successfully", body = UpdateSchemaResponse),
        (status = 400, description = "Invalid request", body = Value),
        (status = 404, description = "Schema not found", body = Value),
        (status = 409, description = "Schema name conflict", body = Value),
        (status = 500, description = "Internal server error", body = Value),
    ),
    tag = "object-model"
)]
pub async fn update_schema(
    crate::middleware::tenant_auth::OrgId(tenant_id): crate::middleware::tenant_auth::OrgId,
    State(state): State<Arc<ObjectModelState>>,
    Path(id): Path<String>,
    Query(params): Query<ConnectionQueryParams>,
    Json(request): Json<UpdateSchemaRequest>,
) -> Result<(StatusCode, Json<UpdateSchemaResponse>), (StatusCode, Json<Value>)> {
    let service = SchemaService::new(state.manager.clone(), state.connections.clone());

    match service
        .update_schema(&id, &tenant_id, request, params.connection_id.as_deref())
        .await
    {
        Ok(_) => Ok((
            StatusCode::OK,
            Json(UpdateSchemaResponse {
                success: true,
                message: "Schema updated successfully".to_string(),
            }),
        )),
        Err(ServiceError::NotFound(msg)) => Err((
            StatusCode::NOT_FOUND,
            Json(json!({"success": false, "error": msg})),
        )),
        Err(ServiceError::ValidationError(msg)) => Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"success": false, "error": msg})),
        )),
        Err(ServiceError::Conflict(msg)) => Err((
            StatusCode::CONFLICT,
            Json(json!({"success": false, "error": msg})),
        )),
        Err(ServiceError::DatabaseError(msg)) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"success": false, "error": msg})),
        )),
    }
}

/// Delete a schema
#[utoipa::path(
    delete,
    path = "/api/runtime/object-model/schemas/{id}",
    params(
        ("id" = String, Path, description = "Schema ID"),
        ("connectionId" = Option<String>, Query, description = "Optional connection ID for database selection")
    ),
    responses(
        (status = 200, description = "Schema deleted successfully", body = Value),
        (status = 404, description = "Schema not found", body = Value),
        (status = 500, description = "Internal server error", body = Value),
    ),
    tag = "object-model"
)]
pub async fn delete_schema(
    crate::middleware::tenant_auth::OrgId(tenant_id): crate::middleware::tenant_auth::OrgId,
    State(state): State<Arc<ObjectModelState>>,
    Path(id): Path<String>,
    Query(params): Query<ConnectionQueryParams>,
) -> Result<(StatusCode, Json<Value>), (StatusCode, Json<Value>)> {
    let service = SchemaService::new(state.manager.clone(), state.connections.clone());

    match service
        .delete_schema(&id, &tenant_id, params.connection_id.as_deref())
        .await
    {
        Ok(_) => Ok((
            StatusCode::OK,
            Json(json!({
                "success": true,
                "message": "Schema deleted successfully"
            })),
        )),
        Err(ServiceError::NotFound(msg)) => Err((
            StatusCode::NOT_FOUND,
            Json(json!({"success": false, "error": msg})),
        )),
        Err(ServiceError::DatabaseError(msg)) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"success": false, "error": msg})),
        )),
        Err(ServiceError::ValidationError(msg)) => Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"success": false, "error": msg})),
        )),
        Err(ServiceError::Conflict(msg)) => Err((
            StatusCode::CONFLICT,
            Json(json!({"success": false, "error": msg})),
        )),
    }
}

// ============================================================================
// Instance Handlers
// ============================================================================

/// Create a new instance
///
/// Creates a new instance of an object with type-validated properties. All values are validated
/// against the schema definition including type checking, nullable constraints, and enum values.
///
/// **Type Requirements:**
/// - `string` - Provide string value
/// - `integer` - Provide integer value (JavaScript number without decimals)
/// - `decimal` - Provide number value (JavaScript number with decimals)
/// - `boolean` - Provide boolean value (true/false)
/// - `timestamp` - Provide ISO 8601 string (e.g., "2025-01-15T10:00:00Z")
/// - `json` - Provide any JSON value (object, array, string, number, boolean, null)
/// - `enum` - Provide string matching one of the allowed values
#[utoipa::path(
    post,
    path = "/api/runtime/object-model/instances",
    params(
        ("connectionId" = Option<String>, Query, description = "Optional connection ID for database selection")
    ),
    request_body(
        content = CreateInstanceRequest,
        description = "Instance data with schemaId and type-validated properties",
        example = json!({
            "schemaId": "550e8400-e29b-41d4-a716-446655440000",
            "properties": {
                "sku": "PROD-001",
                "title": "Widget",
                "price": 29.99,
                "stock": 100,
                "status": "active",
                "metadata": {"color": "blue", "weight": 1.5},
                "published_at": "2025-01-15T10:00:00Z"
            }
        })
    ),
    responses(
        (status = 201, description = "Instance created successfully", body = CreateInstanceResponse,
            example = json!({
                "success": true,
                "instanceId": "660e8400-e29b-41d4-a716-446655440001",
                "message": "Instance created successfully"
            })
        ),
        (status = 400, description = "Invalid request or validation failed", body = Value,
            example = json!({
                "success": false,
                "error": "Invalid value for column 'price': Type mismatch: expected Decimal, got String"
            })
        ),
        (status = 404, description = "Schema not found", body = Value,
            example = json!({
                "success": false,
                "error": "Schema not found"
            })
        ),
        (status = 500, description = "Internal server error", body = Value),
    ),
    tag = "object-model",
    security(
        ("tenant_auth" = [])
    )
)]
pub async fn create_instance(
    crate::middleware::tenant_auth::OrgId(tenant_id): crate::middleware::tenant_auth::OrgId,
    State(state): State<Arc<ObjectModelState>>,
    Query(params): Query<ConnectionQueryParams>,
    Json(request): Json<CreateInstanceRequest>,
) -> Result<(StatusCode, Json<CreateInstanceResponse>), (StatusCode, Json<Value>)> {
    let service = InstanceService::new(state.manager.clone(), state.connections.clone());

    tracing::debug!(
        schema_id = ?request.schema_id,
        schema_name = ?request.schema_name,
        "Create instance request"
    );

    match service
        .create_instance(request, &tenant_id, params.connection_id.as_deref())
        .await
    {
        Ok(instance_id) => Ok((
            StatusCode::CREATED,
            Json(CreateInstanceResponse {
                success: true,
                instance_id,
                message: "Instance created successfully".to_string(),
            }),
        )),
        Err(ServiceError::ValidationError(msg)) => {
            tracing::warn!("Create instance validation error: {}", msg);
            Err((
                StatusCode::BAD_REQUEST,
                Json(json!({"success": false, "error": msg})),
            ))
        }
        Err(ServiceError::NotFound(msg)) => {
            tracing::warn!("Create instance not found: {}", msg);
            Err((
                StatusCode::NOT_FOUND,
                Json(json!({"success": false, "error": msg})),
            ))
        }
        Err(ServiceError::DatabaseError(msg)) => {
            tracing::error!("Create instance database error: {}", msg);
            Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"success": false, "error": msg})),
            ))
        }
        Err(ServiceError::Conflict(msg)) => {
            tracing::warn!("Create instance conflict: {}", msg);
            Err((
                StatusCode::CONFLICT,
                Json(json!({"success": false, "error": msg})),
            ))
        }
    }
}

/// Get instances by schema ID
#[utoipa::path(
    get,
    path = "/api/runtime/object-model/instances/schema/{schema_id}",
    params(
        ("schema_id" = String, Path, description = "Schema ID"),
        ("offset" = Option<i64>, Query, description = "Pagination offset (default: 0)"),
        ("limit" = Option<i64>, Query, description = "Pagination limit (default: 100)"),
        ("connectionId" = Option<String>, Query, description = "Optional connection ID for database selection")
    ),
    responses(
        (status = 200, description = "Instances retrieved successfully", body = ListInstancesResponse),
        (status = 404, description = "Schema not found", body = Value),
        (status = 500, description = "Internal server error", body = Value),
    ),
    tag = "object-model"
)]
pub async fn get_instances_by_schema(
    crate::middleware::tenant_auth::OrgId(tenant_id): crate::middleware::tenant_auth::OrgId,
    State(state): State<Arc<ObjectModelState>>,
    Path(schema_id): Path<String>,
    Query(params): Query<ObjectModelQueryParams>,
) -> Result<(StatusCode, Json<ListInstancesResponse>), (StatusCode, Json<Value>)> {
    let service = InstanceService::new(state.manager.clone(), state.connections.clone());

    match service
        .get_instances_by_schema(
            &schema_id,
            &tenant_id,
            params.offset,
            params.limit,
            params.connection_id.as_deref(),
        )
        .await
    {
        Ok((instances, total_count)) => Ok((
            StatusCode::OK,
            Json(ListInstancesResponse {
                success: true,
                instances,
                total_count,
                offset: params.offset,
                limit: params.limit,
            }),
        )),
        Err(ServiceError::NotFound(msg)) => Err((
            StatusCode::NOT_FOUND,
            Json(json!({"success": false, "error": msg})),
        )),
        Err(ServiceError::DatabaseError(msg)) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"success": false, "error": msg})),
        )),
        Err(ServiceError::ValidationError(msg)) => Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"success": false, "error": msg})),
        )),
        Err(ServiceError::Conflict(msg)) => Err((
            StatusCode::CONFLICT,
            Json(json!({"success": false, "error": msg})),
        )),
    }
}

/// Get instances by schema name
#[utoipa::path(
    get,
    path = "/api/runtime/object-model/instances/schema/name/{schema_name}",
    params(
        ("schema_name" = String, Path, description = "Schema name"),
        ("offset" = Option<i64>, Query, description = "Pagination offset (default: 0)"),
        ("limit" = Option<i64>, Query, description = "Pagination limit (default: 100)"),
        ("connectionId" = Option<String>, Query, description = "Optional connection ID for database selection")
    ),
    responses(
        (status = 200, description = "Instances retrieved successfully", body = ListInstancesResponse),
        (status = 404, description = "Schema not found", body = Value),
        (status = 500, description = "Internal server error", body = Value),
    ),
    tag = "object-model"
)]
pub async fn get_instances_by_schema_name(
    crate::middleware::tenant_auth::OrgId(tenant_id): crate::middleware::tenant_auth::OrgId,
    State(state): State<Arc<ObjectModelState>>,
    Path(schema_name): Path<String>,
    Query(params): Query<ObjectModelQueryParams>,
) -> Result<(StatusCode, Json<ListInstancesResponse>), (StatusCode, Json<Value>)> {
    let service = InstanceService::new(state.manager.clone(), state.connections.clone());

    match service
        .get_instances_by_schema_name(
            &schema_name,
            &tenant_id,
            params.offset,
            params.limit,
            params.connection_id.as_deref(),
        )
        .await
    {
        Ok((instances, total_count)) => Ok((
            StatusCode::OK,
            Json(ListInstancesResponse {
                success: true,
                instances,
                total_count,
                offset: params.offset,
                limit: params.limit,
            }),
        )),
        Err(ServiceError::NotFound(msg)) => Err((
            StatusCode::NOT_FOUND,
            Json(json!({"success": false, "error": msg})),
        )),
        Err(ServiceError::DatabaseError(msg)) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"success": false, "error": msg})),
        )),
        Err(ServiceError::ValidationError(msg)) => Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"success": false, "error": msg})),
        )),
        Err(ServiceError::Conflict(msg)) => Err((
            StatusCode::CONFLICT,
            Json(json!({"success": false, "error": msg})),
        )),
    }
}

/// Filter instances with condition-based queries for a specific schema
#[utoipa::path(
    post,
    path = "/api/runtime/object-model/instances/schema/{name}/filter",
    request_body = FilterRequest,
    params(
        ("name" = String, Path, description = "Schema name"),
        ("connectionId" = Option<String>, Query, description = "Optional connection ID for database selection")
    ),
    responses(
        (status = 200, description = "Instances filtered successfully", body = FilterInstancesResponse),
        (status = 400, description = "Invalid filter condition", body = Value),
        (status = 404, description = "Schema not found", body = Value),
        (status = 500, description = "Internal server error", body = Value),
    ),
    tag = "object-model"
)]
pub async fn filter_instances(
    crate::middleware::tenant_auth::OrgId(tenant_id): crate::middleware::tenant_auth::OrgId,
    State(state): State<Arc<ObjectModelState>>,
    Path(schema_name): Path<String>,
    Query(params): Query<ConnectionQueryParams>,
    Json(request): Json<FilterRequest>,
) -> Result<(StatusCode, Json<FilterInstancesResponse>), (StatusCode, Json<Value>)> {
    tracing::info!(
        "Filter instances handler called for schema '{}' with request: {:?}",
        schema_name,
        request
    );

    tracing::info!("Tenant ID: {}", tenant_id);

    let offset = request.offset;
    let limit = request.limit;

    let service = InstanceService::new(state.manager.clone(), state.connections.clone());

    tracing::info!("Calling service.filter_instances_by_schema");
    match service
        .filter_instances_by_schema(
            &tenant_id,
            &schema_name,
            request,
            params.connection_id.as_deref(),
        )
        .await
    {
        Ok((instances, total_count)) => Ok((
            StatusCode::OK,
            Json(FilterInstancesResponse {
                success: true,
                instances,
                total_count,
                offset,
                limit,
            }),
        )),
        Err(ServiceError::ValidationError(msg)) => Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"success": false, "error": msg})),
        )),
        Err(ServiceError::DatabaseError(msg)) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"success": false, "error": msg})),
        )),
        Err(ServiceError::NotFound(msg)) => Err((
            StatusCode::NOT_FOUND,
            Json(json!({"success": false, "error": msg})),
        )),
        Err(ServiceError::Conflict(msg)) => Err((
            StatusCode::CONFLICT,
            Json(json!({"success": false, "error": msg})),
        )),
    }
}

/// Aggregate instances with GROUP BY for a specific schema.
#[utoipa::path(
    post,
    path = "/api/runtime/object-model/instances/schema/{name}/aggregate",
    request_body = AggregateRequest,
    params(
        ("name" = String, Path, description = "Schema name"),
        ("connectionId" = Option<String>, Query, description = "Optional connection ID for database selection")
    ),
    responses(
        (status = 200, description = "Aggregate computed successfully", body = AggregateResponse),
        (status = 400, description = "Invalid aggregate request", body = Value),
        (status = 404, description = "Schema not found", body = Value),
        (status = 500, description = "Internal server error", body = Value),
    ),
    tag = "object-model"
)]
pub async fn aggregate_instances(
    crate::middleware::tenant_auth::OrgId(tenant_id): crate::middleware::tenant_auth::OrgId,
    State(state): State<Arc<ObjectModelState>>,
    Path(schema_name): Path<String>,
    Query(params): Query<ConnectionQueryParams>,
    Json(request): Json<AggregateRequest>,
) -> Result<(StatusCode, Json<AggregateResponse>), (StatusCode, Json<Value>)> {
    tracing::info!(
        "Aggregate instances handler called for schema '{}'",
        schema_name
    );

    let service = InstanceService::new(state.manager.clone(), state.connections.clone());

    match service
        .aggregate_instances_by_schema(
            &tenant_id,
            &schema_name,
            request,
            params.connection_id.as_deref(),
        )
        .await
    {
        Ok(result) => Ok((
            StatusCode::OK,
            Json(AggregateResponse {
                success: true,
                columns: result.columns,
                rows: result.rows,
                group_count: result.group_count,
                error: None,
            }),
        )),
        Err(ServiceError::ValidationError(msg)) => Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"success": false, "error": msg})),
        )),
        Err(ServiceError::DatabaseError(msg)) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"success": false, "error": msg})),
        )),
        Err(ServiceError::NotFound(msg)) => Err((
            StatusCode::NOT_FOUND,
            Json(json!({"success": false, "error": msg})),
        )),
        Err(ServiceError::Conflict(msg)) => Err((
            StatusCode::CONFLICT,
            Json(json!({"success": false, "error": msg})),
        )),
    }
}

/// Execute a typed positional SQL query.
///
/// SQL uses native Postgres / SQLx placeholders (`$1`, `$2`, ...). Parameters
/// are bound in array order and result rows are validated against
/// `resultSchema`.
#[utoipa::path(
    post,
    path = "/api/runtime/object-model/sql/query",
    request_body = SqlQueryRequest,
    params(
        ("connectionId" = Option<String>, Query, description = "Optional connection ID for database selection")
    ),
    responses(
        (status = 200, description = "SQL query executed successfully", body = SqlQueryResponse),
        (status = 400, description = "Invalid SQL query, parameter, or result schema", body = Value),
        (status = 500, description = "Database error", body = Value),
    ),
    tag = "object-model"
)]
pub async fn query_sql(
    crate::middleware::tenant_auth::OrgId(tenant_id): crate::middleware::tenant_auth::OrgId,
    State(state): State<Arc<ObjectModelState>>,
    Query(params): Query<ConnectionQueryParams>,
    Json(request): Json<SqlQueryRequest>,
) -> Result<(StatusCode, Json<SqlQueryResponse>), (StatusCode, Json<Value>)> {
    let service = InstanceService::new(state.manager.clone(), state.connections.clone());

    match service
        .query_sql(&tenant_id, request, params.connection_id.as_deref())
        .await
    {
        Ok(rows) => {
            let row_count = rows.len();
            Ok((
                StatusCode::OK,
                Json(SqlQueryResponse {
                    success: true,
                    rows,
                    row_count,
                }),
            ))
        }
        Err(error) => Err(raw_sql_error_response(error)),
    }
}

/// Execute a typed positional SQL query that must return exactly one row.
#[utoipa::path(
    post,
    path = "/api/runtime/object-model/sql/query-one",
    request_body = SqlQueryRequest,
    params(
        ("connectionId" = Option<String>, Query, description = "Optional connection ID for database selection")
    ),
    responses(
        (status = 200, description = "SQL query returned exactly one row", body = SqlQueryOneResponse),
        (status = 400, description = "Invalid SQL query, result schema, or cardinality", body = Value),
        (status = 500, description = "Database error", body = Value),
    ),
    tag = "object-model"
)]
pub async fn query_sql_one(
    crate::middleware::tenant_auth::OrgId(tenant_id): crate::middleware::tenant_auth::OrgId,
    State(state): State<Arc<ObjectModelState>>,
    Query(params): Query<ConnectionQueryParams>,
    Json(request): Json<SqlQueryRequest>,
) -> Result<(StatusCode, Json<SqlQueryOneResponse>), (StatusCode, Json<Value>)> {
    let service = InstanceService::new(state.manager.clone(), state.connections.clone());

    match service
        .query_sql_one(&tenant_id, request, params.connection_id.as_deref())
        .await
    {
        Ok(row) => Ok((
            StatusCode::OK,
            Json(SqlQueryOneResponse { success: true, row }),
        )),
        Err(error) => Err(raw_sql_error_response(error)),
    }
}

/// Execute a positional SQL query and return raw rows without result-schema validation.
#[utoipa::path(
    post,
    path = "/api/runtime/object-model/sql/query-raw",
    request_body = SqlRawQueryRequest,
    params(
        ("connectionId" = Option<String>, Query, description = "Optional connection ID for database selection")
    ),
    responses(
        (status = 200, description = "Raw SQL query executed successfully", body = SqlQueryResponse),
        (status = 400, description = "Invalid SQL query or parameter", body = Value),
        (status = 500, description = "Database error", body = Value),
    ),
    tag = "object-model"
)]
pub async fn query_sql_raw(
    crate::middleware::tenant_auth::OrgId(tenant_id): crate::middleware::tenant_auth::OrgId,
    State(state): State<Arc<ObjectModelState>>,
    Query(params): Query<ConnectionQueryParams>,
    Json(request): Json<SqlRawQueryRequest>,
) -> Result<(StatusCode, Json<SqlQueryResponse>), (StatusCode, Json<Value>)> {
    let service = InstanceService::new(state.manager.clone(), state.connections.clone());

    match service
        .query_sql_raw(&tenant_id, request, params.connection_id.as_deref())
        .await
    {
        Ok(rows) => {
            let row_count = rows.len();
            Ok((
                StatusCode::OK,
                Json(SqlQueryResponse {
                    success: true,
                    rows,
                    row_count,
                }),
            ))
        }
        Err(error) => Err(raw_sql_error_response(error)),
    }
}

/// Execute a positional SQL command and return rows affected.
#[utoipa::path(
    post,
    path = "/api/runtime/object-model/sql/execute",
    request_body = SqlExecuteRequest,
    params(
        ("connectionId" = Option<String>, Query, description = "Optional connection ID for database selection")
    ),
    responses(
        (status = 200, description = "SQL command executed successfully", body = SqlExecuteResponse),
        (status = 400, description = "Invalid SQL command or parameter", body = Value),
        (status = 500, description = "Database error", body = Value),
    ),
    tag = "object-model"
)]
pub async fn execute_sql(
    crate::middleware::tenant_auth::OrgId(tenant_id): crate::middleware::tenant_auth::OrgId,
    State(state): State<Arc<ObjectModelState>>,
    Query(params): Query<ConnectionQueryParams>,
    Json(request): Json<SqlExecuteRequest>,
) -> Result<(StatusCode, Json<SqlExecuteResponse>), (StatusCode, Json<Value>)> {
    let service = InstanceService::new(state.manager.clone(), state.connections.clone());

    match service
        .execute_sql(&tenant_id, request, params.connection_id.as_deref())
        .await
    {
        Ok(rows_affected) => Ok((
            StatusCode::OK,
            Json(SqlExecuteResponse {
                success: true,
                rows_affected,
            }),
        )),
        Err(error) => Err(raw_sql_error_response(error)),
    }
}

fn raw_sql_error_response(error: ServiceError) -> (StatusCode, Json<Value>) {
    match error {
        ServiceError::ValidationError(msg) => (
            StatusCode::BAD_REQUEST,
            Json(json!({"success": false, "error": msg})),
        ),
        ServiceError::NotFound(msg) => (
            StatusCode::NOT_FOUND,
            Json(json!({"success": false, "error": msg})),
        ),
        ServiceError::Conflict(msg) => (
            StatusCode::CONFLICT,
            Json(json!({"success": false, "error": msg})),
        ),
        ServiceError::DatabaseError(msg) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"success": false, "error": msg})),
        ),
    }
}
