//! Internal Object Model HTTP Handlers
//!
//! These endpoints are called by compiled scenario binaries (via ureq from runtara-agents)
//! for object model CRUD operations. They have NO authentication middleware —
//! the tenant_id is passed via the X-Org-Id header without JWT validation.
//!
//! Mounted at `/api/internal/object-model/*` on the main runtara server.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::Json,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::Arc;

use super::object_model::ObjectModelState;
use crate::api::dto::object_model::{CreateSchemaRequest, FilterRequest};
use crate::api::services::object_model::{InstanceService, SchemaService, ServiceError};

// ============================================================================
// Request/Response Types (simplified for internal use)
// ============================================================================

/// Extract tenant_id from X-Org-Id header (no JWT validation)
fn extract_tenant_id(headers: &axum::http::HeaderMap) -> Result<String, (StatusCode, Json<Value>)> {
    headers
        .get("X-Org-Id")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
        .ok_or_else(|| {
            (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": "Missing X-Org-Id header"})),
            )
        })
}

#[derive(Debug, Deserialize)]
pub struct InternalCreateInstanceRequest {
    /// Schema name (required — scenarios always know the schema name)
    pub schema_name: String,
    /// Properties to store
    pub properties: Value,
}

#[derive(Debug, Serialize)]
pub struct InternalCreateInstanceResponse {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instance_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct InternalQueryInstancesRequest {
    pub schema_name: String,
    #[serde(default)]
    pub filters: HashMap<String, Value>,
    #[serde(default)]
    pub condition: Option<crate::api::dto::object_model::Condition>,
    #[serde(default = "default_limit")]
    pub limit: i64,
    #[serde(default)]
    pub offset: i64,
    /// Optional sort fields
    #[serde(rename = "sortBy", skip_serializing_if = "Option::is_none", default)]
    pub sort_by: Option<Vec<String>>,
    #[serde(rename = "sortOrder", skip_serializing_if = "Option::is_none", default)]
    pub sort_order: Option<Vec<String>>,
}

fn default_limit() -> i64 {
    100
}

#[derive(Debug, Serialize)]
pub struct InternalQueryInstancesResponse {
    pub success: bool,
    pub instances: Vec<Value>,
    pub total_count: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct InternalCheckExistsRequest {
    pub schema_name: String,
    pub filters: HashMap<String, Value>,
}

#[derive(Debug, Serialize)]
pub struct InternalCheckExistsResponse {
    pub exists: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instance_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instance: Option<Value>,
}

#[derive(Debug, Deserialize)]
pub struct InternalCreateIfNotExistsRequest {
    pub schema_name: String,
    pub match_filters: HashMap<String, Value>,
    pub data: Value,
}

#[derive(Debug, Serialize)]
pub struct InternalCreateIfNotExistsResponse {
    pub success: bool,
    pub created: bool,
    pub already_existed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instance_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct InternalUpdateInstanceRequest {
    pub data: Value,
}

#[derive(Debug, Serialize)]
pub struct InternalUpdateInstanceResponse {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instance_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct InternalGetSchemaResponse {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schema: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct InternalCreateSchemaResponse {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schema_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

// ============================================================================
// Handlers
// ============================================================================

/// POST /api/internal/object-model/instances — create an instance
pub async fn create_instance(
    headers: axum::http::HeaderMap,
    State(state): State<Arc<ObjectModelState>>,
    Json(request): Json<InternalCreateInstanceRequest>,
) -> Result<(StatusCode, Json<InternalCreateInstanceResponse>), (StatusCode, Json<Value>)> {
    let tenant_id = extract_tenant_id(&headers)?;
    let service = InstanceService::new(state.manager.clone(), state.pool.clone());

    let create_request = crate::api::dto::object_model::CreateInstanceRequest {
        schema_id: None,
        schema_name: Some(request.schema_name),
        properties: request.properties,
    };

    match service
        .create_instance(create_request, &tenant_id, None)
        .await
    {
        Ok(instance_id) => Ok((
            StatusCode::CREATED,
            Json(InternalCreateInstanceResponse {
                success: true,
                instance_id: Some(instance_id),
                error: None,
            }),
        )),
        Err(e) => Ok((
            StatusCode::OK,
            Json(InternalCreateInstanceResponse {
                success: false,
                instance_id: None,
                error: Some(e.to_string()),
            }),
        )),
    }
}

/// POST /api/internal/object-model/instances/query — query instances with filters
pub async fn query_instances(
    headers: axum::http::HeaderMap,
    State(state): State<Arc<ObjectModelState>>,
    Json(request): Json<InternalQueryInstancesRequest>,
) -> Result<(StatusCode, Json<InternalQueryInstancesResponse>), (StatusCode, Json<Value>)> {
    let tenant_id = extract_tenant_id(&headers)?;
    let service = InstanceService::new(state.manager.clone(), state.pool.clone());

    // If there are simple filters but no condition, convert simple filters to a condition
    if request.condition.is_some() || request.filters.is_empty() {
        // Use the filter_instances_by_schema service method with condition
        let filter_request = FilterRequest {
            offset: request.offset,
            limit: request.limit,
            condition: request.condition,
            sort_by: request.sort_by,
            sort_order: request.sort_order,
        };

        match service
            .filter_instances_by_schema(&tenant_id, &request.schema_name, filter_request, None)
            .await
        {
            Ok((instances, total_count)) => {
                let instance_values: Vec<Value> =
                    instances.into_iter().map(instance_to_flat_value).collect();

                Ok((
                    StatusCode::OK,
                    Json(InternalQueryInstancesResponse {
                        success: true,
                        instances: instance_values,
                        total_count,
                        error: None,
                    }),
                ))
            }
            Err(e) => Ok((
                StatusCode::OK,
                Json(InternalQueryInstancesResponse {
                    success: false,
                    instances: vec![],
                    total_count: 0,
                    error: Some(e.to_string()),
                }),
            )),
        }
    } else {
        // Convert simple filters to an AND condition with EQ operations
        let condition = simple_filters_to_condition(&request.filters);

        let filter_request = FilterRequest {
            offset: request.offset,
            limit: request.limit,
            condition: Some(condition),
            sort_by: request.sort_by,
            sort_order: request.sort_order,
        };

        match service
            .filter_instances_by_schema(&tenant_id, &request.schema_name, filter_request, None)
            .await
        {
            Ok((instances, total_count)) => {
                let instance_values: Vec<Value> =
                    instances.into_iter().map(instance_to_flat_value).collect();

                Ok((
                    StatusCode::OK,
                    Json(InternalQueryInstancesResponse {
                        success: true,
                        instances: instance_values,
                        total_count,
                        error: None,
                    }),
                ))
            }
            Err(e) => Ok((
                StatusCode::OK,
                Json(InternalQueryInstancesResponse {
                    success: false,
                    instances: vec![],
                    total_count: 0,
                    error: Some(e.to_string()),
                }),
            )),
        }
    }
}

/// POST /api/internal/object-model/instances/exists — check if instance exists
pub async fn check_instance_exists(
    headers: axum::http::HeaderMap,
    State(state): State<Arc<ObjectModelState>>,
    Json(request): Json<InternalCheckExistsRequest>,
) -> Result<(StatusCode, Json<InternalCheckExistsResponse>), (StatusCode, Json<Value>)> {
    let tenant_id = extract_tenant_id(&headers)?;
    let service = InstanceService::new(state.manager.clone(), state.pool.clone());

    let condition = simple_filters_to_condition(&request.filters);
    let filter_request = FilterRequest {
        offset: 0,
        limit: 1,
        condition: Some(condition),
        sort_by: None,
        sort_order: None,
    };

    match service
        .filter_instances_by_schema(&tenant_id, &request.schema_name, filter_request, None)
        .await
    {
        Ok((instances, _)) => {
            if let Some(instance) = instances.into_iter().next() {
                let flat = instance_to_flat_value(instance);
                let instance_id = flat.get("id").and_then(|v| v.as_str()).map(String::from);
                Ok((
                    StatusCode::OK,
                    Json(InternalCheckExistsResponse {
                        exists: true,
                        instance_id,
                        instance: Some(flat),
                    }),
                ))
            } else {
                Ok((
                    StatusCode::OK,
                    Json(InternalCheckExistsResponse {
                        exists: false,
                        instance_id: None,
                        instance: None,
                    }),
                ))
            }
        }
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": e.to_string()})),
        )),
    }
}

/// POST /api/internal/object-model/instances/create-if-not-exists
pub async fn create_if_not_exists(
    headers: axum::http::HeaderMap,
    State(state): State<Arc<ObjectModelState>>,
    Json(request): Json<InternalCreateIfNotExistsRequest>,
) -> Result<(StatusCode, Json<InternalCreateIfNotExistsResponse>), (StatusCode, Json<Value>)> {
    let tenant_id = extract_tenant_id(&headers)?;
    let service = InstanceService::new(state.manager.clone(), state.pool.clone());

    // First check existence
    let condition = simple_filters_to_condition(&request.match_filters);
    let filter_request = FilterRequest {
        offset: 0,
        limit: 1,
        condition: Some(condition),
        sort_by: None,
        sort_order: None,
    };

    let exists_result = service
        .filter_instances_by_schema(&tenant_id, &request.schema_name, filter_request, None)
        .await;

    match exists_result {
        Ok((instances, _)) => {
            if let Some(instance) = instances.into_iter().next() {
                // Already exists
                let instance_id = instance.id.clone();
                Ok((
                    StatusCode::OK,
                    Json(InternalCreateIfNotExistsResponse {
                        success: true,
                        created: false,
                        already_existed: true,
                        instance_id: Some(instance_id),
                        error: None,
                    }),
                ))
            } else {
                // Create new
                let create_request = crate::api::dto::object_model::CreateInstanceRequest {
                    schema_id: None,
                    schema_name: Some(request.schema_name),
                    properties: request.data,
                };

                match service
                    .create_instance(create_request, &tenant_id, None)
                    .await
                {
                    Ok(instance_id) => Ok((
                        StatusCode::CREATED,
                        Json(InternalCreateIfNotExistsResponse {
                            success: true,
                            created: true,
                            already_existed: false,
                            instance_id: Some(instance_id),
                            error: None,
                        }),
                    )),
                    Err(e) => Ok((
                        StatusCode::OK,
                        Json(InternalCreateIfNotExistsResponse {
                            success: false,
                            created: false,
                            already_existed: false,
                            instance_id: None,
                            error: Some(e.to_string()),
                        }),
                    )),
                }
            }
        }
        Err(e) => Ok((
            StatusCode::OK,
            Json(InternalCreateIfNotExistsResponse {
                success: false,
                created: false,
                already_existed: false,
                instance_id: None,
                error: Some(e.to_string()),
            }),
        )),
    }
}

/// PUT /api/internal/object-model/instances/{schema_name}/{id} — update instance
pub async fn update_instance(
    headers: axum::http::HeaderMap,
    State(state): State<Arc<ObjectModelState>>,
    Path((schema_name, instance_id)): Path<(String, String)>,
    Json(request): Json<InternalUpdateInstanceRequest>,
) -> Result<(StatusCode, Json<InternalUpdateInstanceResponse>), (StatusCode, Json<Value>)> {
    let tenant_id = extract_tenant_id(&headers)?;

    // Use the object store directly via the manager (by schema name, not schema ID)
    let store = crate::api::services::object_model::get_store(
        &state.manager,
        &state.pool,
        None,
        &tenant_id,
    )
    .await
    .map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": e.to_string()})),
        )
    })?;

    match store
        .update_instance(&schema_name, &instance_id, request.data)
        .await
    {
        Ok(()) => Ok((
            StatusCode::OK,
            Json(InternalUpdateInstanceResponse {
                success: true,
                instance_id: Some(instance_id),
                error: None,
            }),
        )),
        Err(e) => Ok((
            StatusCode::OK,
            Json(InternalUpdateInstanceResponse {
                success: false,
                instance_id: None,
                error: Some(e.to_string()),
            }),
        )),
    }
}

/// GET /api/internal/object-model/schemas/{name} — get schema by name
pub async fn get_schema(
    headers: axum::http::HeaderMap,
    State(state): State<Arc<ObjectModelState>>,
    Path(name): Path<String>,
) -> Result<(StatusCode, Json<InternalGetSchemaResponse>), (StatusCode, Json<Value>)> {
    let tenant_id = extract_tenant_id(&headers)?;
    let service = SchemaService::new(state.manager.clone(), state.pool.clone());

    match service.get_schema_by_name(&name, &tenant_id, None).await {
        Ok(schema) => Ok((
            StatusCode::OK,
            Json(InternalGetSchemaResponse {
                success: true,
                schema: Some(serde_json::to_value(schema).unwrap_or(json!(null))),
                error: None,
            }),
        )),
        Err(ServiceError::NotFound(_)) => Ok((
            StatusCode::OK,
            Json(InternalGetSchemaResponse {
                success: false,
                schema: None,
                error: None,
            }),
        )),
        Err(e) => Ok((
            StatusCode::OK,
            Json(InternalGetSchemaResponse {
                success: false,
                schema: None,
                error: Some(e.to_string()),
            }),
        )),
    }
}

/// POST /api/internal/object-model/schemas — create schema
pub async fn create_schema(
    headers: axum::http::HeaderMap,
    State(state): State<Arc<ObjectModelState>>,
    Json(request): Json<CreateSchemaRequest>,
) -> Result<(StatusCode, Json<InternalCreateSchemaResponse>), (StatusCode, Json<Value>)> {
    let tenant_id = extract_tenant_id(&headers)?;
    let service = SchemaService::new(state.manager.clone(), state.pool.clone());

    match service.create_schema(request, &tenant_id, None).await {
        Ok(schema_id) => Ok((
            StatusCode::CREATED,
            Json(InternalCreateSchemaResponse {
                success: true,
                schema_id: Some(schema_id),
                error: None,
            }),
        )),
        Err(ServiceError::Conflict(_)) => {
            // Schema already exists — not an error for internal callers
            Ok((
                StatusCode::OK,
                Json(InternalCreateSchemaResponse {
                    success: true,
                    schema_id: None,
                    error: None,
                }),
            ))
        }
        Err(e) => Ok((
            StatusCode::OK,
            Json(InternalCreateSchemaResponse {
                success: false,
                schema_id: None,
                error: Some(e.to_string()),
            }),
        )),
    }
}

// ============================================================================
// Helpers
// ============================================================================

/// Convert an Instance DTO to a flat JSON value (properties merged at top level)
fn instance_to_flat_value(instance: crate::api::dto::object_model::Instance) -> Value {
    let mut map = serde_json::Map::new();
    map.insert("id".to_string(), Value::String(instance.id));
    map.insert("createdAt".to_string(), Value::String(instance.created_at));
    map.insert("updatedAt".to_string(), Value::String(instance.updated_at));
    if let Some(schema_name) = instance.schema_name {
        map.insert("schemaName".to_string(), Value::String(schema_name));
    }
    // Merge properties into top level
    if let Value::Object(props) = instance.properties {
        for (k, v) in props {
            map.insert(k, v);
        }
    }
    Value::Object(map)
}

/// Convert simple key-value filters to an AND condition with EQ operations.
///
/// Produces the same Condition structure that runtara-object-store expects:
/// ```json
/// {"op": "AND", "arguments": [
///   {"op": "EQ", "arguments": [{"op": "FIELD", "arguments": ["key"]}, value]},
///   ...
/// ]}
/// ```
fn simple_filters_to_condition(
    filters: &HashMap<String, Value>,
) -> crate::api::dto::object_model::Condition {
    let eq_conditions: Vec<Value> = filters
        .iter()
        .map(|(key, value)| {
            json!({
                "op": "EQ",
                "arguments": [key, value]
            })
        })
        .collect();

    if eq_conditions.len() == 1 {
        // Single filter — no need for AND wrapper
        serde_json::from_value(eq_conditions.into_iter().next().unwrap()).unwrap_or_else(|_| {
            crate::api::dto::object_model::Condition {
                op: "EQ".to_string(),
                arguments: None,
            }
        })
    } else {
        crate::api::dto::object_model::Condition {
            op: "AND".to_string(),
            arguments: Some(eq_conditions),
        }
    }
}
