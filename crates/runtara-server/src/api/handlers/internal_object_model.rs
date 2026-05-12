//! Internal Object Model HTTP Handlers
//!
//! These endpoints are called by compiled workflow binaries (via ureq from runtara-agents)
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
use crate::api::dto::object_model::{
    CreateSchemaRequest, FilterRequest, OrderByEntry, ScoreExpression,
};
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
    /// Schema name (required — workflows always know the schema name)
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
    /// Optional computed score column. Used with `orderBy` for vector KNN.
    #[serde(
        rename = "scoreExpression",
        alias = "score_expression",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub score_expression: Option<ScoreExpression>,
    /// Optional structured order-by entries. When set, supersedes sortBy/sortOrder.
    #[serde(
        rename = "orderBy",
        alias = "order_by",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub order_by: Option<Vec<OrderByEntry>>,
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

#[derive(Debug, Deserialize)]
pub struct InternalDeleteInstanceRequest {
    pub schema_name: String,
    pub instance_id: String,
}

#[derive(Debug, Serialize)]
pub struct InternalDeleteInstanceResponse {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum InternalBulkConflictMode {
    #[default]
    Error,
    Skip,
    Upsert,
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum InternalBulkValidationMode {
    #[default]
    Stop,
    Skip,
}

#[derive(Debug, Deserialize)]
pub struct InternalBulkCreateRequest {
    pub schema_name: String,

    /// Object form — array of JSON objects, one per record.
    #[serde(default)]
    pub instances: Option<Vec<Value>>,

    /// Columnar form — column names (paired with `rows`).
    #[serde(default)]
    pub columns: Option<Vec<String>>,

    /// Columnar form — each row is an array of values aligned to `columns`.
    #[serde(default)]
    pub rows: Option<Vec<Vec<Value>>>,

    /// Columnar form — fields merged into every row (row cells override on overlap).
    #[serde(default)]
    pub constants: serde_json::Map<String, Value>,

    /// Columnar form — when true, empty strings in non-string columns are
    /// nullified before validation.
    #[serde(default)]
    pub nullify_empty_strings: bool,

    #[serde(default)]
    pub on_conflict: InternalBulkConflictMode,
    #[serde(default)]
    pub on_error: InternalBulkValidationMode,
    #[serde(default)]
    pub conflict_columns: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct InternalBulkRowError {
    pub index: usize,
    pub reason: String,
}

#[derive(Debug, Serialize)]
pub struct InternalBulkCreateResponse {
    pub success: bool,
    pub created_count: i64,
    pub skipped_count: i64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub errors: Vec<InternalBulkRowError>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "mode", rename_all = "camelCase")]
pub enum InternalBulkUpdateMode {
    ByCondition {
        properties: Value,
        condition: crate::api::dto::object_model::Condition,
    },
    ByIds {
        updates: Vec<InternalBulkUpdateByIdEntry>,
    },
}

#[derive(Debug, Deserialize)]
pub struct InternalBulkUpdateByIdEntry {
    pub id: String,
    pub properties: Value,
}

#[derive(Debug, Deserialize)]
pub struct InternalBulkUpdateRequest {
    pub schema_name: String,
    #[serde(flatten)]
    pub mode: InternalBulkUpdateMode,
}

#[derive(Debug, Serialize)]
pub struct InternalBulkUpdateResponse {
    pub success: bool,
    pub updated_count: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct InternalBulkDeleteRequest {
    pub schema_name: String,
    #[serde(default)]
    pub ids: Option<Vec<String>>,
    #[serde(default)]
    pub condition: Option<crate::api::dto::object_model::Condition>,
}

#[derive(Debug, Serialize)]
pub struct InternalBulkDeleteResponse {
    pub success: bool,
    pub deleted_count: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct InternalAggregateRequest {
    pub schema_name: String,
    #[serde(default)]
    pub condition: Option<crate::api::dto::object_model::Condition>,
    #[serde(default, alias = "groupBy")]
    pub group_by: Vec<String>,
    pub aggregates: Vec<crate::api::dto::object_model::AggregateSpec>,
    #[serde(default, alias = "orderBy")]
    pub order_by: Vec<crate::api::dto::object_model::AggregateOrderBy>,
    #[serde(default)]
    pub limit: Option<i64>,
    #[serde(default)]
    pub offset: Option<i64>,
}

#[derive(Debug, Serialize)]
pub struct InternalAggregateResponse {
    pub success: bool,
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Value>>,
    pub group_count: i64,
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
    let service = InstanceService::new(state.manager.clone(), state.connections.clone());

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
    let service = InstanceService::new(state.manager.clone(), state.connections.clone());

    // If there are simple filters but no condition, convert simple filters to a condition
    if request.condition.is_some() || request.filters.is_empty() {
        // Use the filter_instances_by_schema service method with condition
        let filter_request = FilterRequest {
            offset: request.offset,
            limit: request.limit,
            condition: request.condition,
            sort_by: request.sort_by,
            sort_order: request.sort_order,
            score_expression: request.score_expression,
            order_by: request.order_by,
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
            score_expression: request.score_expression,
            order_by: request.order_by,
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
    let service = InstanceService::new(state.manager.clone(), state.connections.clone());

    let condition = simple_filters_to_condition(&request.filters);
    let filter_request = FilterRequest {
        offset: 0,
        limit: 1,
        condition: Some(condition),
        sort_by: None,
        sort_order: None,
        score_expression: None,
        order_by: None,
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
    let service = InstanceService::new(state.manager.clone(), state.connections.clone());

    // First check existence
    let condition = simple_filters_to_condition(&request.match_filters);
    let filter_request = FilterRequest {
        offset: 0,
        limit: 1,
        condition: Some(condition),
        sort_by: None,
        sort_order: None,
        score_expression: None,
        order_by: None,
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
    let store =
        crate::api::services::object_model::get_store(&state.manager, None, None, &tenant_id)
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

/// POST /api/internal/object-model/instances/delete — delete a single instance by schema name + id.
///
/// Uses POST (not DELETE) so the body can carry `schema_name` + `instance_id` in the
/// same shape as the other internal mutation endpoints.
pub async fn delete_instance(
    headers: axum::http::HeaderMap,
    State(state): State<Arc<ObjectModelState>>,
    Json(request): Json<InternalDeleteInstanceRequest>,
) -> Result<(StatusCode, Json<InternalDeleteInstanceResponse>), (StatusCode, Json<Value>)> {
    let tenant_id = extract_tenant_id(&headers)?;

    let store =
        crate::api::services::object_model::get_store(&state.manager, None, None, &tenant_id)
            .await
            .map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({"error": e.to_string()})),
                )
            })?;

    match store
        .delete_instance(&request.schema_name, &request.instance_id)
        .await
    {
        Ok(()) => Ok((
            StatusCode::OK,
            Json(InternalDeleteInstanceResponse {
                success: true,
                error: None,
            }),
        )),
        Err(e) => Ok((
            StatusCode::OK,
            Json(InternalDeleteInstanceResponse {
                success: false,
                error: Some(e.to_string()),
            }),
        )),
    }
}

/// POST /api/internal/object-model/instances/bulk-create
pub async fn bulk_create_instances(
    headers: axum::http::HeaderMap,
    State(state): State<Arc<ObjectModelState>>,
    Json(request): Json<InternalBulkCreateRequest>,
) -> Result<(StatusCode, Json<InternalBulkCreateResponse>), (StatusCode, Json<Value>)> {
    let tenant_id = extract_tenant_id(&headers)?;

    let store =
        crate::api::services::object_model::get_store(&state.manager, None, None, &tenant_id)
            .await
            .map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({"error": e.to_string()})),
                )
            })?;

    use runtara_object_store::{
        BulkCreateOptions, ConflictMode as StoreConflictMode, ValidationMode,
    };

    let conflict_mode = match request.on_conflict {
        InternalBulkConflictMode::Error => StoreConflictMode::Error,
        InternalBulkConflictMode::Skip => {
            if request.conflict_columns.is_empty() {
                return Ok((
                    StatusCode::OK,
                    Json(InternalBulkCreateResponse {
                        success: false,
                        created_count: 0,
                        skipped_count: 0,
                        errors: vec![],
                        error: Some(
                            "`conflict_columns` is required when `on_conflict` is 'skip'"
                                .to_string(),
                        ),
                    }),
                ));
            }
            StoreConflictMode::Skip {
                conflict_columns: request.conflict_columns,
            }
        }
        InternalBulkConflictMode::Upsert => {
            if request.conflict_columns.is_empty() {
                return Ok((
                    StatusCode::OK,
                    Json(InternalBulkCreateResponse {
                        success: false,
                        created_count: 0,
                        skipped_count: 0,
                        errors: vec![],
                        error: Some(
                            "`conflict_columns` is required when `on_conflict` is 'upsert'"
                                .to_string(),
                        ),
                    }),
                ));
            }
            StoreConflictMode::Upsert {
                conflict_columns: request.conflict_columns,
            }
        }
    };

    let validation_mode = match request.on_error {
        InternalBulkValidationMode::Stop => ValidationMode::Stop,
        InternalBulkValidationMode::Skip => ValidationMode::Skip,
    };

    let opts = BulkCreateOptions {
        conflict_mode,
        validation_mode,
    };

    // Resolve schema so the normalizer can honor `nullify_empty_strings` per
    // column type. A missing schema will surface naturally from
    // `create_instances_extended` too, but catching it here gives a cleaner
    // error for columnar payloads whose row/column shape is schema-aware.
    let schema = match store.get_schema(&request.schema_name).await {
        Ok(Some(s)) => s,
        Ok(None) => {
            return Ok((
                StatusCode::OK,
                Json(InternalBulkCreateResponse {
                    success: false,
                    created_count: 0,
                    skipped_count: 0,
                    errors: vec![],
                    error: Some(format!("Schema '{}' not found", request.schema_name)),
                }),
            ));
        }
        Err(e) => {
            return Ok((
                StatusCode::OK,
                Json(InternalBulkCreateResponse {
                    success: false,
                    created_count: 0,
                    skipped_count: 0,
                    errors: vec![],
                    error: Some(e.to_string()),
                }),
            ));
        }
    };

    let instances = match crate::api::services::object_model::normalize_bulk_create_inputs(
        request.instances.as_deref(),
        request.columns.as_deref(),
        request.rows.as_deref(),
        &request.constants,
        request.nullify_empty_strings,
        &schema,
    ) {
        Ok(v) => v,
        Err(e) => {
            return Ok((
                StatusCode::OK,
                Json(InternalBulkCreateResponse {
                    success: false,
                    created_count: 0,
                    skipped_count: 0,
                    errors: vec![],
                    error: Some(e.to_string()),
                }),
            ));
        }
    };

    match store
        .create_instances_extended(&request.schema_name, instances, opts)
        .await
    {
        Ok(result) => Ok((
            StatusCode::OK,
            Json(InternalBulkCreateResponse {
                success: true,
                created_count: result.created_count,
                skipped_count: result.skipped_count,
                errors: result
                    .errors
                    .into_iter()
                    .map(|e| InternalBulkRowError {
                        index: e.index,
                        reason: e.reason,
                    })
                    .collect(),
                error: None,
            }),
        )),
        Err(e) => Ok((
            StatusCode::OK,
            Json(InternalBulkCreateResponse {
                success: false,
                created_count: 0,
                skipped_count: 0,
                errors: vec![],
                error: Some(e.to_string()),
            }),
        )),
    }
}

/// POST /api/internal/object-model/instances/bulk-update — supports byCondition and byIds modes.
pub async fn bulk_update_instances(
    headers: axum::http::HeaderMap,
    State(state): State<Arc<ObjectModelState>>,
    Json(request): Json<InternalBulkUpdateRequest>,
) -> Result<(StatusCode, Json<InternalBulkUpdateResponse>), (StatusCode, Json<Value>)> {
    let tenant_id = extract_tenant_id(&headers)?;

    let store =
        crate::api::services::object_model::get_store(&state.manager, None, None, &tenant_id)
            .await
            .map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({"error": e.to_string()})),
                )
            })?;

    let result = match request.mode {
        InternalBulkUpdateMode::ByCondition {
            properties,
            condition,
        } => {
            store
                .update_instances(&request.schema_name, properties, condition.into())
                .await
        }
        InternalBulkUpdateMode::ByIds { updates } => {
            let pairs: Vec<(String, Value)> =
                updates.into_iter().map(|u| (u.id, u.properties)).collect();
            store
                .update_instances_by_ids(&request.schema_name, pairs)
                .await
        }
    };

    match result {
        Ok(count) => Ok((
            StatusCode::OK,
            Json(InternalBulkUpdateResponse {
                success: true,
                updated_count: count,
                error: None,
            }),
        )),
        Err(e) => Ok((
            StatusCode::OK,
            Json(InternalBulkUpdateResponse {
                success: false,
                updated_count: 0,
                error: Some(e.to_string()),
            }),
        )),
    }
}

/// POST /api/internal/object-model/instances/bulk-delete — accepts either ids or condition.
pub async fn bulk_delete_instances(
    headers: axum::http::HeaderMap,
    State(state): State<Arc<ObjectModelState>>,
    Json(request): Json<InternalBulkDeleteRequest>,
) -> Result<(StatusCode, Json<InternalBulkDeleteResponse>), (StatusCode, Json<Value>)> {
    let tenant_id = extract_tenant_id(&headers)?;

    let store =
        crate::api::services::object_model::get_store(&state.manager, None, None, &tenant_id)
            .await
            .map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({"error": e.to_string()})),
                )
            })?;

    let condition = match (request.ids, request.condition) {
        (Some(ids), _) if !ids.is_empty() => {
            let id_values: Vec<Value> = ids.into_iter().map(Value::String).collect();
            runtara_object_store::Condition::r#in("id", id_values)
        }
        (_, Some(cond)) => cond.into(),
        _ => {
            return Ok((
                StatusCode::OK,
                Json(InternalBulkDeleteResponse {
                    success: false,
                    deleted_count: 0,
                    error: Some("Either 'ids' or 'condition' must be provided".to_string()),
                }),
            ));
        }
    };

    match store
        .delete_instances(&request.schema_name, condition)
        .await
    {
        Ok(count) => Ok((
            StatusCode::OK,
            Json(InternalBulkDeleteResponse {
                success: true,
                deleted_count: count,
                error: None,
            }),
        )),
        Err(e) => Ok((
            StatusCode::OK,
            Json(InternalBulkDeleteResponse {
                success: false,
                deleted_count: 0,
                error: Some(e.to_string()),
            }),
        )),
    }
}

/// POST /api/internal/object-model/instances/aggregate — GROUP BY aggregates
pub async fn aggregate_instances(
    headers: axum::http::HeaderMap,
    State(state): State<Arc<ObjectModelState>>,
    Json(request): Json<InternalAggregateRequest>,
) -> Result<(StatusCode, Json<InternalAggregateResponse>), (StatusCode, Json<Value>)> {
    let tenant_id = extract_tenant_id(&headers)?;
    let service = InstanceService::new(state.manager.clone(), state.connections.clone());

    let api_req = crate::api::dto::object_model::AggregateRequest {
        condition: request.condition,
        group_by: request.group_by,
        aggregates: request.aggregates,
        order_by: request.order_by,
        limit: request.limit,
        offset: request.offset,
    };

    match service
        .aggregate_instances_by_schema(&tenant_id, &request.schema_name, api_req, None)
        .await
    {
        Ok(result) => Ok((
            StatusCode::OK,
            Json(InternalAggregateResponse {
                success: true,
                columns: result.columns,
                rows: result.rows,
                group_count: result.group_count,
                error: None,
            }),
        )),
        Err(e) => Ok((
            StatusCode::OK,
            Json(InternalAggregateResponse {
                success: false,
                columns: vec![],
                rows: vec![],
                group_count: 0,
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
    let service = SchemaService::new(state.manager.clone(), state.connections.clone());

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
    let service = SchemaService::new(state.manager.clone(), state.connections.clone());

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
    if let Some(computed) = instance.computed {
        map.insert("computed".to_string(), Value::Object(computed));
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::dto::object_model::{Instance, OrderByTarget, SortDirection};

    #[test]
    fn query_instances_request_accepts_score_expression_and_order_by() {
        let request: InternalQueryInstancesRequest = serde_json::from_value(json!({
            "schema_name": "UnspscNode",
            "scoreExpression": {
                "alias": "distance",
                "expression": {
                    "fn": "COSINE_DISTANCE",
                    "arguments": [
                        {"valueType": "reference", "value": "embedding"},
                        {"valueType": "immediate", "value": [0.1, 0.2, 0.3]}
                    ]
                }
            },
            "orderBy": [{
                "expression": {"kind": "alias", "name": "distance"},
                "direction": "ASC"
            }],
            "limit": 25
        }))
        .unwrap();

        let score = request.score_expression.unwrap();
        assert_eq!(score.alias, "distance");

        let order_by = request.order_by.unwrap();
        assert_eq!(order_by[0].direction, SortDirection::Asc);
        match &order_by[0].expression {
            OrderByTarget::Alias { name } => assert_eq!(name, "distance"),
            other => panic!("expected alias order target, got {other:?}"),
        }
    }

    #[test]
    fn instance_to_flat_value_preserves_computed_scores() {
        let mut computed = serde_json::Map::new();
        computed.insert("distance".to_string(), json!(0.125));

        let flat = instance_to_flat_value(Instance {
            id: "row-1".to_string(),
            tenant_id: "tenant-1".to_string(),
            created_at: "2026-05-11T00:00:00Z".to_string(),
            updated_at: "2026-05-11T00:00:00Z".to_string(),
            schema_id: None,
            schema_name: Some("UnspscNode".to_string()),
            properties: json!({"commodityTitle": "ball bearing"}),
            computed: Some(computed),
        });

        assert_eq!(flat["commodityTitle"], "ball bearing");
        assert_eq!(flat["computed"]["distance"], json!(0.125));
    }
}
