//! Connection API Handlers
//!
//! Thin HTTP handlers that delegate to ConnectionService
//! SECURITY: All handlers exclude connection_parameters from responses

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::Json,
};
use serde_json::{Value, json};
use sqlx::PgPool;
use std::sync::Arc;

use crate::crypto::CredentialCipher;
use crate::repository::connections::ConnectionRepository;
use crate::service::connections::{ConnectionService, ServiceError};
use crate::service::rate_limits::RateLimitService;
use crate::types::*;

/// Create a new connection
#[cfg_attr(feature = "utoipa", utoipa::path(
    post,
    path = "/api/runtime/connections",
    request_body = CreateConnectionRequest,
    responses(
        (status = 201, description = "Connection created successfully", body = CreateConnectionResponse),
        (status = 400, description = "Invalid request", body = ErrorResponse),
        (status = 409, description = "Connection with this title already exists", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    ),
    tag = "connections-controller"
))]
pub async fn create_connection_handler(
    crate::tenant::TenantId(tenant_id): crate::tenant::TenantId,
    State(pool): State<PgPool>,
    State(cipher): State<Arc<dyn CredentialCipher>>,
    Json(payload): Json<CreateConnectionRequest>,
) -> Result<(StatusCode, Json<CreateConnectionResponse>), (StatusCode, Json<Value>)> {
    // Create service with repository
    let repository = Arc::new(ConnectionRepository::new(pool, cipher.clone()));
    let service = ConnectionService::new(repository);

    match service.create_connection(payload, &tenant_id).await {
        Ok(connection_id) => Ok((
            StatusCode::CREATED,
            Json(CreateConnectionResponse {
                success: true,
                message: "Connection created successfully".to_string(),
                connection_id,
            }),
        )),
        Err(ServiceError::ValidationError(msg)) => Err((
            StatusCode::BAD_REQUEST,
            Json(json!({
                "success": false,
                "error": msg,
                "message": Value::Null
            })),
        )),
        Err(ServiceError::Conflict(msg)) => Err((
            StatusCode::CONFLICT,
            Json(json!({
                "success": false,
                "error": msg,
                "message": Value::Null
            })),
        )),
        Err(ServiceError::DatabaseError(msg)) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({
                "success": false,
                "error": "Failed to create connection",
                "message": msg
            })),
        )),
        Err(ServiceError::NotFound(_)) => unreachable!("Create should not return NotFound"),
    }
}

/// List all connections for a tenant
/// SECURITY: Does NOT return connection_parameters field
#[cfg_attr(feature = "utoipa", utoipa::path(
    get,
    path = "/api/runtime/connections",
    params(
        ("integrationId" = Option<String>, Query, description = "Filter by integration ID (connection type identifier)"),
        ("status" = Option<String>, Query, description = "Filter by status (UNKNOWN, ACTIVE, REQUIRES_RECONNECTION, INVALID_CREDENTIALS)"),
        ("includeRateLimitStats" = Option<bool>, Query, description = "Include rate limit statistics for each connection"),
        ("interval" = Option<String>, Query, description = "Time interval for rate limit stats: 1h, 24h, 7d, 30d (default: 24h)")
    ),
    responses(
        (status = 200, description = "List of connections (without sensitive connection_parameters)", body = ListConnectionsResponse),
        (status = 400, description = "Invalid interval parameter", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    ),
    tag = "connections-controller"
))]
pub async fn list_connections_handler(
    crate::tenant::TenantId(tenant_id): crate::tenant::TenantId,
    State(pool): State<PgPool>,
    State(cipher): State<Arc<dyn CredentialCipher>>,
    Query(params): Query<ListConnectionsQuery>,
) -> Result<Json<ListConnectionsResponse>, (StatusCode, Json<Value>)> {
    // Create service with repository
    let repository = Arc::new(ConnectionRepository::new(pool.clone(), cipher.clone()));
    let service = ConnectionService::new(repository.clone());

    match service
        .list_connections(&tenant_id, params.integration_id, params.status)
        .await
    {
        Ok(mut connections) => {
            // Optionally fetch rate limit stats
            if params.include_rate_limit_stats && !connections.is_empty() {
                let interval = if params.interval.is_empty() {
                    "24h"
                } else {
                    &params.interval
                };

                // Validate interval
                if let Err(crate::service::rate_limits::ServiceError::DatabaseError(msg)) =
                    RateLimitService::parse_interval(interval)
                {
                    return Err((
                        StatusCode::BAD_REQUEST,
                        Json(json!({
                            "success": false,
                            "error": "INVALID_INTERVAL",
                            "message": msg
                        })),
                    ));
                }

                let connection_ids: Vec<String> =
                    connections.iter().map(|c| c.id.clone()).collect();
                let rate_limit_service = RateLimitService::with_db_pool(repository, pool);

                match rate_limit_service
                    .get_period_stats_for_connections(&tenant_id, &connection_ids, interval)
                    .await
                {
                    Ok(stats_map) => {
                        for conn in &mut connections {
                            conn.rate_limit_stats = stats_map.get(&conn.id).cloned();
                        }
                    }
                    Err(e) => {
                        // Log error but don't fail the request - stats are optional
                        tracing::warn!("Failed to fetch rate limit stats: {:?}", e);
                    }
                }
            }

            let count = connections.len();
            Ok(Json(ListConnectionsResponse {
                success: true,
                connections,
                count,
            }))
        }
        Err(ServiceError::DatabaseError(msg)) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({
                "success": false,
                "error": "Failed to fetch connections",
                "message": msg
            })),
        )),
        Err(_) => unreachable!("List should only return DatabaseError"),
    }
}

/// Get a single connection by ID
/// SECURITY: Does NOT return connection_parameters field
#[cfg_attr(feature = "utoipa", utoipa::path(
    get,
    path = "/api/runtime/connections/{id}",
    params(
        ("id" = String, Path, description = "Connection ID")
    ),
    responses(
        (status = 200, description = "Connection details (without sensitive connection_parameters)", body = ConnectionResponse),
        (status = 404, description = "Connection not found", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    ),
    tag = "connections-controller"
))]
pub async fn get_connection_handler(
    crate::tenant::TenantId(tenant_id): crate::tenant::TenantId,
    State(pool): State<PgPool>,
    State(cipher): State<Arc<dyn CredentialCipher>>,
    Path(id): Path<String>,
) -> Result<Json<ConnectionResponse>, (StatusCode, Json<Value>)> {
    // Create service with repository
    let repository = Arc::new(ConnectionRepository::new(pool, cipher.clone()));
    let service = ConnectionService::new(repository);

    match service.get_connection(&id, &tenant_id).await {
        Ok(connection) => Ok(Json(ConnectionResponse {
            success: true,
            connection,
        })),
        Err(ServiceError::NotFound(msg)) => Err((
            StatusCode::NOT_FOUND,
            Json(json!({
                "success": false,
                "error": msg,
                "message": Value::Null
            })),
        )),
        Err(ServiceError::DatabaseError(msg)) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({
                "success": false,
                "error": "Failed to fetch connection",
                "message": msg
            })),
        )),
        Err(_) => unreachable!("Get should only return NotFound or DatabaseError"),
    }
}

/// Update a connection
#[cfg_attr(feature = "utoipa", utoipa::path(
    put,
    path = "/api/runtime/connections/{id}",
    request_body = UpdateConnectionRequest,
    params(
        ("id" = String, Path, description = "Connection ID")
    ),
    responses(
        (status = 200, description = "Connection updated successfully", body = ConnectionResponse),
        (status = 404, description = "Connection not found", body = ErrorResponse),
        (status = 409, description = "Connection title already exists", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    ),
    tag = "connections-controller"
))]
pub async fn update_connection_handler(
    crate::tenant::TenantId(tenant_id): crate::tenant::TenantId,
    State(pool): State<PgPool>,
    State(cipher): State<Arc<dyn CredentialCipher>>,
    Path(id): Path<String>,
    Json(payload): Json<UpdateConnectionRequest>,
) -> Result<Json<ConnectionResponse>, (StatusCode, Json<Value>)> {
    // Create service with repository
    let repository = Arc::new(ConnectionRepository::new(pool, cipher.clone()));
    let service = ConnectionService::new(repository);

    match service.update_connection(&id, &tenant_id, payload).await {
        Ok(connection) => Ok(Json(ConnectionResponse {
            success: true,
            connection,
        })),
        Err(ServiceError::ValidationError(msg)) => Err((
            StatusCode::BAD_REQUEST,
            Json(json!({
                "success": false,
                "error": msg,
                "message": Value::Null
            })),
        )),
        Err(ServiceError::NotFound(msg)) => Err((
            StatusCode::NOT_FOUND,
            Json(json!({
                "success": false,
                "error": msg,
                "message": Value::Null
            })),
        )),
        Err(ServiceError::Conflict(msg)) => Err((
            StatusCode::CONFLICT,
            Json(json!({
                "success": false,
                "error": msg,
                "message": Value::Null
            })),
        )),
        Err(ServiceError::DatabaseError(msg)) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({
                "success": false,
                "error": "Failed to update connection",
                "message": msg
            })),
        )),
    }
}

/// Delete a connection
#[cfg_attr(feature = "utoipa", utoipa::path(
    delete,
    path = "/api/runtime/connections/{id}",
    params(
        ("id" = String, Path, description = "Connection ID")
    ),
    responses(
        (status = 200, description = "Connection deleted successfully", body = DeleteConnectionResponse),
        (status = 404, description = "Connection not found", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    ),
    tag = "connections-controller"
))]
pub async fn delete_connection_handler(
    crate::tenant::TenantId(tenant_id): crate::tenant::TenantId,
    State(pool): State<PgPool>,
    State(cipher): State<Arc<dyn CredentialCipher>>,
    Path(id): Path<String>,
) -> Result<Json<DeleteConnectionResponse>, (StatusCode, Json<Value>)> {
    // Create service with repository
    let repository = Arc::new(ConnectionRepository::new(pool, cipher.clone()));
    let service = ConnectionService::new(repository);

    match service.delete_connection(&id, &tenant_id).await {
        Ok(()) => Ok(Json(DeleteConnectionResponse {
            success: true,
            message: "Connection deleted successfully".to_string(),
        })),
        Err(ServiceError::NotFound(msg)) => Err((
            StatusCode::NOT_FOUND,
            Json(json!({
                "success": false,
                "error": msg,
                "message": Value::Null
            })),
        )),
        Err(ServiceError::DatabaseError(msg)) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({
                "success": false,
                "error": "Failed to delete connection",
                "message": msg
            })),
        )),
        Err(_) => unreachable!("Delete should only return NotFound or DatabaseError"),
    }
}

/// Get connections by operator name
///
/// Automatically searches for connections that match the operator using:
/// - Direct match: connection_type = operatorName (case-insensitive)
/// - Integration match: integration_id IN operator.integrationIds
///
/// For example, "Shopify" operator finds connections with:
/// - connection_type = "shopify" (direct Shopify connections)
/// - integration_id = "shopify_access_token" (HTTP connections for Shopify)
///
/// The operator's supported integration_ids are automatically looked up from the operator registry.
///
/// SECURITY: Does NOT return connection_parameters field
#[cfg_attr(feature = "utoipa", utoipa::path(
    get,
    path = "/api/runtime/connections/operator/{operatorName}",
    params(
        ("operatorName" = String, Path, description = "Operator name (e.g., 'HTTP', 'Shopify', 'SFTP')"),
        ("status" = Option<String>, Query, description = "Filter by status (UNKNOWN, ACTIVE, REQUIRES_RECONNECTION, INVALID_CREDENTIALS)")
    ),
    responses(
        (status = 200, description = "List of connections for the operator (without sensitive connection_parameters)", body = ListConnectionsResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    ),
    tag = "connections-controller"
))]
pub async fn get_connections_by_operator_handler(
    crate::tenant::TenantId(tenant_id): crate::tenant::TenantId,
    State(pool): State<PgPool>,
    State(cipher): State<Arc<dyn CredentialCipher>>,
    Path(operator_name): Path<String>,
    Query(params): Query<ListConnectionsQuery>,
) -> Result<Json<ListConnectionsResponse>, (StatusCode, Json<Value>)> {
    // Create service with repository
    let repository = Arc::new(ConnectionRepository::new(pool, cipher.clone()));
    let service = ConnectionService::new(repository);

    match service
        .list_connections_by_operator(&tenant_id, &operator_name, params.status)
        .await
    {
        Ok(connections) => {
            let count = connections.len();
            Ok(Json(ListConnectionsResponse {
                success: true,
                connections,
                count,
            }))
        }
        Err(ServiceError::DatabaseError(msg)) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({
                "success": false,
                "error": "Failed to fetch connections",
                "message": msg
            })),
        )),
        Err(_) => unreachable!("List by operator should only return DatabaseError"),
    }
}

// ============================================================================
// Connection Types Handlers (Schema Discovery)
// ============================================================================

use crate::util::rate_limit_defaults::get_default_rate_limit_config;
use runtara_dsl::agent_meta::{find_connection_type, get_all_connection_types};

/// Convert ConnectionTypeMeta to ConnectionTypeDto
fn meta_to_dto(meta: &runtara_dsl::agent_meta::ConnectionTypeMeta) -> ConnectionTypeDto {
    ConnectionTypeDto {
        integration_id: meta.integration_id.to_string(),
        display_name: meta.display_name.to_string(),
        description: meta.description.map(|s| s.to_string()),
        category: meta.category.map(|s| s.to_string()),
        fields: meta
            .fields
            .iter()
            .map(|f| ConnectionFieldDto {
                name: f.name.to_string(),
                type_name: f.type_name.to_string(),
                is_optional: f.is_optional,
                display_name: f.display_name.map(|s| s.to_string()),
                description: f.description.map(|s| s.to_string()),
                placeholder: f.placeholder.map(|s| s.to_string()),
                default_value: f.default_value.map(|s| s.to_string()),
                is_secret: f.is_secret,
            })
            .collect(),
        default_rate_limit_config: get_default_rate_limit_config(meta.integration_id),
        oauth_config: meta.oauth_config.map(|c| OAuthConfigDto {
            auth_url: c.auth_url.to_string(),
            token_url: c.token_url.to_string(),
            default_scopes: c.default_scopes.to_string(),
        }),
    }
}

/// List all available connection types
///
/// Returns all registered connection types with their parameter schemas.
/// This endpoint is used by the frontend to dynamically generate connection forms.
#[cfg_attr(feature = "utoipa", utoipa::path(
    get,
    path = "/api/runtime/connections/types",
    responses(
        (status = 200, description = "List of all connection types with their schemas", body = ListConnectionTypesResponse)
    ),
    tag = "connections-controller"
))]
pub async fn list_connection_types_handler() -> Json<ListConnectionTypesResponse> {
    let connection_types: Vec<ConnectionTypeDto> =
        get_all_connection_types().map(meta_to_dto).collect();
    let count = connection_types.len();

    Json(ListConnectionTypesResponse {
        success: true,
        connection_types,
        count,
    })
}

/// List all connection categories
///
/// Returns the canonical list of connection categories with display names and descriptions.
/// Used by the frontend to populate category filters and grouping UI.
#[cfg_attr(feature = "utoipa", utoipa::path(
    get,
    path = "/api/runtime/connections/categories",
    responses(
        (status = 200, description = "List of all connection categories", body = ListConnectionCategoriesResponse)
    ),
    tag = "connections-controller"
))]
pub async fn list_connection_categories_handler() -> Json<ListConnectionCategoriesResponse> {
    let categories: Vec<ConnectionCategoryDto> = ConnectionCategory::ALL
        .iter()
        .map(|&cat| ConnectionCategoryDto::from(cat))
        .collect();
    let count = categories.len();

    Json(ListConnectionCategoriesResponse {
        success: true,
        categories,
        count,
    })
}

/// List all connection auth types
///
/// Returns the canonical list of authentication / credential types.
/// Used by the frontend to populate auth type selectors when creating connections.
#[cfg_attr(feature = "utoipa", utoipa::path(
    get,
    path = "/api/runtime/connections/auth-types",
    responses(
        (status = 200, description = "List of all connection auth types", body = ListConnectionAuthTypesResponse)
    ),
    tag = "connections-controller"
))]
pub async fn list_connection_auth_types_handler() -> Json<ListConnectionAuthTypesResponse> {
    let auth_types: Vec<ConnectionAuthTypeDto> = ConnectionAuthType::ALL
        .iter()
        .map(|&auth| ConnectionAuthTypeDto::from(auth))
        .collect();
    let count = auth_types.len();

    Json(ListConnectionAuthTypesResponse {
        success: true,
        auth_types,
        count,
    })
}

/// Get a specific connection type by integration_id
///
/// Returns the connection type schema for the given integration_id.
/// This endpoint is used by the frontend to get the form schema for a specific connection type.
#[cfg_attr(feature = "utoipa", utoipa::path(
    get,
    path = "/api/runtime/connections/types/{integration_id}",
    params(
        ("integration_id" = String, Path, description = "Connection type integration ID (e.g., 'shopify_access_token', 'http_bearer')")
    ),
    responses(
        (status = 200, description = "Connection type with its schema", body = ConnectionTypeResponse),
        (status = 404, description = "Connection type not found", body = ErrorResponse)
    ),
    tag = "connections-controller"
))]
pub async fn get_connection_type_handler(
    Path(integration_id): Path<String>,
) -> Result<Json<ConnectionTypeResponse>, (StatusCode, Json<Value>)> {
    match find_connection_type(&integration_id) {
        Some(meta) => Ok(Json(ConnectionTypeResponse {
            success: true,
            connection_type: meta_to_dto(meta),
        })),
        None => Err((
            StatusCode::NOT_FOUND,
            Json(json!({
                "success": false,
                "error": "CONNECTION_TYPE_NOT_FOUND",
                "message": format!("Connection type '{}' not found", integration_id)
            })),
        )),
    }
}

// ============================================================================
// Runtime Connection Handler (Internal API for runtara-workflows)
// ============================================================================

/// Get connection for runtara-workflows runtime
///
/// INTERNAL ENDPOINT: Returns connection with decrypted parameters and rate limit state.
/// This endpoint is called by runtara-workflows at runtime to fetch credentials.
///
/// Path format: /api/connections/{tenant_id}/{connection_id}
/// This matches the CONNECTION_SERVICE_URL format expected by runtara-workflows.
// No OpenAPI annotation — this is an internal-only endpoint (not exposed via gateway)
pub async fn get_connection_for_runtime_handler(
    State(pool): State<PgPool>,
    State(cipher): State<Arc<dyn CredentialCipher>>,
    Path((tenant_id, connection_id)): Path<(String, String)>,
    Query(query): Query<RuntimeConnectionQuery>,
) -> Result<Json<RuntimeConnectionResponse>, (StatusCode, Json<Value>)> {
    // Build metadata from query params (tag, stepId, workflowId, instanceId)
    let metadata = {
        let mut map = serde_json::Map::new();
        if let Some(ref tag) = query.tag {
            map.insert("tag".to_string(), json!(tag));
        }
        if let Some(ref step_id) = query.step_id {
            map.insert("stepId".to_string(), json!(step_id));
        }
        if let Some(ref workflow_id) = query.workflow_id {
            map.insert("workflowId".to_string(), json!(workflow_id));
        }
        if let Some(ref instance_id) = query.instance_id {
            map.insert("instanceId".to_string(), json!(instance_id));
        }
        if map.is_empty() {
            None
        } else {
            Some(Value::Object(map))
        }
    };

    // Create services with db pool for rate limit event tracking
    let repository = Arc::new(ConnectionRepository::new(pool.clone(), cipher.clone()));
    let rate_limit_service = Arc::new(RateLimitService::with_db_pool(repository.clone(), pool));
    let service = ConnectionService::with_rate_limit_service(repository, rate_limit_service);

    match service
        .get_for_runtime(&connection_id, &tenant_id, metadata)
        .await
    {
        Ok(response) => Ok(Json(response)),
        Err(ServiceError::NotFound(msg)) => Err((
            StatusCode::NOT_FOUND,
            Json(json!({
                "success": false,
                "error": "CONNECTION_NOT_FOUND",
                "message": msg
            })),
        )),
        Err(ServiceError::DatabaseError(msg)) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({
                "success": false,
                "error": "DATABASE_ERROR",
                "message": msg
            })),
        )),
        Err(_) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({
                "success": false,
                "error": "INTERNAL_ERROR",
                "message": "An unexpected error occurred"
            })),
        )),
    }
}
