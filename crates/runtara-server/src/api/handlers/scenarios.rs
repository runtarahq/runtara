//! Scenario HTTP Handlers
//!
//! Thin HTTP layer that:
//! - Extracts HTTP parameters (path, query, body, headers)
//! - Validates tenant authentication
//! - Delegates business logic to ScenarioService
//! - Maps service errors to HTTP status codes
//! - Returns standardized API responses

// Allow dead code temporarily - handlers will be wired up in routing layer
#![allow(dead_code)]

use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Json, Response},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::PgPool;
use std::sync::Arc;
use tracing::instrument;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::api::dto::common::{ApiResponse, ErrorResponse};
use crate::api::dto::scenarios::{
    CheckpointMetadataDto, CompileScenarioResponse, ExecuteScenarioRequest,
    ExecuteScenarioResponse, FoldersResponse, GetDependenciesResponse, GetDependentsResponse,
    ListCheckpointsQuery, ListCheckpointsResponse, ListInstancesQuery, ListStepTypesResponse,
    MoveScenarioRequest, MoveScenarioResponse, PageScenarioDto, PageScenarioInstanceHistoryDto,
    RenameFolderRequest, RenameFolderResponse, ScenarioDependency, ScenarioDependent, ScenarioDto,
    ScenarioInstanceDto, ScenarioVersionInfoDto, StepTypeInfo, UpdateTrackEventsRequest,
    VersionSchemasResponse, WorkflowValidationErrorResponse, validate_scenario_inputs,
};
use crate::api::repositories::connections::ConnectionRepository;
use crate::api::repositories::scenarios::ScenarioRepository;
use crate::api::services::scenarios::{ScenarioService, ServiceError};
use crate::runtime_client::RuntimeClient;

use crate::types::MemoryTier;

// ============================================================================
// Request/Response Types
// ============================================================================

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct CreateScenarioRequest {
    pub name: String,
    pub description: String,
    #[serde(default)]
    #[serde(rename = "memoryTier")]
    pub memory_tier: Option<MemoryTier>,
    /// Enable step-event tracking for this scenario version (default: true)
    #[serde(default)]
    #[serde(rename = "trackEvents")]
    pub track_events: Option<bool>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct UpdateScenarioRequest {
    /// The execution graph containing scenario definition.
    /// Must include 'name' and optionally 'description' fields.
    #[serde(rename = "executionGraph")]
    pub execution_graph: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "memoryTier")]
    pub memory_tier: Option<MemoryTier>,
    /// Enable step-event tracking for this scenario version (optional, keeps existing if not provided)
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "trackEvents")]
    pub track_events: Option<bool>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct ListScenariosQuery {
    pub page: Option<i32>,
    #[serde(rename = "pageSize")]
    pub page_size: Option<i32>,
    /// Filter by folder path (e.g., "/Sales/")
    /// If not provided, returns all scenarios (backward compatible)
    pub path: Option<String>,
    /// If true and path is provided, includes scenarios in subfolders
    #[serde(default)]
    pub recursive: bool,
    /// Search scenarios by name (case-insensitive substring match)
    pub search: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct GetScenarioQuery {
    #[serde(rename = "versionNumber")]
    pub version_number: Option<i32>,
}

#[derive(Debug, Deserialize, Serialize, ToSchema)]
pub struct CloneScenarioRequest {
    pub name: String,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct ExecuteScenarioQuery {
    /// Specific version number to execute (defaults to active version)
    #[serde(default)]
    #[schema(value_type = Option<i32>)]
    pub version: Option<String>,
}

// ============================================================================
// HTTP Handlers
// ============================================================================

/// Create a new scenario with auto-generated ID
#[utoipa::path(
    post,
    path = "/api/runtime/scenarios/create",
    request_body = CreateScenarioRequest,
    responses(
        (status = 200, description = "Scenario created successfully", body = ApiResponse<ScenarioDto>),
        (status = 400, description = "Validation error", body = Value),
        (status = 500, description = "Internal server error", body = Value)
    ),
    tag = "scenario-controller"
)]
#[instrument(skip(pool, request), fields(scenario_name = %request.name))]
pub async fn create_scenario_handler(
    crate::middleware::tenant_auth::OrgId(tenant_id): crate::middleware::tenant_auth::OrgId,
    State(pool): State<PgPool>,

    Json(request): Json<CreateScenarioRequest>,
) -> (StatusCode, Json<Value>) {
    // Create repository and service
    let repository = Arc::new(ScenarioRepository::new(pool.clone()));
    let connection_repository = Arc::new(ConnectionRepository::new(pool));
    let service = ScenarioService::new(repository, connection_repository);

    // Delegate to service
    match service
        .create_scenario(
            &tenant_id,
            request.name,
            request.description,
            request.memory_tier,
            request.track_events,
        )
        .await
    {
        Ok(scenario_dto) => {
            let response =
                ApiResponse::success_with_message("Scenario created successfully", scenario_dto);
            (
                StatusCode::OK,
                Json(serde_json::to_value(response).unwrap()),
            )
        }
        Err(e) => map_service_error_to_response(e),
    }
}

/// Update a scenario by creating a new version
#[utoipa::path(
    post,
    path = "/api/runtime/scenarios/{id}/update",
    request_body = UpdateScenarioRequest,
    params(
        ("id" = String, Path, description = "Scenario identifier")
    ),
    responses(
        (status = 200, description = "Scenario version stored successfully", body = Value),
        (status = 400, description = "Workflow validation error with step context", body = WorkflowValidationErrorResponse),
        (status = 404, description = "Scenario not found", body = Value),
        (status = 500, description = "Internal server error", body = Value)
    ),
    tag = "scenario-controller"
)]
pub async fn update_scenario_handler(
    crate::middleware::tenant_auth::OrgId(tenant_id): crate::middleware::tenant_auth::OrgId,
    State(pool): State<PgPool>,
    State(_runtime_client): State<Option<Arc<RuntimeClient>>>,
    Path(scenario_id): Path<String>,
    Json(request): Json<UpdateScenarioRequest>,
) -> (StatusCode, Json<Value>) {
    // Create repositories and service
    let repository = Arc::new(ScenarioRepository::new(pool.clone()));
    let connection_repository = Arc::new(ConnectionRepository::new(pool.clone()));
    let service = ScenarioService::new(repository.clone(), connection_repository);

    // Delegate to service (name/description are now inside execution_graph)
    let (version_num, warnings) = match service
        .update_scenario(
            &tenant_id,
            &scenario_id,
            request.execution_graph,
            request.memory_tier,
            request.track_events,
        )
        .await
    {
        Ok(result) => result,
        Err(e) => return map_service_error_to_response(e),
    };

    // Queue compilation asynchronously instead of blocking
    // The compilation worker will process this in the background
    let compilation_status = if let Some(valkey_config) = crate::valkey::ValkeyConfig::from_env() {
        let redis_url = valkey_config.connection_url();
        match crate::workers::compilation_worker::enqueue_compilation(
            &redis_url,
            &tenant_id,
            &scenario_id,
            version_num,
        )
        .await
        {
            Ok(true) => "queued",
            Ok(false) => "already_pending",
            Err(e) => {
                tracing::warn!(
                    tenant_id = %tenant_id,
                    scenario_id = %scenario_id,
                    version = version_num,
                    error = %e,
                    "Failed to queue compilation, it will need to be triggered manually"
                );
                "queue_failed"
            }
        }
    } else {
        // Valkey not configured - compilation must be triggered manually
        tracing::warn!(
            tenant_id = %tenant_id,
            scenario_id = %scenario_id,
            version = version_num,
            "Valkey not configured, compilation must be triggered manually"
        );
        "manual_required"
    };

    let response = json!({
        "success": true,
        "message": "Scenario saved successfully",
        "scenarioId": scenario_id,
        "version": version_num.to_string(),
        "timestamp": chrono::Utc::now().to_rfc3339(),
        "warnings": warnings,
        "compilation": {
            "status": compilation_status
        }
    });
    (StatusCode::OK, Json(response))
}

/// Patch a scenario version's execution graph in-place (no new version created)
#[utoipa::path(
    put,
    path = "/api/runtime/scenarios/{id}/versions/{version}/graph",
    request_body = UpdateScenarioRequest,
    params(
        ("id" = String, Path, description = "Scenario identifier"),
        ("version" = i32, Path, description = "Version number to patch")
    ),
    responses(
        (status = 200, description = "Version graph updated in-place", body = Value),
        (status = 400, description = "Validation error", body = Value),
        (status = 404, description = "Version not found", body = Value),
        (status = 500, description = "Internal server error", body = Value)
    ),
    tag = "scenario-controller"
)]
pub async fn patch_version_graph_handler(
    crate::middleware::tenant_auth::OrgId(tenant_id): crate::middleware::tenant_auth::OrgId,
    State(pool): State<PgPool>,
    Path((scenario_id, version)): Path<(String, i32)>,
    Json(request): Json<UpdateScenarioRequest>,
) -> (StatusCode, Json<Value>) {
    let repository = Arc::new(ScenarioRepository::new(pool.clone()));
    let connection_repository = Arc::new(ConnectionRepository::new(pool.clone()));
    let service = ScenarioService::new(repository, connection_repository);

    let warnings = match service
        .patch_version_graph(&tenant_id, &scenario_id, version, request.execution_graph)
        .await
    {
        Ok(warnings) => warnings,
        Err(e) => return map_service_error_to_response(e),
    };

    let response = json!({
        "success": true,
        "message": "Version graph updated in-place",
        "scenarioId": scenario_id,
        "version": version.to_string(),
        "timestamp": chrono::Utc::now().to_rfc3339(),
        "warnings": warnings,
    });
    (StatusCode::OK, Json(response))
}

/// Toggle step-event tracking for a specific scenario version
#[utoipa::path(
    put,
    path = "/api/runtime/scenarios/{id}/versions/{version}/track-events",
    request_body = UpdateTrackEventsRequest,
    params(
        ("id" = String, Path, description = "Scenario identifier"),
        ("version" = i32, Path, description = "Version number")
    ),
    responses(
        (status = 200, description = "Track-events mode updated successfully", body = ApiResponse<ScenarioDto>),
        (status = 400, description = "Validation error", body = Value),
        (status = 404, description = "Scenario or version not found", body = Value),
        (status = 500, description = "Internal server error", body = Value)
    ),
    tag = "scenario-controller"
)]
#[instrument(skip(pool, request), fields(scenario_id = %scenario_id, version = %version))]
pub async fn toggle_track_events_handler(
    crate::middleware::tenant_auth::OrgId(tenant_id): crate::middleware::tenant_auth::OrgId,
    State(pool): State<PgPool>,

    Path((scenario_id, version)): Path<(String, i32)>,
    Json(request): Json<UpdateTrackEventsRequest>,
) -> (StatusCode, Json<Value>) {
    // Create repository and service
    let repository = Arc::new(ScenarioRepository::new(pool.clone()));
    let connection_repository = Arc::new(ConnectionRepository::new(pool));
    let service = ScenarioService::new(repository, connection_repository);

    // Delegate to service
    match service
        .toggle_track_events(&tenant_id, &scenario_id, version, request.track_events)
        .await
    {
        Ok(scenario_dto) => {
            let response = ApiResponse::success_with_message(
                "Track-events mode updated successfully. Compilation invalidated, will recompile on next execution.",
                scenario_dto,
            );
            (
                StatusCode::OK,
                Json(serde_json::to_value(response).unwrap()),
            )
        }
        Err(e) => map_service_error_to_response(e),
    }
}

/// List all scenarios for a tenant with pagination and optional folder filtering
#[utoipa::path(
    get,
    path = "/api/runtime/scenarios",
    params(
        ("page" = Option<i32>, Query, description = "Page number (1-based, default: 1)"),
        ("pageSize" = Option<i32>, Query, description = "Page size (default: 20, max: 100)"),
        ("path" = Option<String>, Query, description = "Filter by folder path (e.g., '/Sales/'). If not provided, returns all scenarios."),
        ("recursive" = bool, Query, description = "If true and path is provided, includes scenarios in subfolders (default: false)"),
        ("search" = Option<String>, Query, description = "Search scenarios by name (case-insensitive substring match)")
    ),
    responses(
        (status = 200, description = "List of scenarios retrieved successfully", body = ApiResponse<PageScenarioDto>),
        (status = 400, description = "Invalid path format", body = Value),
        (status = 500, description = "Internal server error", body = Value)
    ),
    tag = "scenario-controller"
)]
pub async fn list_scenarios_handler(
    crate::middleware::tenant_auth::OrgId(tenant_id): crate::middleware::tenant_auth::OrgId,
    State(pool): State<PgPool>,

    Query(query): Query<ListScenariosQuery>,
) -> (StatusCode, Json<Value>) {
    let handler_start = std::time::Instant::now();

    // Create repository and service
    let repository = Arc::new(ScenarioRepository::new(pool.clone()));
    let connection_repository = Arc::new(ConnectionRepository::new(pool));
    let service = ScenarioService::new(repository, connection_repository);

    // Delegate to service with pagination
    let page = query.page.unwrap_or(1);
    let page_size = query.page_size.unwrap_or(20);

    let query_start = std::time::Instant::now();

    match service
        .list_scenarios(
            &tenant_id,
            page,
            page_size,
            query.path.as_deref(),
            query.recursive,
            query.search.as_deref(),
        )
        .await
    {
        Ok((scenarios, total, current_page, current_page_size)) => {
            let query_duration = query_start.elapsed();
            let total_duration = handler_start.elapsed();
            tracing::debug!(
                query_ms = query_duration.as_millis(),
                total_ms = total_duration.as_millis(),
                scenario_count = scenarios.len(),
                total_count = total,
                "list_scenarios: completed"
            );
            let page_dto = PageScenarioDto::new(scenarios, total, current_page, current_page_size);
            let response = ApiResponse::success(page_dto);
            (
                StatusCode::OK,
                Json(serde_json::to_value(response).unwrap()),
            )
        }
        Err(e) => {
            let query_duration = query_start.elapsed();
            let total_duration = handler_start.elapsed();
            tracing::error!(
                query_ms = query_duration.as_millis(),
                total_ms = total_duration.as_millis(),
                error = %e,
                "list_scenarios: failed"
            );
            map_service_error_to_response(e)
        }
    }
}

/// Get a specific scenario by ID and optional version
#[utoipa::path(
    get,
    path = "/api/runtime/scenarios/{id}",
    params(
        ("id" = String, Path, description = "Scenario identifier"),
        ("versionNumber" = Option<i32>, Query, description = "Version number (defaults to latest)")
    ),
    responses(
        (status = 200, description = "Scenario retrieved successfully", body = ApiResponse<ScenarioDto>),
        (status = 404, description = "Scenario not found", body = Value),
        (status = 500, description = "Internal server error", body = Value)
    ),
    tag = "scenario-controller"
)]
pub async fn get_scenario_handler(
    crate::middleware::tenant_auth::OrgId(tenant_id): crate::middleware::tenant_auth::OrgId,
    State(pool): State<PgPool>,

    Path(scenario_id): Path<String>,
    Query(query): Query<GetScenarioQuery>,
) -> (StatusCode, Json<Value>) {
    // Create repository and service
    let repository = Arc::new(ScenarioRepository::new(pool.clone()));
    let connection_repository = Arc::new(ConnectionRepository::new(pool));
    let service = ScenarioService::new(repository, connection_repository);

    // Delegate to service
    match service
        .get_scenario(&tenant_id, &scenario_id, query.version_number)
        .await
    {
        Ok(scenario_dto) => {
            let response = ApiResponse::success(scenario_dto);
            (
                StatusCode::OK,
                Json(serde_json::to_value(response).unwrap()),
            )
        }
        Err(e) => map_service_error_to_response(e),
    }
}

/// Get all versions of a specific scenario
#[utoipa::path(
    get,
    path = "/api/runtime/scenarios/{id}/versions",
    params(
        ("id" = String, Path, description = "Scenario identifier")
    ),
    responses(
        (status = 200, description = "Scenario versions retrieved successfully", body = ApiResponse<Vec<ScenarioVersionInfoDto>>),
        (status = 404, description = "Scenario not found", body = Value),
        (status = 500, description = "Internal server error", body = Value)
    ),
    tag = "scenario-controller"
)]
pub async fn list_scenario_versions_handler(
    crate::middleware::tenant_auth::OrgId(tenant_id): crate::middleware::tenant_auth::OrgId,
    State(pool): State<PgPool>,

    Path(scenario_id): Path<String>,
) -> (StatusCode, Json<Value>) {
    // Create repository and service
    let repository = Arc::new(ScenarioRepository::new(pool.clone()));
    let connection_repository = Arc::new(ConnectionRepository::new(pool));
    let service = ScenarioService::new(repository, connection_repository);

    // Delegate to service
    match service.list_versions(&tenant_id, &scenario_id).await {
        Ok(versions) => {
            let response = ApiResponse::success(versions);
            (
                StatusCode::OK,
                Json(serde_json::to_value(response).unwrap()),
            )
        }
        Err(e) => map_service_error_to_response(e),
    }
}

/// Delete a scenario and all its versions (soft delete)
#[utoipa::path(
    post,
    path = "/api/runtime/scenarios/{id}/delete",
    params(
        ("id" = String, Path, description = "Scenario identifier")
    ),
    responses(
        (status = 200, description = "Scenario deleted successfully", body = Value),
        (status = 404, description = "Scenario not found", body = Value),
        (status = 500, description = "Internal server error", body = Value)
    ),
    tag = "scenario-controller"
)]
pub async fn delete_scenario_handler(
    crate::middleware::tenant_auth::OrgId(tenant_id): crate::middleware::tenant_auth::OrgId,
    State(pool): State<PgPool>,

    Path(scenario_id): Path<String>,
) -> (StatusCode, Json<Value>) {
    // Create repository and service
    let repository = Arc::new(ScenarioRepository::new(pool.clone()));
    let connection_repository = Arc::new(ConnectionRepository::new(pool));
    let service = ScenarioService::new(repository, connection_repository);

    // Delegate to service
    match service.delete_scenario(&tenant_id, &scenario_id).await {
        Ok(rows_affected) => {
            let response = json!({
                "success": true,
                "message": format!("Scenario '{}' marked as deleted ({} definitions deleted)", scenario_id, rows_affected),
                "scenarioId": scenario_id,
                "definitionsDeleted": rows_affected,
                "timestamp": chrono::Utc::now().to_rfc3339()
            });
            (StatusCode::OK, Json(response))
        }
        Err(e) => map_service_error_to_response(e),
    }
}

/// Clone a scenario with all its versions
#[utoipa::path(
    post,
    path = "/api/runtime/scenarios/{id}/clone",
    request_body = CloneScenarioRequest,
    params(
        ("id" = String, Path, description = "Source scenario identifier")
    ),
    responses(
        (status = 200, description = "Scenario cloned successfully", body = Value),
        (status = 400, description = "Validation error", body = Value),
        (status = 404, description = "Source scenario not found", body = Value),
        (status = 500, description = "Internal server error", body = Value)
    ),
    tag = "scenario-controller"
)]
pub async fn clone_scenario_handler(
    crate::middleware::tenant_auth::OrgId(tenant_id): crate::middleware::tenant_auth::OrgId,
    State(pool): State<PgPool>,

    Path(scenario_id): Path<String>,
    Json(request): Json<CloneScenarioRequest>,
) -> (StatusCode, Json<Value>) {
    // Create repository and service
    let repository = Arc::new(ScenarioRepository::new(pool.clone()));
    let connection_repository = Arc::new(ConnectionRepository::new(pool));
    let service = ScenarioService::new(repository, connection_repository);

    // Delegate to service
    match service
        .clone_scenario(&tenant_id, &scenario_id, &request.name)
        .await
    {
        Ok((new_scenario_id, versions_cloned)) => {
            let response = json!({
                "success": true,
                "message": format!("Scenario '{}' cloned successfully", scenario_id),
                "sourceScenarioId": scenario_id,
                "newScenarioId": new_scenario_id,
                "newName": request.name,
                "versionsCloned": versions_cloned,
                "timestamp": chrono::Utc::now().to_rfc3339()
            });
            (StatusCode::OK, Json(response))
        }
        Err(e) => map_service_error_to_response(e),
    }
}

// ============================================================================
// Compilation Handlers
// ============================================================================

/// Compile a specific scenario by tenant ID, scenario ID, and version
#[utoipa::path(
    post,
    path = "/api/runtime/scenarios/{id}/versions/{version}/compile",
    params(
        ("scenario_id" = String, Path, description = "Scenario identifier"),
        ("version" = String, Path, description = "Version number (positive integer)")
    ),
    responses(
        (status = 200, description = "Scenario compiled successfully", body = CompileScenarioResponse),
        (status = 400, description = "Invalid version format", body = ErrorResponse),
        (status = 404, description = "Scenario not found", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    ),
    tag = "scenario-controller"
)]
#[instrument(skip(pool, runtime_client), fields(scenario_id = %scenario_id, version = %version))]
pub async fn compile_scenario_handler(
    crate::middleware::tenant_auth::OrgId(tenant_id): crate::middleware::tenant_auth::OrgId,
    State(pool): State<PgPool>,
    State(runtime_client): State<Option<Arc<crate::runtime_client::RuntimeClient>>>,
    Path((scenario_id, version)): Path<(String, String)>,
) -> (StatusCode, Json<Value>) {
    // Validate version is a positive integer
    let version_num = match version.parse::<i32>() {
        Err(_) | Ok(0) | Ok(i32::MIN..=0) => {
            let error_response = json!({
                "success": false,
                "error": "Invalid version format",
                "message": "Version must be a positive integer (greater than 0).",
                "scenarioId": scenario_id,
                "version": version
            });
            return (StatusCode::BAD_REQUEST, Json(error_response));
        }
        Ok(v) => v,
    };

    // Route compilation through the queue if Valkey is available.
    // This ensures all compilations are serialized through the compilation worker,
    // preventing OOM from concurrent compiler processes.
    if let Some(valkey_config) = crate::valkey::ValkeyConfig::from_env() {
        let redis_url = valkey_config.connection_url();

        // Check if already compiled
        let repository = ScenarioRepository::new(pool.clone());
        match repository
            .get_registered_image_id(&tenant_id, &scenario_id, version_num)
            .await
        {
            Ok(Some(image_id)) => {
                let response = json!({
                    "success": true,
                    "message": "Scenario already compiled",
                    "scenarioId": scenario_id,
                    "version": version,
                    "imageId": image_id,
                    "registered": true,
                    "timestamp": chrono::Utc::now().to_rfc3339()
                });
                return (StatusCode::OK, Json(response));
            }
            Ok(None) => {} // Not compiled yet, proceed to queue
            Err(e) => {
                tracing::warn!(error = %e, "Failed to check compilation status, proceeding to queue");
            }
        }

        // Enqueue the compilation request
        match crate::workers::compilation_worker::enqueue_compilation(
            &redis_url,
            &tenant_id,
            &scenario_id,
            version_num,
        )
        .await
        {
            Ok(_) => {
                tracing::info!(
                    tenant_id = %tenant_id,
                    scenario_id = %scenario_id,
                    version = version_num,
                    "Compilation request queued via API"
                );
            }
            Err(e) => {
                let error_response = json!({
                    "success": false,
                    "error": "Failed to queue compilation",
                    "message": format!("Failed to enqueue compilation: {}", e),
                    "scenarioId": scenario_id,
                    "version": version
                });
                return (StatusCode::INTERNAL_SERVER_ERROR, Json(error_response));
            }
        }

        // Wait for the compilation worker to process the request (up to 5 minutes)
        let timeout = std::time::Duration::from_secs(300);
        let completed = crate::workers::compilation_worker::wait_for_compilation(
            &redis_url,
            &tenant_id,
            &scenario_id,
            version_num,
            timeout,
        )
        .await
        .unwrap_or(false);

        if !completed {
            let error_response = json!({
                "success": false,
                "error": "Compilation timeout",
                "message": format!(
                    "Compilation for scenario '{}' version {} timed out after 5 minutes",
                    scenario_id, version_num
                ),
                "scenarioId": scenario_id,
                "version": version
            });
            return (StatusCode::GATEWAY_TIMEOUT, Json(error_response));
        }

        // Query DB for the compilation result
        return match query_compilation_result(&pool, &tenant_id, &scenario_id, version_num).await {
            Ok(result) => {
                if result.success {
                    let mut response = json!({
                        "success": true,
                        "message": "Scenario compiled successfully",
                        "scenarioId": scenario_id,
                        "version": version,
                        "timestamp": chrono::Utc::now().to_rfc3339()
                    });
                    if let Some(image_id) = result.image_id {
                        response["imageId"] = json!(image_id);
                        response["registered"] = json!(true);
                    }
                    if let Some(size) = result.wasm_size {
                        response["binarySize"] = json!(size);
                    }
                    (StatusCode::OK, Json(response))
                } else {
                    let error_response = json!({
                        "success": false,
                        "error": "Compilation failed",
                        "message": result.error_message.unwrap_or_else(|| "Unknown compilation error".to_string()),
                        "scenarioId": scenario_id,
                        "version": version
                    });
                    (StatusCode::INTERNAL_SERVER_ERROR, Json(error_response))
                }
            }
            Err(e) => {
                let error_response = json!({
                    "success": false,
                    "error": "Database error",
                    "message": format!("Failed to query compilation result: {}", e),
                    "scenarioId": scenario_id,
                    "version": version
                });
                (StatusCode::INTERNAL_SERVER_ERROR, Json(error_response))
            }
        };
    }

    // Fallback: Valkey not configured, compile directly (still protected by semaphore)
    tracing::warn!("Valkey not configured, compiling directly (no queue)");
    let repository = Arc::new(ScenarioRepository::new(pool));
    let connection_service_url = std::env::var("CONNECTION_SERVICE_URL").ok();
    let compilation_service = crate::api::services::compilation::CompilationService::new(
        repository,
        connection_service_url,
        runtime_client,
    );

    match compilation_service
        .compile_scenario(&tenant_id, &scenario_id, version_num)
        .await
    {
        Ok(result) => {
            let mut response = json!({
                "success": true,
                "message": "Scenario compiled to native binary successfully",
                "scenarioId": result.scenario_id,
                "version": result.version.to_string(),
                "buildDir": result.build_dir,
                "binarySize": result.binary_size,
                "binaryChecksum": result.binary_checksum,
                "timestamp": chrono::Utc::now().to_rfc3339()
            });
            if let Some(image_id) = result.image_id {
                response["imageId"] = json!(image_id);
                response["registered"] = json!(true);
            }
            (StatusCode::OK, Json(response))
        }
        Err(crate::api::services::compilation::ServiceError::NotFound(msg)) => {
            let error_response = json!({
                "success": false,
                "error": "Scenario not found",
                "message": msg,
                "scenarioId": scenario_id,
                "version": version
            });
            (StatusCode::NOT_FOUND, Json(error_response))
        }
        Err(crate::api::services::compilation::ServiceError::CompilationError(msg)) => {
            let error_response = json!({
                "success": false,
                "error": "Compilation failed",
                "message": msg,
                "scenarioId": scenario_id,
                "version": version
            });
            (StatusCode::INTERNAL_SERVER_ERROR, Json(error_response))
        }
        Err(crate::api::services::compilation::ServiceError::DatabaseError(msg)) => {
            let error_response = json!({
                "success": false,
                "error": "Database error",
                "message": msg,
                "scenarioId": scenario_id,
                "version": version
            });
            (StatusCode::INTERNAL_SERVER_ERROR, Json(error_response))
        }
        Err(crate::api::services::compilation::ServiceError::RegistrationError(msg)) => {
            let error_response = json!({
                "success": false,
                "error": "Registration failed",
                "message": msg,
                "scenarioId": scenario_id,
                "version": version
            });
            (StatusCode::INTERNAL_SERVER_ERROR, Json(error_response))
        }
    }
}

/// Result of querying compilation status from the database after queue processing
struct CompilationQueryResult {
    success: bool,
    image_id: Option<String>,
    wasm_size: Option<i32>,
    error_message: Option<String>,
}

/// Raw row from scenario_compilations (avoids sqlx::query! macro which requires offline cache update)
#[derive(sqlx::FromRow)]
struct CompilationRow {
    compilation_status: String,
    registered_image_id: Option<String>,
    wasm_size: Option<i32>,
    error_message: Option<String>,
}

/// Query the compilation result from the database after the compilation worker has processed it
async fn query_compilation_result(
    pool: &PgPool,
    tenant_id: &str,
    scenario_id: &str,
    version: i32,
) -> Result<CompilationQueryResult, sqlx::Error> {
    let result: Option<CompilationRow> = sqlx::query_as(
        "SELECT compilation_status, registered_image_id, wasm_size, error_message \
         FROM scenario_compilations \
         WHERE tenant_id = $1 AND scenario_id = $2 AND version = $3",
    )
    .bind(tenant_id)
    .bind(scenario_id)
    .bind(version)
    .fetch_optional(pool)
    .await?;

    match result {
        Some(record) => Ok(CompilationQueryResult {
            success: record.compilation_status == "success" && record.registered_image_id.is_some(),
            image_id: record.registered_image_id,
            wasm_size: record.wasm_size,
            error_message: record.error_message,
        }),
        None => Ok(CompilationQueryResult {
            success: false,
            image_id: None,
            wasm_size: None,
            error_message: Some("No compilation record found after queue processing".to_string()),
        }),
    }
}

// ============================================================================
// Validation Handlers
// ============================================================================

/// Response for validate-mappings endpoint
#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ValidateMappingsResponse {
    pub success: bool,
    pub scenario_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<i32>,
    pub error_count: usize,
    pub warning_count: usize,
    pub issues: Vec<crate::api::utils::reference_validation::ValidationIssue>,
}

/// Query params for validate-mappings endpoint
#[derive(Debug, Deserialize, ToSchema)]
pub struct ValidateMappingsQuery {
    #[serde(rename = "versionNumber")]
    pub version_number: Option<i32>,
}

/// Validate scenario mappings without full compilation
/// Returns validation issues (errors and warnings) for reference paths, types, and connections
#[utoipa::path(
    post,
    path = "/api/runtime/scenarios/{id}/validate-mappings",
    params(
        ("id" = String, Path, description = "Scenario identifier"),
        ("versionNumber" = Option<i32>, Query, description = "Version number (defaults to latest)")
    ),
    responses(
        (status = 200, description = "Validation completed", body = ValidateMappingsResponse),
        (status = 404, description = "Scenario not found", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    ),
    tag = "scenario-controller"
)]
#[instrument(skip(pool), fields(scenario_id = %scenario_id))]
pub async fn validate_mappings_handler(
    crate::middleware::tenant_auth::OrgId(tenant_id): crate::middleware::tenant_auth::OrgId,
    State(pool): State<PgPool>,
    Path(scenario_id): Path<String>,
    Query(query): Query<ValidateMappingsQuery>,
) -> (StatusCode, Json<Value>) {
    // Create repositories and service
    let scenario_repository = Arc::new(ScenarioRepository::new(pool.clone()));
    let connection_repository = Arc::new(ConnectionRepository::new(pool));
    let service = ScenarioService::new(scenario_repository, connection_repository);

    // Validate mappings
    match service
        .validate_mappings(&tenant_id, &scenario_id, query.version_number)
        .await
    {
        Ok(issues) => {
            let error_count = issues
                .iter()
                .filter(|i| {
                    matches!(
                        i.severity,
                        crate::api::utils::reference_validation::IssueSeverity::Error
                    )
                })
                .count();
            let warning_count = issues.len() - error_count;

            let response = json!({
                "success": error_count == 0,
                "scenarioId": scenario_id,
                "version": query.version_number,
                "errorCount": error_count,
                "warningCount": warning_count,
                "issues": issues
            });
            (StatusCode::OK, Json(response))
        }
        Err(ServiceError::NotFound(msg)) => {
            let error_response = json!({
                "success": false,
                "error": "Scenario not found",
                "message": msg,
                "scenarioId": scenario_id
            });
            (StatusCode::NOT_FOUND, Json(error_response))
        }
        Err(ServiceError::ValidationError(msg)) => {
            let error_response = json!({
                "success": false,
                "error": "Validation error",
                "message": msg,
                "scenarioId": scenario_id
            });
            (StatusCode::BAD_REQUEST, Json(error_response))
        }
        Err(e) => {
            let error_response = json!({
                "success": false,
                "error": "Internal error",
                "message": e.to_string(),
                "scenarioId": scenario_id
            });
            (StatusCode::INTERNAL_SERVER_ERROR, Json(error_response))
        }
    }
}

// ============================================================================
// Execution Handlers
// ============================================================================

/// Execute a scenario by scheduling it with inputs (defaults to active version)
#[utoipa::path(
    post,
    path = "/api/runtime/scenarios/{id}/execute",
    request_body = ExecuteScenarioRequest,
    params(
        ("id" = String, Path, description = "Scenario identifier"),
        ("version" = Option<i32>, Query, description = "Specific version to execute (defaults to current)")
    ),
    responses(
        (status = 400, description = "Validation error", body = ErrorResponse),
        (status = 200, description = "Scenario scheduled successfully", body = ExecuteScenarioResponse),
        (status = 404, description = "Scenario not found", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    ),
    tag = "scenario-controller"
)]
#[instrument(skip(pool, trigger_stream, request), fields(scenario_id = %scenario_id))]
pub async fn execute_scenario_handler(
    crate::middleware::tenant_auth::OrgId(tenant_id): crate::middleware::tenant_auth::OrgId,
    State(pool): State<PgPool>,
    State(trigger_stream): State<
        Option<Arc<crate::api::repositories::trigger_stream::TriggerStreamPublisher>>,
    >,
    Path(scenario_id): Path<String>,
    Query(query): Query<ExecuteScenarioQuery>,
    Json(request): Json<ExecuteScenarioRequest>,
) -> (StatusCode, Json<Value>) {
    // Require trigger stream for execution
    let stream = match trigger_stream {
        Some(s) => s,
        None => {
            let error_response = json!({
                "success": false,
                "message": "Valkey trigger stream not configured. Cannot queue execution.",
                "data": serde_json::Value::Null
            });
            return (StatusCode::SERVICE_UNAVAILABLE, Json(error_response));
        }
    };

    // Parse and validate optional version query parameter
    let version = match query.version.as_deref().filter(|v| !v.is_empty()) {
        Some(v) => match v.parse::<i32>() {
            Ok(v) => Some(v),
            Err(_) => {
                let error_response = json!({
                    "success": false,
                    "message": "Invalid version parameter. Must be an integer.",
                    "data": serde_json::Value::Null
                });
                return (StatusCode::BAD_REQUEST, Json(error_response));
            }
        },
        None => None,
    };

    // Create repositories and service
    let scenario_repo = Arc::new(ScenarioRepository::new(pool));
    let service = crate::api::services::executions::ExecutionService::with_trigger_stream(
        scenario_repo,
        stream,
    );

    // Validate inputs match canonical format: {"data": {...}, "variables": {...}}
    let validated_inputs = match validate_scenario_inputs(request.inputs) {
        Ok(inputs) => inputs,
        Err(e) => {
            let error_response = json!({
                "success": false,
                "error": "INVALID_INPUT_FORMAT",
                "message": e.message
            });
            return (StatusCode::BAD_REQUEST, Json(error_response));
        }
    };

    let debug = request.debug.unwrap_or(false);

    // Delegate to service
    match service
        .queue_execution(&tenant_id, &scenario_id, version, validated_inputs, debug)
        .await
    {
        Ok(result) => {
            let response_data = json!({
                "instanceId": result.instance_id.to_string(),
                "status": result.status
            });
            let response = ApiResponse::success_with_message(
                "Scenario execution queued successfully",
                response_data,
            );
            (
                StatusCode::OK,
                Json(serde_json::to_value(response).unwrap()),
            )
        }
        Err(crate::api::services::executions::ServiceError::NotFound(msg)) => {
            let error_response = json!({
                "success": false,
                "message": msg,
                "data": serde_json::Value::Null
            });
            (StatusCode::NOT_FOUND, Json(error_response))
        }
        Err(crate::api::services::executions::ServiceError::DatabaseError(msg)) => {
            let error_response = json!({
                "success": false,
                "message": msg,
                "data": serde_json::Value::Null
            });
            (StatusCode::INTERNAL_SERVER_ERROR, Json(error_response))
        }
        Err(crate::api::services::executions::ServiceError::ValidationError(msg)) => {
            let error_response = json!({
                "success": false,
                "message": msg,
                "data": serde_json::Value::Null
            });
            (StatusCode::BAD_REQUEST, Json(error_response))
        }
    }
}

/// Get execution results for a scenario instance
#[utoipa::path(
    get,
    path = "/api/runtime/scenarios/instances/{instance_id}",
    params(
        ("instance_id" = String, Path, description = "Instance identifier (UUID)")
    ),
    responses(
        (status = 200, description = "Execution results retrieved successfully", body = ScenarioInstanceDto),
        (status = 404, description = "Instance not found", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    ),
    tag = "scenario-controller"
)]
#[instrument(skip(pool, runtime_client), fields(instance_id = %instance_id))]
pub async fn get_execution_metrics_handler(
    crate::middleware::tenant_auth::OrgId(tenant_id): crate::middleware::tenant_auth::OrgId,
    State(pool): State<PgPool>,
    State(runtime_client): State<Option<Arc<RuntimeClient>>>,
    Path(instance_id): Path<String>,
) -> (StatusCode, Json<Value>) {
    // Require runtime client - runtara-environment is the source of truth
    let runtime_client = match runtime_client {
        Some(client) => client,
        None => {
            let error_response = json!({
                "success": false,
                "message": "Runtime client not configured. Cannot get execution without runtara-environment connection.",
                "data": serde_json::Value::Null
            });
            return (StatusCode::INTERNAL_SERVER_ERROR, Json(error_response));
        }
    };

    // Create repositories and service
    let scenario_repo = Arc::new(ScenarioRepository::new(pool));
    let service =
        crate::api::services::executions::ExecutionService::new(scenario_repo, runtime_client);

    // Delegate to service
    match service
        .get_execution_results(&instance_id, &tenant_id)
        .await
    {
        Ok(instance) => {
            let response = ApiResponse::success(instance);
            (
                StatusCode::OK,
                Json(serde_json::to_value(response).unwrap()),
            )
        }
        Err(crate::api::services::executions::ServiceError::ValidationError(msg)) => {
            let error_response = json!({
                "success": false,
                "message": msg,
                "data": serde_json::Value::Null
            });
            (StatusCode::BAD_REQUEST, Json(error_response))
        }
        Err(crate::api::services::executions::ServiceError::NotFound(msg)) => {
            let error_response = json!({
                "success": false,
                "message": msg,
                "data": serde_json::Value::Null
            });
            (StatusCode::NOT_FOUND, Json(error_response))
        }
        Err(crate::api::services::executions::ServiceError::DatabaseError(msg)) => {
            let error_response = json!({
                "success": false,
                "message": format!("Database error: {}", msg),
                "data": serde_json::Value::Null
            });
            (StatusCode::INTERNAL_SERVER_ERROR, Json(error_response))
        }
    }
}

/// Get a scenario instance by scenario_id and instance_id with all available data
#[utoipa::path(
    get,
    path = "/api/runtime/scenarios/{scenario_id}/instances/{instance_id}",
    params(
        ("scenario_id" = String, Path, description = "Scenario identifier"),
        ("instance_id" = String, Path, description = "Instance identifier (UUID)")
    ),
    responses(
        (status = 200, description = "Scenario instance retrieved successfully", body = ScenarioInstanceDto),
        (status = 400, description = "Invalid instance ID format", body = ErrorResponse),
        (status = 404, description = "Instance not found", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    ),
    tag = "scenario-controller"
)]
#[instrument(skip(pool, runtime_client), fields(scenario_id = %scenario_id, instance_id = %instance_id))]
pub async fn get_instance_handler(
    crate::middleware::tenant_auth::OrgId(tenant_id): crate::middleware::tenant_auth::OrgId,
    State(pool): State<PgPool>,
    State(runtime_client): State<Option<Arc<RuntimeClient>>>,
    Path((scenario_id, instance_id)): Path<(String, String)>,
) -> (StatusCode, Json<Value>) {
    // Require runtime client - runtara-environment is the source of truth
    let runtime_client = match runtime_client {
        Some(client) => client,
        None => {
            let error_response = json!({
                "success": false,
                "message": "Runtime client not configured. Cannot get instance without runtara-environment connection.",
                "data": serde_json::Value::Null
            });
            return (StatusCode::INTERNAL_SERVER_ERROR, Json(error_response));
        }
    };

    // Create repositories and service
    let scenario_repo = Arc::new(ScenarioRepository::new(pool));
    let service =
        crate::api::services::executions::ExecutionService::new(scenario_repo, runtime_client);

    // Delegate to service
    match service
        .get_execution_with_metadata(&scenario_id, &instance_id, &tenant_id)
        .await
    {
        Ok(execution_data) => {
            // Build extended response with metadata
            let response_data = json!({
                "instance": execution_data.instance,
                "metadata": {
                    "scenarioName": execution_data.scenario_name,
                    "scenarioDescription": execution_data.scenario_description,
                    "workerId": execution_data.worker_id,
                    "heartbeatAt": execution_data.heartbeat_at.map(|t| t.to_rfc3339()),
                    "retryCount": execution_data.retry_count,
                    "maxRetries": execution_data.max_retries,
                    "additionalMetadata": execution_data.additional_metadata,
                    "errorMessage": execution_data.error_message,
                    "startedAt": execution_data.started_at.map(|t| t.to_rfc3339()),
                    "completedAt": execution_data.completed_at.map(|t| t.to_rfc3339()),
                }
            });
            let response = ApiResponse::success(response_data);
            (
                StatusCode::OK,
                Json(serde_json::to_value(response).unwrap()),
            )
        }
        Err(crate::api::services::executions::ServiceError::ValidationError(msg)) => {
            let error_response = json!({
                "success": false,
                "message": msg,
                "data": serde_json::Value::Null
            });
            (StatusCode::BAD_REQUEST, Json(error_response))
        }
        Err(crate::api::services::executions::ServiceError::NotFound(msg)) => {
            let error_response = json!({
                "success": false,
                "message": msg,
                "data": serde_json::Value::Null
            });
            (StatusCode::NOT_FOUND, Json(error_response))
        }
        Err(crate::api::services::executions::ServiceError::DatabaseError(msg)) => {
            let error_response = json!({
                "success": false,
                "message": format!("Database error: {}", msg),
                "data": serde_json::Value::Null
            });
            (StatusCode::INTERNAL_SERVER_ERROR, Json(error_response))
        }
    }
}

/// List all scenario instances for a given tenant and scenario
#[utoipa::path(
    get,
    path = "/api/runtime/scenarios/{scenario_id}/instances",
    params(
        ("scenario_id" = String, Path, description = "Scenario identifier"),
        ("page" = Option<i32>, Query, description = "Page number (default: 0)"),
        ("size" = Option<i32>, Query, description = "Page size (default: 10)")
    ),
    responses(
        (status = 200, description = "Scenario instances retrieved successfully", body = PageScenarioInstanceHistoryDto),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    ),
    tag = "scenario-controller"
)]
#[instrument(skip(pool, runtime_client), fields(scenario_id = %scenario_id))]
pub async fn list_instances_handler(
    crate::middleware::tenant_auth::OrgId(tenant_id): crate::middleware::tenant_auth::OrgId,
    State(pool): State<PgPool>,
    State(runtime_client): State<Option<Arc<RuntimeClient>>>,
    Path(scenario_id): Path<String>,
    Query(query): Query<ListInstancesQuery>,
) -> (StatusCode, Json<Value>) {
    // Require runtime client - runtara-environment is the source of truth
    let runtime_client = match runtime_client {
        Some(client) => client,
        None => {
            let error_response = json!({
                "success": false,
                "error": "Failed to list scenario instances",
                "message": "Runtime client not configured. Cannot list executions without runtara-environment connection.",
                "scenarioId": scenario_id
            });
            return (StatusCode::INTERNAL_SERVER_ERROR, Json(error_response));
        }
    };

    // Create repositories and service
    let scenario_repo = Arc::new(ScenarioRepository::new(pool));
    let service =
        crate::api::services::executions::ExecutionService::new(scenario_repo, runtime_client);

    // Delegate to service
    match service
        .list_executions(&tenant_id, &scenario_id, query.page, query.size)
        .await
    {
        Ok(page_dto) => {
            let response = ApiResponse::success(page_dto);
            (StatusCode::OK, Json(json!(response)))
        }
        Err(crate::api::services::executions::ServiceError::DatabaseError(msg)) => {
            let error_response = json!({
                "success": false,
                "error": "Failed to list scenario instances",
                "message": format!("Database error: {}", msg),
                "scenarioId": scenario_id
            });
            (StatusCode::INTERNAL_SERVER_ERROR, Json(error_response))
        }
        Err(crate::api::services::executions::ServiceError::NotFound(msg)) => {
            let error_response = json!({
                "success": false,
                "message": msg,
                "scenarioId": scenario_id
            });
            (StatusCode::NOT_FOUND, Json(error_response))
        }
        Err(crate::api::services::executions::ServiceError::ValidationError(msg)) => {
            let error_response = json!({
                "success": false,
                "message": msg,
                "scenarioId": scenario_id
            });
            (StatusCode::BAD_REQUEST, Json(error_response))
        }
    }
}

// ============================================================================
// Checkpoint Handlers
// ============================================================================

/// List checkpoints for a scenario instance via runtara management SDK
#[utoipa::path(
    get,
    path = "/api/runtime/scenarios/{scenario_id}/instances/{instance_id}/checkpoints",
    params(
        ("scenario_id" = String, Path, description = "Scenario identifier"),
        ("instance_id" = String, Path, description = "Instance identifier (UUID)"),
        ("page" = Option<i32>, Query, description = "Page number (default: 0)"),
        ("size" = Option<i32>, Query, description = "Page size (default: 20, max: 100)")
    ),
    responses(
        (status = 200, description = "Checkpoints retrieved successfully", body = ListCheckpointsResponse),
        (status = 400, description = "Invalid instance ID format", body = ErrorResponse),
        (status = 503, description = "Runtime client not configured", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    ),
    tag = "scenario-controller"
)]
#[instrument(skip(_pool, runtime_client), fields(instance_id = %instance_id))]
pub async fn list_instance_checkpoints_handler(
    crate::middleware::tenant_auth::OrgId(_tenant_id): crate::middleware::tenant_auth::OrgId,
    State(_pool): State<PgPool>,
    State(runtime_client): State<Option<Arc<RuntimeClient>>>,
    Path((_scenario_id, instance_id)): Path<(String, String)>,
    Query(query): Query<ListCheckpointsQuery>,
) -> (StatusCode, Json<Value>) {
    // Validate instance_id is a valid UUID
    if Uuid::parse_str(&instance_id).is_err() {
        let error_response = json!({
            "success": false,
            "error": "Invalid instance ID format",
            "message": "Instance ID must be a valid UUID",
        });
        return (StatusCode::BAD_REQUEST, Json(error_response));
    }

    // Check if runtime client is available
    let client = match runtime_client {
        Some(c) => c,
        None => {
            let error_response = json!({
                "success": false,
                "error": "Runtime client not configured",
                "message": "Checkpoints require runtara-environment connection",
            });
            return (StatusCode::SERVICE_UNAVAILABLE, Json(error_response));
        }
    };

    // Normalize pagination
    let page = query.page.unwrap_or(0).max(0);
    let size = query.size.unwrap_or(20).clamp(1, 100) as u32;

    // Fetch checkpoints via runtara management SDK
    match client.list_checkpoints(&instance_id, Some(size)).await {
        Ok(result) => {
            // Convert to DTOs and sort chronologically (oldest first)
            let mut checkpoints: Vec<CheckpointMetadataDto> = result
                .checkpoints
                .into_iter()
                .enumerate()
                .map(|(idx, cp)| CheckpointMetadataDto {
                    seq: idx as u64,
                    step_id: Some(cp.checkpoint_id.clone()),
                    operation: "checkpoint".to_string(),
                    result_type: "Inline".to_string(),
                    result_size: cp.data_size_bytes,
                })
                .collect();

            // Sort chronologically (by created_at via checkpoint_id)
            checkpoints.sort_by(|a, b| a.step_id.cmp(&b.step_id));

            let total_count = result.total_count as usize;
            let total_pages = ((total_count as f64) / (size as f64)).ceil() as i32;

            // Apply pagination (SDK already handles limit, but we track page for response)
            let response = ListCheckpointsResponse {
                success: true,
                instance_id: instance_id.clone(),
                checkpoints,
                total_count,
                page,
                size: size as i32,
                total_pages: total_pages.max(0),
            };

            (
                StatusCode::OK,
                Json(serde_json::to_value(response).unwrap()),
            )
        }
        Err(e) => {
            let error_response = json!({
                "success": false,
                "error": "Failed to retrieve checkpoints",
                "message": format!("{}", e),
            });
            (StatusCode::INTERNAL_SERVER_ERROR, Json(error_response))
        }
    }
}

/// Replay a scenario instance with the same inputs
///
/// Note: This endpoint is currently not implemented as execution data is stored
/// in runtara-environment and replay requires fetching instance inputs from there.
#[utoipa::path(
    post,
    path = "/api/runtime/scenarios/instances/{instance_id}/replay",
    params(
        ("instance_id" = String, Path, description = "Instance identifier (UUID)")
    ),
    responses(
        (status = 501, description = "Not implemented", body = Value),
        (status = 400, description = "Invalid instance ID", body = ErrorResponse),
        (status = 404, description = "Instance not found", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    ),
    tag = "scenario-controller"
)]
#[instrument(skip(_pool), fields(instance_id = %instance_id))]
pub async fn replay_instance_handler(
    crate::middleware::tenant_auth::OrgId(_tenant_id): crate::middleware::tenant_auth::OrgId,
    State(_pool): State<PgPool>,
    Path(instance_id): Path<String>,
) -> (StatusCode, Json<Value>) {
    // Replay is not currently supported - instance data is in runtara-environment
    // and we need to implement fetching the original inputs from there
    let response = json!({
        "success": false,
        "error": "Not implemented",
        "message": "Replay functionality requires fetching instance inputs from runtara-environment. This feature is pending implementation.",
        "instanceId": instance_id
    });
    (StatusCode::NOT_IMPLEMENTED, Json(response))
}

// ============================================================================
// Control Handlers
// ============================================================================

/// Stop a running scenario instance
#[utoipa::path(
    post,
    path = "/api/runtime/scenarios/instances/{instance_id}/stop",
    params(
        ("instance_id" = String, Path, description = "Instance identifier (UUID)")
    ),
    responses(
        (status = 200, description = "Instance stopped successfully", body = Value),
        (status = 400, description = "Invalid instance ID", body = ErrorResponse),
        (status = 404, description = "Instance not found", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    ),
    tag = "scenario-controller"
)]
#[instrument(skip(pool, running_executions, runtime_client), fields(instance_id = %instance_id))]
pub async fn stop_instance_handler(
    crate::middleware::tenant_auth::OrgId(tenant_id): crate::middleware::tenant_auth::OrgId,
    State(pool): State<PgPool>,
    State(running_executions): State<Arc<dashmap::DashMap<Uuid, crate::types::CancellationHandle>>>,
    State(runtime_client): State<Option<Arc<RuntimeClient>>>,
    Path(instance_id): Path<String>,
) -> (StatusCode, Json<Value>) {
    // Runtime client is required for stop_instance
    let runtime_client = match runtime_client {
        Some(client) => client,
        None => {
            let error_response = json!({
                "success": false,
                "error": "Runtime client not configured",
                "message": "Cannot stop instance without runtara-environment connection.",
                "instanceId": instance_id
            });
            return (StatusCode::SERVICE_UNAVAILABLE, Json(error_response));
        }
    };

    // Create repositories and service
    let scenario_repo = Arc::new(ScenarioRepository::new(pool));
    let service = crate::api::services::executions::ExecutionService::new(
        scenario_repo,
        runtime_client.clone(),
    );

    // Delegate to service
    match service
        .stop_instance(
            &instance_id,
            &tenant_id,
            &running_executions,
            Some(&runtime_client),
        )
        .await
    {
        Ok(crate::api::services::executions::StopInstanceResult::AlreadyStopped { status }) => {
            let response = ApiResponse::success_with_message(
                format!(
                    "Instance {} is already stopped (status: {})",
                    instance_id, status
                ),
                serde_json::Value::Null,
            );
            (StatusCode::OK, Json(json!(response)))
        }
        Ok(crate::api::services::executions::StopInstanceResult::Stopped {
            previous_status,
            cancellation_flag_set: _,
        }) => {
            let response = ApiResponse::success_with_message(
                format!(
                    "Instance {} stopped successfully (was: {})",
                    instance_id, previous_status
                ),
                serde_json::Value::Null,
            );
            (StatusCode::OK, Json(json!(response)))
        }
        Err(crate::api::services::executions::ServiceError::ValidationError(msg)) => {
            let error_response = json!({
                "success": false,
                "error": "Invalid instance ID",
                "message": msg,
                "instanceId": instance_id
            });
            (StatusCode::BAD_REQUEST, Json(error_response))
        }
        Err(crate::api::services::executions::ServiceError::NotFound(msg)) => {
            let error_response = json!({
                "success": false,
                "error": "Instance not found",
                "message": msg,
                "instanceId": instance_id
            });
            (StatusCode::NOT_FOUND, Json(error_response))
        }
        Err(crate::api::services::executions::ServiceError::DatabaseError(msg)) => {
            let error_response = json!({
                "success": false,
                "error": "Failed to stop instance",
                "message": msg,
                "instanceId": instance_id
            });
            (StatusCode::INTERNAL_SERVER_ERROR, Json(error_response))
        }
    }
}

/// Pause a running workflow instance
///
/// Sends a pause signal to the instance. The instance will checkpoint its state
/// and suspend execution until resumed.
#[utoipa::path(
    post,
    path = "/api/runtime/scenarios/instances/{instance_id}/pause",
    params(
        ("instance_id" = String, Path, description = "Instance UUID to pause")
    ),
    responses(
        (status = 200, description = "Instance paused successfully", body = Value),
        (status = 400, description = "Invalid instance ID or instance not pausable", body = ErrorResponse),
        (status = 404, description = "Instance not found", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    ),
    tag = "scenario-controller"
)]
#[instrument(skip(pool, runtime_client), fields(instance_id = %instance_id))]
pub async fn pause_instance_handler(
    crate::middleware::tenant_auth::OrgId(tenant_id): crate::middleware::tenant_auth::OrgId,
    State(pool): State<PgPool>,
    State(runtime_client): State<Option<Arc<RuntimeClient>>>,
    Path(instance_id): Path<String>,
) -> (StatusCode, Json<Value>) {
    // Runtime client is required for pause_instance
    let runtime_client = match runtime_client {
        Some(client) => client,
        None => {
            let error_response = json!({
                "success": false,
                "error": "Runtime client not configured",
                "message": "Cannot pause instance without runtara-environment connection.",
                "instanceId": instance_id
            });
            return (StatusCode::SERVICE_UNAVAILABLE, Json(error_response));
        }
    };

    // Create repositories and service
    let scenario_repo = Arc::new(ScenarioRepository::new(pool));
    let service = crate::api::services::executions::ExecutionService::new(
        scenario_repo,
        runtime_client.clone(),
    );

    // Delegate to service
    match service
        .pause_instance(&instance_id, &tenant_id, Some(&runtime_client))
        .await
    {
        Ok(crate::api::services::executions::PauseInstanceResult::AlreadyPaused) => {
            let response = ApiResponse::success_with_message(
                format!("Instance {} is already paused", instance_id),
                serde_json::Value::Null,
            );
            (StatusCode::OK, Json(json!(response)))
        }
        Ok(crate::api::services::executions::PauseInstanceResult::Paused { previous_status }) => {
            let response = ApiResponse::success_with_message(
                format!(
                    "Instance {} paused successfully (was: {})",
                    instance_id, previous_status
                ),
                serde_json::Value::Null,
            );
            (StatusCode::OK, Json(json!(response)))
        }
        Ok(crate::api::services::executions::PauseInstanceResult::NotPausable { status }) => {
            let error_response = json!({
                "success": false,
                "error": "Instance not pausable",
                "message": format!("Instance is in '{}' state and cannot be paused. Only running instances can be paused.", status),
                "instanceId": instance_id,
                "currentStatus": status
            });
            (StatusCode::BAD_REQUEST, Json(error_response))
        }
        Err(crate::api::services::executions::ServiceError::ValidationError(msg)) => {
            let error_response = json!({
                "success": false,
                "error": "Invalid request",
                "message": msg,
                "instanceId": instance_id
            });
            (StatusCode::BAD_REQUEST, Json(error_response))
        }
        Err(crate::api::services::executions::ServiceError::NotFound(msg)) => {
            let error_response = json!({
                "success": false,
                "error": "Instance not found",
                "message": msg,
                "instanceId": instance_id
            });
            (StatusCode::NOT_FOUND, Json(error_response))
        }
        Err(crate::api::services::executions::ServiceError::DatabaseError(msg)) => {
            let error_response = json!({
                "success": false,
                "error": "Failed to pause instance",
                "message": msg,
                "instanceId": instance_id
            });
            (StatusCode::INTERNAL_SERVER_ERROR, Json(error_response))
        }
    }
}

/// Resume a paused workflow instance
///
/// Sends a resume signal to the instance. The instance will resume execution
/// from its last checkpoint.
#[utoipa::path(
    post,
    path = "/api/runtime/scenarios/instances/{instance_id}/resume",
    params(
        ("instance_id" = String, Path, description = "Instance UUID to resume")
    ),
    responses(
        (status = 200, description = "Instance resumed successfully", body = Value),
        (status = 400, description = "Invalid instance ID or instance not resumable", body = ErrorResponse),
        (status = 404, description = "Instance not found", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    ),
    tag = "scenario-controller"
)]
#[instrument(skip(pool, runtime_client), fields(instance_id = %instance_id))]
pub async fn resume_instance_handler(
    crate::middleware::tenant_auth::OrgId(tenant_id): crate::middleware::tenant_auth::OrgId,
    State(pool): State<PgPool>,
    State(runtime_client): State<Option<Arc<RuntimeClient>>>,
    Path(instance_id): Path<String>,
) -> (StatusCode, Json<Value>) {
    // Runtime client is required for resume_instance
    let runtime_client = match runtime_client {
        Some(client) => client,
        None => {
            let error_response = json!({
                "success": false,
                "error": "Runtime client not configured",
                "message": "Cannot resume instance without runtara-environment connection.",
                "instanceId": instance_id
            });
            return (StatusCode::SERVICE_UNAVAILABLE, Json(error_response));
        }
    };

    // Create repositories and service
    let scenario_repo = Arc::new(ScenarioRepository::new(pool));
    let service = crate::api::services::executions::ExecutionService::new(
        scenario_repo,
        runtime_client.clone(),
    );

    // Delegate to service
    match service
        .resume_instance(&instance_id, &tenant_id, Some(&runtime_client))
        .await
    {
        Ok(crate::api::services::executions::ResumeInstanceResult::AlreadyRunning) => {
            let response = ApiResponse::success_with_message(
                format!("Instance {} is already running", instance_id),
                serde_json::Value::Null,
            );
            (StatusCode::OK, Json(json!(response)))
        }
        Ok(crate::api::services::executions::ResumeInstanceResult::Resumed { previous_status }) => {
            let response = ApiResponse::success_with_message(
                format!(
                    "Instance {} resumed successfully (was: {})",
                    instance_id, previous_status
                ),
                serde_json::Value::Null,
            );
            (StatusCode::OK, Json(json!(response)))
        }
        Ok(crate::api::services::executions::ResumeInstanceResult::NotResumable { status }) => {
            let error_response = json!({
                "success": false,
                "error": "Instance not resumable",
                "message": format!("Instance is in '{}' state and cannot be resumed. Only suspended instances can be resumed.", status),
                "instanceId": instance_id,
                "currentStatus": status
            });
            (StatusCode::BAD_REQUEST, Json(error_response))
        }
        Err(crate::api::services::executions::ServiceError::ValidationError(msg)) => {
            let error_response = json!({
                "success": false,
                "error": "Invalid request",
                "message": msg,
                "instanceId": instance_id
            });
            (StatusCode::BAD_REQUEST, Json(error_response))
        }
        Err(crate::api::services::executions::ServiceError::NotFound(msg)) => {
            let error_response = json!({
                "success": false,
                "error": "Instance not found",
                "message": msg,
                "instanceId": instance_id
            });
            (StatusCode::NOT_FOUND, Json(error_response))
        }
        Err(crate::api::services::executions::ServiceError::DatabaseError(msg)) => {
            let error_response = json!({
                "success": false,
                "error": "Failed to resume instance",
                "message": msg,
                "instanceId": instance_id
            });
            (StatusCode::INTERNAL_SERVER_ERROR, Json(error_response))
        }
    }
}

/// Schedule a scenario execution (placeholder - not implemented)
#[utoipa::path(
    post,
    path = "/api/runtime/scenarios/{id}/schedule",
    params(
        ("id" = String, Path, description = "Scenario identifier")
    ),
    request_body = Value,
    responses(
        (status = 501, description = "Not implemented", body = Value)
    ),
    tag = "scenario-controller"
)]
#[instrument(fields(scenario_id = %id))]
pub async fn schedule_scenario_handler(
    crate::middleware::tenant_auth::OrgId(tenant_id): crate::middleware::tenant_auth::OrgId,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> (StatusCode, Json<Value>) {
    // Scheduling requires additional infrastructure:
    // 1. scenario_schedules table
    // 2. Background scheduler service (e.g., tokio-cron-scheduler)
    // 3. Schedule execution logic
    let response = json!({
        "success": false,
        "error": "Not implemented",
        "message": "Scenario scheduling requires additional infrastructure (scheduling service, cron scheduler). This endpoint is a placeholder for future implementation.",
        "endpoint": format!("/api/runtime/{}/scenarios/{}/schedule", tenant_id, id),
        "scenarioId": id,
        "requestedSchedule": body.get("schedule"),
        "status": 501,
        "suggestion": "Use the execute endpoint to run scenarios immediately, or implement a scheduling service externally"
    });
    (StatusCode::NOT_IMPLEMENTED, Json(response))
}

// ============================================================================
// Error Mapping
// ============================================================================

/// Map ServiceError to HTTP response with appropriate status code
fn map_service_error_to_response(error: ServiceError) -> (StatusCode, Json<Value>) {
    match error {
        ServiceError::ValidationError(msg) => {
            let response = json!({
                "success": false,
                "message": msg
            });
            (StatusCode::BAD_REQUEST, Json(response))
        }
        ServiceError::WorkflowValidationError { message, errors } => {
            let response = json!({
                "success": false,
                "message": message,
                "validationErrors": errors
            });
            (StatusCode::BAD_REQUEST, Json(response))
        }
        ServiceError::NotFound(msg) => {
            let response = json!({
                "success": false,
                "message": msg
            });
            (StatusCode::NOT_FOUND, Json(response))
        }
        ServiceError::Conflict(msg) => {
            let response = json!({
                "success": false,
                "message": msg
            });
            (StatusCode::CONFLICT, Json(response))
        }
        ServiceError::DatabaseError(msg) => {
            let response = json!({
                "success": false,
                "message": format!("Database error: {}", msg)
            });
            (StatusCode::INTERNAL_SERVER_ERROR, Json(response))
        }
        ServiceError::ExecutionError(msg) => {
            let response = json!({
                "success": false,
                "message": format!("Execution error: {}", msg)
            });
            (StatusCode::INTERNAL_SERVER_ERROR, Json(response))
        }
        ServiceError::CompilationTimeout(msg) => {
            let response = json!({
                "success": false,
                "message": format!("Compilation timeout: {}", msg)
            });
            (StatusCode::GATEWAY_TIMEOUT, Json(response))
        }
    }
}

// ============================================================================
// Version Management Handlers
// ============================================================================

/// Set the current version for a scenario
///
/// Updates which version is marked as "current" for execution.
/// Note: Requires database migration to add current_version column.
#[utoipa::path(
    post,
    path = "/api/runtime/scenarios/{scenario_id}/versions/{version_number}/set-current",
    params(
        ("scenario_id" = String, Path, description = "Scenario identifier"),
        ("version_number" = i32, Path, description = "Version number to set as current")
    ),
    responses(
        (status = 200, description = "Current version updated successfully"),
        (status = 400, description = "Invalid request", body = ErrorResponse),
        (status = 404, description = "Scenario or version not found", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    ),
    tag = "scenario-controller"
)]
#[instrument(skip(pool), fields(scenario_id = %scenario_id, version_number = %version_number))]
pub async fn set_current_version_handler(
    crate::middleware::tenant_auth::OrgId(tenant_id): crate::middleware::tenant_auth::OrgId,
    State(pool): State<PgPool>,

    Path((scenario_id, version_number)): Path<(String, i32)>,
) -> (StatusCode, Json<Value>) {
    // Create repository and service
    let repository = Arc::new(ScenarioRepository::new(pool.clone()));
    let connection_repository = Arc::new(ConnectionRepository::new(pool));
    let service = ScenarioService::new(repository, connection_repository);

    // Delegate to service
    match service
        .set_current_version(&tenant_id, &scenario_id, version_number)
        .await
    {
        Ok(()) => {
            let response = json!({
                "success": true,
                "message": format!("Current version set to {} for scenario '{}'", version_number, scenario_id),
                "scenarioId": scenario_id,
                "currentVersion": version_number,
                "timestamp": chrono::Utc::now().to_rfc3339()
            });
            (StatusCode::OK, Json(response))
        }
        Err(e) => map_service_error_to_response(e),
    }
}

// ============================================================================
// Metadata Handlers
// ============================================================================

/// Validate graph structure
///
/// Pure validation handler - no database or external dependencies.
/// Validates the execution graph using runtara-workflows validation.
#[utoipa::path(
    post,
    path = "/api/runtime/scenarios/graph/validate",
    request_body = Value,
    responses(
        (status = 200, description = "Validation completed"),
        (status = 400, description = "Validation failed")
    ),
    tag = "scenario-controller"
)]
#[instrument(skip(body))]
pub async fn validate_graph_handler(Json(body): Json<Value>) -> (StatusCode, Json<Value>) {
    // Validate that it's a valid JSON object
    if !body.is_object() {
        let error_response = json!({
            "success": false,
            "valid": false,
            "error": "Invalid graph format",
            "message": "Graph must be a JSON object"
        });
        return (StatusCode::BAD_REQUEST, Json(error_response));
    }

    // Try to parse as runtara-dsl Scenario and validate with runtara-workflows
    match serde_json::from_value::<runtara_dsl::Scenario>(json!({
        "executionGraph": body.clone()
    })) {
        Ok(scenario) => {
            let validation_result =
                runtara_workflows::validation::validate_workflow(&scenario.execution_graph);

            let errors: Vec<String> = validation_result
                .errors
                .iter()
                .map(|e| e.to_string())
                .collect();
            let warnings: Vec<String> = validation_result
                .warnings
                .iter()
                .map(|w| w.to_string())
                .collect();

            let is_valid = errors.is_empty();
            let message = if is_valid {
                "Graph validation passed".to_string()
            } else {
                format!("Graph validation failed with {} error(s)", errors.len())
            };

            let response = json!({
                "success": true,
                "valid": is_valid,
                "errors": errors,
                "warnings": warnings,
                "message": message,
                "timestamp": chrono::Utc::now().to_rfc3339()
            });
            (StatusCode::OK, Json(response))
        }
        Err(e) => {
            let response = json!({
                "success": true,
                "valid": false,
                "errors": [format!("Failed to parse graph: {}", e)],
                "warnings": [],
                "message": "Graph validation failed: invalid scenario format",
                "timestamp": chrono::Utc::now().to_rfc3339()
            });
            (StatusCode::OK, Json(response))
        }
    }
}

/// List all supported step types
///
/// Returns hardcoded metadata about available step types.
/// No database or external dependencies - just static data.
#[utoipa::path(
    get,
    path = "/api/runtime/steps",
    responses(
        (status = 200, description = "Step types retrieved successfully", body = ListStepTypesResponse),
        (status = 500, description = "Internal server error")
    ),
    tag = "scenario-controller"
)]
#[instrument]
pub async fn list_step_types_handler() -> Result<Json<ListStepTypesResponse>, StatusCode> {
    // Start step is virtual (no struct), add it first
    let mut step_types = vec![StepTypeInfo {
        id: "Start".to_string(),
        name: "Start".to_string(),
        description: "Entry point - receives scenario inputs".to_string(),
        category: "control".to_string(),
    }];

    // Add all registered step types from inventory
    for meta in runtara_dsl::agent_meta::get_all_step_types() {
        step_types.push(StepTypeInfo {
            id: meta.id.to_string(),
            name: meta.display_name.to_string(),
            description: meta.description.to_string(),
            category: meta.category.to_string(),
        });
    }

    // Sort by id for consistent ordering
    step_types.sort_by(|a, b| a.id.cmp(&b.id));

    Ok(Json(ListStepTypesResponse { step_types }))
}

/// Get step subinstances (execution events) for a specific step
///
/// Note: This endpoint is currently not implemented as execution event data
/// is stored in runtara-environment and requires querying the environment.
#[utoipa::path(
    get,
    path = "/api/runtime/scenarios/instances/{instance_id}/steps/{step_id}/subinstances",
    params(
        ("instance_id" = String, Path, description = "Instance identifier (UUID)"),
        ("step_id" = String, Path, description = "Step identifier")
    ),
    responses(
        (status = 501, description = "Not implemented", body = Value),
        (status = 404, description = "Instance not found", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    ),
    tag = "scenario-controller"
)]
#[instrument(skip(_pool), fields(instance_id = %instance_id, step_id = %step_id))]
pub async fn get_step_subinstances_handler(
    crate::middleware::tenant_auth::OrgId(_tenant_id): crate::middleware::tenant_auth::OrgId,
    State(_pool): State<PgPool>,
    Path((instance_id, step_id)): Path<(String, String)>,
) -> (StatusCode, Json<Value>) {
    // Step subinstances query is not currently supported - execution event data is in runtara-environment
    let response = json!({
        "success": false,
        "error": "Not implemented",
        "message": "Step subinstances functionality requires querying execution events from runtara-environment. This feature is pending implementation.",
        "instanceId": instance_id,
        "stepId": step_id
    });
    (StatusCode::NOT_IMPLEMENTED, Json(response))
}

// ============================================================================
// Dependency Tracking Handlers
// ============================================================================

/// Get all dependencies for a scenario
///
/// Returns all child scenarios that this scenario depends on (via StartScenario steps).
/// Can query all versions or a specific version.
#[utoipa::path(
    get,
    path = "/api/runtime/scenarios/{id}/dependencies",
    params(
        ("id" = String, Path, description = "Scenario ID"),
        ("version" = Option<i32>, Query, description = "Optional version number (returns all versions if not specified)")
    ),
    responses(
        (status = 200, description = "Dependencies retrieved successfully", body = GetDependenciesResponse),
        (status = 401, description = "Unauthorized"),
        (status = 500, description = "Internal server error")
    ),
    tag = "scenario-controller"
)]
#[instrument]
pub async fn get_scenario_dependencies_handler(
    crate::middleware::tenant_auth::OrgId(tenant_id): crate::middleware::tenant_auth::OrgId,
    State(pool): State<PgPool>,
    Path(scenario_id): Path<String>,
    Query(params): Query<serde_json::Value>,
) -> Result<Json<GetDependenciesResponse>, (StatusCode, Json<ErrorResponse>)> {
    let version = params
        .get("version")
        .and_then(|v| v.as_i64())
        .map(|v| v as i32);

    let repo = ScenarioRepository::new(pool);
    match repo
        .get_dependencies(&tenant_id, &scenario_id, version)
        .await
    {
        Ok(deps) => {
            let dependencies = deps
                .into_iter()
                .map(
                    |(parent_version, child_id, child_requested, child_resolved, step_id)| {
                        ScenarioDependency {
                            parent_version,
                            child_scenario_id: child_id,
                            child_version_requested: child_requested,
                            child_version_resolved: child_resolved,
                            step_id,
                        }
                    },
                )
                .collect();

            Ok(Json(GetDependenciesResponse {
                success: true,
                dependencies,
            }))
        }
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                success: false,
                error: "DatabaseError".to_string(),
                message: Some(format!("Failed to query dependencies: {}", e)),
            }),
        )),
    }
}

/// Get all parent scenarios that depend on this scenario
///
/// Returns all parent scenarios that reference this scenario in StartScenario steps.
/// Can query all versions or a specific version.
#[utoipa::path(
    get,
    path = "/api/runtime/scenarios/{id}/dependents",
    params(
        ("id" = String, Path, description = "Scenario ID"),
        ("version" = Option<i32>, Query, description = "Optional version number (returns all versions if not specified)")
    ),
    responses(
        (status = 200, description = "Dependents retrieved successfully", body = GetDependentsResponse),
        (status = 401, description = "Unauthorized"),
        (status = 500, description = "Internal server error")
    ),
    tag = "scenario-controller"
)]
#[instrument]
pub async fn get_scenario_dependents_handler(
    crate::middleware::tenant_auth::OrgId(tenant_id): crate::middleware::tenant_auth::OrgId,
    State(pool): State<PgPool>,
    Path(scenario_id): Path<String>,
    Query(params): Query<serde_json::Value>,
) -> Result<Json<GetDependentsResponse>, (StatusCode, Json<ErrorResponse>)> {
    let version = params
        .get("version")
        .and_then(|v| v.as_i64())
        .map(|v| v as i32);

    let repo = ScenarioRepository::new(pool);
    match repo.get_dependents(&tenant_id, &scenario_id, version).await {
        Ok(deps) => {
            let dependents = deps
                .into_iter()
                .map(
                    |(parent_id, parent_version, child_resolved, step_id)| ScenarioDependent {
                        parent_scenario_id: parent_id,
                        parent_version,
                        child_version_resolved: child_resolved,
                        step_id,
                    },
                )
                .collect();

            Ok(Json(GetDependentsResponse {
                success: true,
                dependents,
            }))
        }
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                success: false,
                error: "DatabaseError".to_string(),
                message: Some(format!("Failed to query dependents: {}", e)),
            }),
        )),
    }
}

// ============================================================================
// Schema Handlers
// ============================================================================

/// Get schemas for a specific scenario version
///
/// Returns the input schema, output schema, and variables from the execution graph
/// of a specific scenario version.
#[utoipa::path(
    get,
    path = "/api/runtime/scenarios/{id}/versions/{version}/schemas",
    params(
        ("id" = String, Path, description = "Scenario ID"),
        ("version" = i32, Path, description = "Version number")
    ),
    responses(
        (status = 200, description = "Schemas retrieved successfully", body = VersionSchemasResponse),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Scenario or version not found"),
        (status = 500, description = "Internal server error")
    ),
    tag = "scenario-controller"
)]
#[instrument]
pub async fn get_version_schemas_handler(
    crate::middleware::tenant_auth::OrgId(tenant_id): crate::middleware::tenant_auth::OrgId,
    State(pool): State<PgPool>,
    Path((scenario_id, version)): Path<(String, i32)>,
) -> Result<Json<VersionSchemasResponse>, (StatusCode, Json<ErrorResponse>)> {
    let repo = Arc::new(ScenarioRepository::new(pool.clone()));
    let connection_repo = Arc::new(ConnectionRepository::new(pool));
    let service = ScenarioService::new(repo, connection_repo);

    match service
        .get_version_schemas(&tenant_id, &scenario_id, version)
        .await
    {
        Ok((input_schema, output_schema, variables)) => Ok(Json(VersionSchemasResponse {
            input_schema,
            output_schema,
            variables,
        })),
        Err(ServiceError::NotFound(msg)) => Err((
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                success: false,
                error: "NotFound".to_string(),
                message: Some(msg),
            }),
        )),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                success: false,
                error: "InternalError".to_string(),
                message: Some(e.to_string()),
            }),
        )),
    }
}

// ============================================================================
// Folder Management Handlers
// ============================================================================

/// Move a scenario to a different folder
#[utoipa::path(
    put,
    path = "/api/runtime/scenarios/{id}/move",
    request_body = MoveScenarioRequest,
    params(
        ("id" = String, Path, description = "Scenario identifier")
    ),
    responses(
        (status = 200, description = "Scenario moved successfully", body = ApiResponse<MoveScenarioResponse>),
        (status = 400, description = "Invalid path format", body = Value),
        (status = 404, description = "Scenario not found", body = Value),
        (status = 500, description = "Internal server error", body = Value)
    ),
    tag = "scenario-controller"
)]
pub async fn move_scenario_handler(
    crate::middleware::tenant_auth::OrgId(tenant_id): crate::middleware::tenant_auth::OrgId,
    State(pool): State<PgPool>,
    Path(id): Path<String>,
    Json(request): Json<MoveScenarioRequest>,
) -> (StatusCode, Json<Value>) {
    // Create repository and service
    let repository = Arc::new(ScenarioRepository::new(pool.clone()));
    let connection_repository = Arc::new(ConnectionRepository::new(pool));
    let service = ScenarioService::new(repository, connection_repository);

    match service.move_scenario(&tenant_id, &id, &request.path).await {
        Ok(response) => {
            let api_response =
                ApiResponse::success_with_message("Scenario moved successfully", response);
            (
                StatusCode::OK,
                Json(serde_json::to_value(api_response).unwrap()),
            )
        }
        Err(e) => map_service_error_to_response(e),
    }
}

/// List all folders (distinct paths) for a tenant
#[utoipa::path(
    get,
    path = "/api/runtime/scenarios/folders",
    responses(
        (status = 200, description = "Folders retrieved successfully", body = FoldersResponse),
        (status = 500, description = "Internal server error", body = Value)
    ),
    tag = "scenario-controller"
)]
pub async fn list_folders_handler(
    crate::middleware::tenant_auth::OrgId(tenant_id): crate::middleware::tenant_auth::OrgId,
    State(pool): State<PgPool>,
) -> (StatusCode, Json<Value>) {
    // Create repository and service
    let repository = Arc::new(ScenarioRepository::new(pool.clone()));
    let connection_repository = Arc::new(ConnectionRepository::new(pool));
    let service = ScenarioService::new(repository, connection_repository);

    match service.list_folders(&tenant_id).await {
        Ok(response) => (
            StatusCode::OK,
            Json(serde_json::to_value(response).unwrap()),
        ),
        Err(e) => map_service_error_to_response(e),
    }
}

/// Rename a folder (updates all scenarios with matching path prefix)
#[utoipa::path(
    put,
    path = "/api/runtime/scenarios/folders/rename",
    request_body = RenameFolderRequest,
    responses(
        (status = 200, description = "Folder renamed successfully", body = ApiResponse<RenameFolderResponse>),
        (status = 400, description = "Invalid path format or cannot rename root", body = Value),
        (status = 500, description = "Internal server error", body = Value)
    ),
    tag = "scenario-controller"
)]
pub async fn rename_folder_handler(
    crate::middleware::tenant_auth::OrgId(tenant_id): crate::middleware::tenant_auth::OrgId,
    State(pool): State<PgPool>,
    Json(request): Json<RenameFolderRequest>,
) -> (StatusCode, Json<Value>) {
    // Create repository and service
    let repository = Arc::new(ScenarioRepository::new(pool.clone()));
    let connection_repository = Arc::new(ConnectionRepository::new(pool));
    let service = ScenarioService::new(repository, connection_repository);

    match service
        .rename_folder(&tenant_id, &request.old_path, &request.new_path)
        .await
    {
        Ok(response) => {
            let api_response =
                ApiResponse::success_with_message("Folder renamed successfully", response);
            (
                StatusCode::OK,
                Json(serde_json::to_value(api_response).unwrap()),
            )
        }
        Err(e) => map_service_error_to_response(e),
    }
}

// ============================================================================
// WASM Binary Download Handler
// ============================================================================

/// Download the compiled WASM binary for a scenario version
///
/// Returns the raw WASM binary with appropriate headers for browser consumption.
/// Supports ETag-based caching via If-None-Match.
pub async fn download_wasm_handler(
    crate::middleware::tenant_auth::OrgId(tenant_id): crate::middleware::tenant_auth::OrgId,
    State(pool): State<PgPool>,
    headers: HeaderMap,
    Path((scenario_id, version)): Path<(String, String)>,
) -> Response {
    // Validate version
    let version_num = match version.parse::<i32>() {
        Err(_) | Ok(i32::MIN..=0) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "error": "Invalid version format",
                    "message": "Version must be a positive integer."
                })),
            )
                .into_response();
        }
        Ok(v) => v,
    };

    // Query for the WASM binary path
    let repository = ScenarioRepository::new(pool);
    let (translated_path, wasm_checksum) = match repository
        .get_wasm_path(&tenant_id, &scenario_id, version_num)
        .await
    {
        Ok(Some(result)) => result,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({
                    "error": "Compilation not found",
                    "message": format!(
                        "No successful compilation found for scenario {} version {}",
                        scenario_id, version
                    )
                })),
            )
                .into_response();
        }
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "error": "Database error",
                    "message": format!("{}", e)
                })),
            )
                .into_response();
        }
    };

    // Build the binary path
    let binary_path = std::path::PathBuf::from(&translated_path).join("scenario.wasm");

    // Check file exists
    let metadata = match tokio::fs::metadata(&binary_path).await {
        Ok(m) => m,
        Err(_) => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({
                    "error": "WASM binary not found",
                    "message": "The compiled WASM binary is missing from the filesystem. Try recompiling."
                })),
            )
                .into_response();
        }
    };

    // ETag support
    let etag = wasm_checksum.as_deref().map(|c| format!("\"{}\"", c));

    let client_matches_etag = etag.as_ref().is_some_and(|etag_val| {
        headers
            .get(header::IF_NONE_MATCH)
            .is_some_and(|v| v.as_bytes() == etag_val.as_bytes())
    });
    if client_matches_etag {
        return StatusCode::NOT_MODIFIED.into_response();
    }

    // Stream the file
    let file = match tokio::fs::File::open(&binary_path).await {
        Ok(f) => f,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "error": "Failed to read WASM binary",
                    "message": format!("{}", e)
                })),
            )
                .into_response();
        }
    };

    let stream = tokio_util::io::ReaderStream::new(file);
    let body = axum::body::Body::from_stream(stream);

    let mut response = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/wasm")
        .header(header::CONTENT_LENGTH, metadata.len())
        .header(
            header::CONTENT_DISPOSITION,
            format!(
                "attachment; filename=\"scenario-{}-v{}.wasm\"",
                scenario_id, version
            ),
        )
        .header(header::CACHE_CONTROL, "private, max-age=3600");

    if let Some(etag_val) = etag {
        response = response.header(header::ETAG, etag_val);
    }

    response.body(body).unwrap_or_else(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": "Failed to build response"})),
        )
            .into_response()
    })
}

// ============================================================================
// Debug Proxy Handler
// ============================================================================

/// Authenticated proxy handler for browser-side WASM debugging
///
/// Wraps the internal proxy logic but runs through the authenticated tenant_routes,
/// so browser-side WASM scenarios can make credential-injected requests via the gateway.
pub async fn debug_proxy_handler(
    crate::middleware::tenant_auth::OrgId(tenant_id): crate::middleware::tenant_auth::OrgId,
    State(pool): State<PgPool>,
    Path(_scenario_id): Path<String>,
    Json(request): Json<crate::api::handlers::internal_proxy::ProxyRequest>,
) -> Result<
    (
        StatusCode,
        Json<crate::api::handlers::internal_proxy::ProxyResponse>,
    ),
    (StatusCode, Json<Value>),
> {
    // Create a one-shot client for the proxy request.
    // POC simplification — production would share a client via AppState.
    let client = reqwest::Client::new();
    let redis_url = crate::valkey::build_redis_url();
    crate::api::handlers::internal_proxy::execute_proxy_request(
        &tenant_id,
        &pool,
        &client,
        request,
        redis_url.as_deref(),
    )
    .await
}

// ============================================================================
// Runtara-Core Proxy (for browser WASM execution)
// ============================================================================

/// Reverse proxy to the embedded runtara-core HTTP instance API.
///
/// Browser-side WASM scenarios need to reach runtara-core for instance lifecycle
/// (register, checkpoint, completed, events, signals). This endpoint provides
/// authenticated access through the gateway.
///
/// Routes: `/api/runtime/core/{*path}` → `http://localhost:{RUNTARA_CORE_HTTP_PORT}/{path}`
pub async fn core_proxy_handler(
    crate::middleware::tenant_auth::OrgId(_tenant_id): crate::middleware::tenant_auth::OrgId,
    request: axum::extract::Request,
) -> Response {
    let core_port: u16 = std::env::var("RUNTARA_CORE_HTTP_PORT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(8003);

    // Extract the sub-path after /api/runtime/core/
    let path = request.uri().path();
    let core_path = path.strip_prefix("/api/runtime/core").unwrap_or(path);
    let query = request
        .uri()
        .query()
        .map(|q| format!("?{}", q))
        .unwrap_or_default();
    let target_url = format!("http://127.0.0.1:{}{}{}", core_port, core_path, query);

    let method = request.method().clone();
    let client = reqwest::Client::new();

    // Forward headers (except host)
    let mut headers = reqwest::header::HeaderMap::new();
    for (key, value) in request.headers() {
        if key == "host" {
            continue;
        }
        if let (Ok(name), Ok(val)) = (
            reqwest::header::HeaderName::from_bytes(key.as_str().as_bytes()),
            reqwest::header::HeaderValue::from_bytes(value.as_bytes()),
        ) {
            headers.insert(name, val);
        }
    }

    // Read body
    let body_bytes = match axum::body::to_bytes(request.into_body(), 10 * 1024 * 1024).await {
        Ok(b) => b,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": format!("Failed to read request body: {}", e)})),
            )
                .into_response();
        }
    };

    // Forward to runtara-core
    let resp = match client
        .request(method, &target_url)
        .headers(headers)
        .body(body_bytes)
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            return (
                StatusCode::BAD_GATEWAY,
                Json(json!({"error": format!("Failed to reach runtara-core: {}", e)})),
            )
                .into_response();
        }
    };

    // Forward response
    let status =
        StatusCode::from_u16(resp.status().as_u16()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    let resp_headers = resp.headers().clone();
    let resp_body = resp.bytes().await.unwrap_or_default();

    let mut response = Response::builder().status(status);
    for (key, value) in resp_headers.iter() {
        if key != "transfer-encoding" {
            response = response.header(key.as_str(), value.as_bytes());
        }
    }
    response
        .body(axum::body::Body::from(resp_body))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}
