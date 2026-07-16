//! Workflow HTTP Handlers
//!
//! Thin HTTP layer that:
//! - Extracts HTTP parameters (path, query, body, headers)
//! - Validates tenant authentication
//! - Delegates business logic to WorkflowService
//! - Maps service errors to HTTP status codes
//! - Returns standardized API responses

// Allow dead code temporarily - handlers will be wired up in routing layer
#![allow(dead_code)]

use axum::{
    extract::{Extension, Path, Query, State},
    http::StatusCode,
    response::Json,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::PgPool;
use std::sync::Arc;
use tracing::instrument;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::api::dto::common::{ApiResponse, ErrorResponse};
use crate::api::dto::workflows::{
    CheckpointMetadataDto, CompileWorkflowResponse, ExecuteWorkflowRequest,
    ExecuteWorkflowResponse, FoldersResponse, GetDependenciesResponse, GetDependentsResponse,
    ListCheckpointsQuery, ListCheckpointsResponse, ListInstancesQuery, ListStepTypesResponse,
    MoveWorkflowRequest, MoveWorkflowResponse, PageWorkflowDto, PageWorkflowInstanceHistoryDto,
    RenameFolderRequest, RenameFolderResponse, StepTypeInfo, UpdateTrackEventsRequest,
    VersionSchemasResponse, WorkflowDependency, WorkflowDependent, WorkflowDto,
    WorkflowInstanceDto, WorkflowValidationErrorResponse, WorkflowVersionInfoDto,
    validate_workflow_inputs,
};
use crate::api::handlers::common::{execution_error_response, execution_error_response_with};
use crate::api::repositories::workflows::WorkflowRepository;
use crate::api::services::workflows::{ServiceError, WorkflowService};
use crate::auth::AuthContext;
use crate::middleware::tenant_auth::Source;
use crate::product_events::{EventSource, EventType, ProductEvent, ProductEventSink};
use crate::runtime_client::RuntimeClient;
use crate::workers::execution_engine::{
    ExecutionEngine, PauseOutcome, QueueRequest, ResumeOutcome, StopOutcome, TriggerSource,
};
use runtara_connections::ConnectionsFacade;

use crate::types::MemoryTier;

// ============================================================================
// Request/Response Types
// ============================================================================

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct CreateWorkflowRequest {
    pub name: String,
    pub description: String,
    #[serde(default)]
    #[serde(rename = "memoryTier")]
    pub memory_tier: Option<MemoryTier>,
    /// Enable step-event tracking for this workflow version (default: true)
    #[serde(default)]
    #[serde(rename = "trackEvents")]
    pub track_events: Option<bool>,
    /// Capability id override (lowercase kebab, ≤64 chars). Auto-derived from
    /// the name when absent; a supplied slug that is taken or reserved is a 409.
    #[serde(default)]
    pub slug: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct UpdateWorkflowSlugRequest {
    /// The new slug (lowercase kebab, ≤64 chars, per-tenant unique).
    pub slug: String,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct UpdateWorkflowRequest {
    /// The execution graph containing workflow definition.
    /// Must include 'name' and optionally 'description' fields.
    #[serde(rename = "executionGraph")]
    pub execution_graph: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "memoryTier")]
    pub memory_tier: Option<MemoryTier>,
    /// Enable step-event tracking for this workflow version (optional, keeps existing if not provided)
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "trackEvents")]
    pub track_events: Option<bool>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct ListWorkflowsQuery {
    pub page: Option<i32>,
    #[serde(rename = "pageSize")]
    pub page_size: Option<i32>,
    /// Filter by folder path (e.g., "/Sales/")
    /// If not provided, returns all workflows (backward compatible)
    pub path: Option<String>,
    /// If true and path is provided, includes workflows in subfolders
    #[serde(default)]
    pub recursive: bool,
    /// Search workflows by name (case-insensitive substring match)
    pub search: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct GetWorkflowQuery {
    #[serde(rename = "versionNumber")]
    pub version_number: Option<i32>,
}

#[derive(Debug, Deserialize, Serialize, ToSchema)]
pub struct CloneWorkflowRequest {
    pub name: String,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct ExecuteWorkflowQuery {
    /// Specific version number to execute (defaults to active version)
    #[serde(default)]
    #[schema(value_type = Option<i32>)]
    pub version: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct CompileWorkflowQuery {
    /// Force deleting any existing compiled artifact before compiling.
    #[serde(default, rename = "forceRecompile")]
    pub force_recompile: Option<bool>,
}

// ============================================================================
// HTTP Handlers
// ============================================================================

/// Create a new workflow with auto-generated ID
#[utoipa::path(
    post,
    path = "/api/runtime/workflows/create",
    request_body = CreateWorkflowRequest,
    responses(
        (status = 200, description = "Workflow created successfully", body = ApiResponse<WorkflowDto>),
        (status = 400, description = "Validation error", body = Value),
        (status = 500, description = "Internal server error", body = Value)
    ),
    tag = "workflow-controller"
)]
#[instrument(skip(pool, connections, request, agent_catalog, events, ctx, source), fields(workflow_name = %request.name))]
#[allow(clippy::too_many_arguments)]
pub async fn create_workflow_handler(
    crate::middleware::tenant_auth::OrgId(tenant_id): crate::middleware::tenant_auth::OrgId,
    crate::middleware::tenant_auth::CallerId(user_id): crate::middleware::tenant_auth::CallerId,
    State(pool): State<PgPool>,
    State(connections): State<Arc<ConnectionsFacade>>,
    State(agent_catalog): State<Arc<runtara_dsl::agent_meta::AgentCatalog>>,
    State(events): State<ProductEventSink>,
    Extension(ctx): Extension<AuthContext>,
    Source(source): Source,
    Json(request): Json<CreateWorkflowRequest>,
) -> (StatusCode, Json<Value>) {
    // Create repository and service
    let repository = Arc::new(WorkflowRepository::new(pool.clone()));
    let service = WorkflowService::new(repository, connections.clone(), agent_catalog.clone());

    // Delegate to service
    match service
        .create_workflow(
            &tenant_id,
            request.name,
            request.description,
            request.memory_tier,
            request.track_events,
            &user_id,
            request.slug,
        )
        .await
    {
        Ok(workflow_dto) => {
            events.emit(
                ProductEvent::from_auth(EventType::WorkflowCreated, &ctx)
                    .resource(workflow_dto.id.as_str(), "workflow")
                    .source(source),
            );
            // Creating a workflow also creates its first immutable version row — the same
            // "a new workflow version was created" transition `update_workflow_handler` reports
            // for every subsequent version.
            events.emit(
                ProductEvent::from_auth(EventType::WorkflowVersionRegistered, &ctx)
                    .resource(workflow_dto.id.as_str(), "workflow")
                    .properties(json!({ "version": workflow_dto.last_version_number }))
                    .source(source),
            );
            let response =
                ApiResponse::success_with_message("Workflow created successfully", workflow_dto);
            (
                StatusCode::OK,
                Json(serde_json::to_value(response).unwrap()),
            )
        }
        Err(e) => {
            // `maxWorkflows` is the only entitlement gate `create_workflow` can hit — this
            // filters to that (a feature-gate denial would no-op here, but there isn't one on
            // this path today).
            if let ServiceError::EntitlementDenied(ref denial) = e {
                crate::product_events::emit_quota_exceeded(
                    &events,
                    ProductEvent::from_auth(EventType::QuotaExceeded, &ctx).source(source),
                    denial,
                );
            }
            map_service_error_to_response(e)
        }
    }
}

/// Walk a workflow's execution graph and collect the distinct `(agent_id, capability_id)`
/// pairs it references, recursing into Split/While subgraphs and a WaitForSignal `onWait`
/// branch. Best-effort: an unparseable graph yields no pairs (the save path itself owns
/// validation, so this only runs to feed `agent.capability_used` analytics).
fn collect_workflow_capabilities(execution_graph: &Value) -> Vec<(String, String)> {
    fn walk(
        graph: &runtara_dsl::ExecutionGraph,
        out: &mut std::collections::BTreeSet<(String, String)>,
    ) {
        for step in graph.steps.values() {
            match step {
                runtara_dsl::Step::Agent(agent) => {
                    out.insert((agent.agent_id.clone(), agent.capability_id.clone()));
                }
                runtara_dsl::Step::Split(split) => walk(&split.subgraph, out),
                runtara_dsl::Step::While(while_step) => walk(&while_step.subgraph, out),
                runtara_dsl::Step::WaitForSignal(wait) => {
                    if let Some(on_wait) = &wait.on_wait {
                        walk(on_wait, out);
                    }
                }
                _ => {}
            }
        }
    }

    let Ok(graph) = serde_json::from_value::<runtara_dsl::ExecutionGraph>(execution_graph.clone())
    else {
        return Vec::new();
    };
    let mut pairs = std::collections::BTreeSet::new();
    walk(&graph, &mut pairs);
    pairs.into_iter().collect()
}

/// Emit one `agent.capability_used` event per distinct capability referenced by a saved
/// graph, attributed to the saving caller/surface and scoped to the workflow.
fn emit_capability_used(
    events: &ProductEventSink,
    ctx: &AuthContext,
    source: EventSource,
    workflow_id: &str,
    capabilities: &[(String, String)],
) {
    for (agent_id, capability_id) in capabilities {
        events.emit(
            ProductEvent::from_auth(EventType::AgentCapabilityUsed, ctx)
                .resource(workflow_id, "workflow")
                .properties(json!({ "agentId": agent_id, "capabilityId": capability_id }))
                .source(source),
        );
    }
}

/// Update a workflow by creating a new version
#[utoipa::path(
    post,
    path = "/api/runtime/workflows/{id}/update",
    request_body = UpdateWorkflowRequest,
    params(
        ("id" = String, Path, description = "Workflow identifier")
    ),
    responses(
        (status = 200, description = "Workflow version stored successfully", body = Value),
        (status = 400, description = "Workflow validation error with step context", body = WorkflowValidationErrorResponse),
        (status = 404, description = "Workflow not found", body = Value),
        (status = 500, description = "Internal server error", body = Value)
    ),
    tag = "workflow-controller"
)]
#[allow(clippy::too_many_arguments)]
pub async fn update_workflow_handler(
    crate::middleware::tenant_auth::OrgId(tenant_id): crate::middleware::tenant_auth::OrgId,
    crate::middleware::tenant_auth::Caller { user_id, role }: crate::middleware::tenant_auth::Caller,
    State(pool): State<PgPool>,
    State(connections): State<Arc<ConnectionsFacade>>,
    State(agent_catalog): State<Arc<runtara_dsl::agent_meta::AgentCatalog>>,
    State(_runtime_client): State<Option<Arc<RuntimeClient>>>,
    Path(workflow_id): Path<String>,
    State(events): State<ProductEventSink>,
    Extension(ctx): Extension<AuthContext>,
    Source(source): Source,
    Json(request): Json<UpdateWorkflowRequest>,
) -> (StatusCode, Json<Value>) {
    // Create repositories and service
    let repository = Arc::new(WorkflowRepository::new(pool.clone()));

    // Own-scoped authorization: a Member may update only workflows they created. Dormant
    // unless membership enforcement is Required (Owner/Admin and non-SaaS modes pass).
    let owner = repository
        .owner(&tenant_id, &workflow_id)
        .await
        .ok()
        .flatten();
    if let Err(denial) = crate::middleware::authorization::require_ownership(
        crate::auth::membership_policy(),
        &tenant_id,
        role,
        crate::authz::Permission::WorkflowUpdate,
        owner.as_deref(),
        &user_id,
    ) {
        return (StatusCode::FORBIDDEN, Json(denial.json_body()));
    }

    let service = WorkflowService::new(
        repository.clone(),
        connections.clone(),
        agent_catalog.clone(),
    );

    // Collect referenced capabilities before the graph is moved into the service, so a
    // successful save can emit one `agent.capability_used` per distinct capability.
    let capabilities = collect_workflow_capabilities(&request.execution_graph);

    // Delegate to service (name/description are now inside execution_graph)
    let (version_num, warnings) = match service
        .update_workflow(
            &tenant_id,
            &workflow_id,
            request.execution_graph,
            request.memory_tier,
            request.track_events,
        )
        .await
    {
        Ok(result) => {
            events.emit(
                ProductEvent::from_auth(EventType::WorkflowUpdated, &ctx)
                    .resource(&workflow_id, "workflow")
                    .source(source),
            );
            // Distinct from `workflow.updated`: this handler always creates a new immutable
            // version row (unlike `patch_version_graph_handler`, which mutates one in place and
            // does not create a version), so this is the one place that genuinely reports
            // "a new workflow version was created".
            events.emit(
                ProductEvent::from_auth(EventType::WorkflowVersionRegistered, &ctx)
                    .resource(&workflow_id, "workflow")
                    .properties(json!({ "version": result.0 }))
                    .source(source),
            );
            emit_capability_used(&events, &ctx, source, &workflow_id, &capabilities);
            result
        }
        Err(e) => return map_service_error_to_response(e),
    };

    // Queue compilation asynchronously instead of blocking
    // The compilation worker will process this in the background
    let compilation_status = if let Some(valkey_config) = crate::valkey::ValkeyConfig::from_env() {
        let redis_url = valkey_config.connection_url();
        // This compile is a side effect of the update — hand the worker a pre-built event
        // attributed to the updating caller/surface, so the resulting `workflow.compiled` is
        // not an anonymous worker event.
        let compiled_event = ProductEvent::from_auth(EventType::WorkflowCompiled, &ctx)
            .resource(&workflow_id, "workflow")
            .source(source);
        match crate::workers::compilation_worker::enqueue_compilation_with_event(
            &redis_url,
            &tenant_id,
            &workflow_id,
            version_num,
            false,
            compiled_event,
        )
        .await
        {
            Ok(enqueued) => {
                // Stamp a "queued" progress entry so the frontend's first
                // poll sees a real stage instead of `unknown`. Cheap; only
                // runs when the redis manager is already initialized.
                if let Ok(m) = crate::valkey::get_or_create_manager(&redis_url).await {
                    crate::valkey::compilation_progress::mark_queued(
                        &m,
                        &tenant_id,
                        &workflow_id,
                        version_num,
                    )
                    .await;
                }
                if enqueued {
                    "queued"
                } else {
                    "already_pending"
                }
            }
            Err(e) => {
                tracing::warn!(
                    tenant_id = %tenant_id,
                    workflow_id = %workflow_id,
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
            workflow_id = %workflow_id,
            version = version_num,
            "Valkey not configured, compilation must be triggered manually"
        );
        "manual_required"
    };

    let response = json!({
        "success": true,
        "message": "Workflow saved successfully",
        "workflowId": workflow_id,
        "version": version_num.to_string(),
        "timestamp": chrono::Utc::now().to_rfc3339(),
        "warnings": warnings,
        "compilation": {
            "status": compilation_status
        }
    });
    (StatusCode::OK, Json(response))
}

/// Patch a workflow version's execution graph in-place (no new version created)
#[utoipa::path(
    put,
    path = "/api/runtime/workflows/{id}/versions/{version}/graph",
    request_body = UpdateWorkflowRequest,
    params(
        ("id" = String, Path, description = "Workflow identifier"),
        ("version" = i32, Path, description = "Version number to patch")
    ),
    responses(
        (status = 200, description = "Version graph updated in-place", body = Value),
        (status = 400, description = "Validation error", body = Value),
        (status = 404, description = "Version not found", body = Value),
        (status = 500, description = "Internal server error", body = Value)
    ),
    tag = "workflow-controller"
)]
#[allow(clippy::too_many_arguments)]
pub async fn patch_version_graph_handler(
    crate::middleware::tenant_auth::OrgId(tenant_id): crate::middleware::tenant_auth::OrgId,
    crate::middleware::tenant_auth::Caller { user_id, role }: crate::middleware::tenant_auth::Caller,
    State(pool): State<PgPool>,
    State(connections): State<Arc<ConnectionsFacade>>,
    State(agent_catalog): State<Arc<runtara_dsl::agent_meta::AgentCatalog>>,
    State(events): State<ProductEventSink>,
    Extension(ctx): Extension<AuthContext>,
    Source(source): Source,
    Path((workflow_id, version)): Path<(String, i32)>,
    Json(request): Json<UpdateWorkflowRequest>,
) -> (StatusCode, Json<Value>) {
    let repository = Arc::new(WorkflowRepository::new(pool.clone()));

    // Own-scoped authorization: a Member may edit only workflows they created.
    let owner = repository
        .owner(&tenant_id, &workflow_id)
        .await
        .ok()
        .flatten();
    if let Err(denial) = crate::middleware::authorization::require_ownership(
        crate::auth::membership_policy(),
        &tenant_id,
        role,
        crate::authz::Permission::WorkflowUpdate,
        owner.as_deref(),
        &user_id,
    ) {
        return (StatusCode::FORBIDDEN, Json(denial.json_body()));
    }

    let service = WorkflowService::new(repository, connections.clone(), agent_catalog.clone());

    // Collect referenced capabilities before the graph is moved into the service.
    let capabilities = collect_workflow_capabilities(&request.execution_graph);

    let warnings = match service
        .patch_version_graph(&tenant_id, &workflow_id, version, request.execution_graph)
        .await
    {
        Ok(warnings) => warnings,
        Err(e) => return map_service_error_to_response(e),
    };

    events.emit(
        ProductEvent::from_auth(EventType::WorkflowUpdated, &ctx)
            .resource(&workflow_id, "workflow")
            .source(source)
            .properties(json!({ "version": version })),
    );
    emit_capability_used(&events, &ctx, source, &workflow_id, &capabilities);

    let response = json!({
        "success": true,
        "message": "Version graph updated in-place",
        "workflowId": workflow_id,
        "version": version.to_string(),
        "timestamp": chrono::Utc::now().to_rfc3339(),
        "warnings": warnings,
    });
    (StatusCode::OK, Json(response))
}

/// Toggle step-event tracking for a specific workflow version
#[utoipa::path(
    put,
    path = "/api/runtime/workflows/{id}/versions/{version}/track-events",
    request_body = UpdateTrackEventsRequest,
    params(
        ("id" = String, Path, description = "Workflow identifier"),
        ("version" = i32, Path, description = "Version number")
    ),
    responses(
        (status = 200, description = "Track-events mode updated successfully", body = ApiResponse<WorkflowDto>),
        (status = 400, description = "Validation error", body = Value),
        (status = 404, description = "Workflow or version not found", body = Value),
        (status = 500, description = "Internal server error", body = Value)
    ),
    tag = "workflow-controller"
)]
#[allow(clippy::too_many_arguments)]
#[instrument(skip(pool, connections, request, agent_catalog, user_id, role), fields(workflow_id = %workflow_id, version = %version))]
pub async fn toggle_track_events_handler(
    crate::middleware::tenant_auth::OrgId(tenant_id): crate::middleware::tenant_auth::OrgId,
    crate::middleware::tenant_auth::Caller { user_id, role }: crate::middleware::tenant_auth::Caller,
    State(pool): State<PgPool>,
    State(connections): State<Arc<ConnectionsFacade>>,
    State(agent_catalog): State<Arc<runtara_dsl::agent_meta::AgentCatalog>>,

    Path((workflow_id, version)): Path<(String, i32)>,
    Json(request): Json<UpdateTrackEventsRequest>,
) -> (StatusCode, Json<Value>) {
    // Create repository and service
    let repository = Arc::new(WorkflowRepository::new(pool.clone()));

    // Own-scoped authorization: a Member may edit only workflows they created.
    let owner = repository
        .owner(&tenant_id, &workflow_id)
        .await
        .ok()
        .flatten();
    if let Err(denial) = crate::middleware::authorization::require_ownership(
        crate::auth::membership_policy(),
        &tenant_id,
        role,
        crate::authz::Permission::WorkflowUpdate,
        owner.as_deref(),
        &user_id,
    ) {
        return (StatusCode::FORBIDDEN, Json(denial.json_body()));
    }

    let service = WorkflowService::new(repository, connections.clone(), agent_catalog.clone());

    // Delegate to service
    match service
        .toggle_track_events(&tenant_id, &workflow_id, version, request.track_events)
        .await
    {
        Ok(workflow_dto) => {
            let response = ApiResponse::success_with_message(
                "Track-events mode updated successfully. Compilation invalidated, will recompile on next execution.",
                workflow_dto,
            );
            (
                StatusCode::OK,
                Json(serde_json::to_value(response).unwrap()),
            )
        }
        Err(e) => map_service_error_to_response(e),
    }
}

/// Update a workflow's slug — the stable capability id a workflow-as-agent
/// exports. Identity-level write (never rides the graph-JSON path); always
/// allowed — a parent that composed `agent-<oldslug>` keeps that pin until it
/// recompiles.
#[utoipa::path(
    put,
    path = "/api/runtime/workflows/{id}/slug",
    request_body = UpdateWorkflowSlugRequest,
    params(
        ("id" = String, Path, description = "Workflow identifier")
    ),
    responses(
        (status = 200, description = "Slug updated successfully", body = Value),
        (status = 400, description = "Invalid slug", body = Value),
        (status = 404, description = "Workflow not found", body = Value),
        (status = 409, description = "Slug already taken or reserved", body = Value),
        (status = 500, description = "Internal server error", body = Value)
    ),
    tag = "workflow-controller"
)]
#[instrument(skip(pool, connections, request, agent_catalog, user_id, role), fields(workflow_id = %workflow_id))]
pub async fn update_workflow_slug_handler(
    crate::middleware::tenant_auth::OrgId(tenant_id): crate::middleware::tenant_auth::OrgId,
    crate::middleware::tenant_auth::Caller { user_id, role }: crate::middleware::tenant_auth::Caller,
    State(pool): State<PgPool>,
    State(connections): State<Arc<ConnectionsFacade>>,
    State(agent_catalog): State<Arc<runtara_dsl::agent_meta::AgentCatalog>>,
    Path(workflow_id): Path<String>,
    Json(request): Json<UpdateWorkflowSlugRequest>,
) -> (StatusCode, Json<Value>) {
    let repository = Arc::new(WorkflowRepository::new(pool.clone()));

    // Own-scoped authorization: a Member may edit only workflows they created.
    let owner = repository
        .owner(&tenant_id, &workflow_id)
        .await
        .ok()
        .flatten();
    if let Err(denial) = crate::middleware::authorization::require_ownership(
        crate::auth::membership_policy(),
        &tenant_id,
        role,
        crate::authz::Permission::WorkflowUpdate,
        owner.as_deref(),
        &user_id,
    ) {
        return (StatusCode::FORBIDDEN, Json(denial.json_body()));
    }

    let service = WorkflowService::new(repository, connections.clone(), agent_catalog.clone());
    match service
        .update_workflow_slug(&tenant_id, &workflow_id, &request.slug)
        .await
    {
        Ok(slug) => (
            StatusCode::OK,
            Json(json!({
                "success": true,
                "message": "Slug updated successfully",
                "data": { "slug": slug }
            })),
        ),
        Err(e) => map_service_error_to_response(e),
    }
}

/// Publish a workflow AS an agent: compile the current (or latest) version
/// with the AgentCapabilities ABI, synthesize catalog metadata from its
/// input/output schemas, and stage both into the tenant's workflow-agent dir.
/// Any parent workflow can then target it as `agentId: <slug>,
/// capabilityId: "run"`.
#[utoipa::path(
    post,
    path = "/api/runtime/workflows/{id}/publish-agent",
    params(
        ("id" = String, Path, description = "Workflow identifier")
    ),
    responses(
        (status = 200, description = "Workflow published as agent", body = Value),
        (status = 404, description = "Workflow not found", body = Value),
        (status = 409, description = "Workflow has no slug", body = Value),
        (status = 500, description = "Compilation or staging failed", body = Value)
    ),
    tag = "workflow-controller"
)]
#[allow(clippy::too_many_arguments)]
#[instrument(skip(pool, runtime_client, agent_catalog, user_id, role), fields(workflow_id = %workflow_id))]
pub async fn publish_workflow_agent_handler(
    crate::middleware::tenant_auth::OrgId(tenant_id): crate::middleware::tenant_auth::OrgId,
    crate::middleware::tenant_auth::Caller { user_id, role }: crate::middleware::tenant_auth::Caller,
    State(pool): State<PgPool>,
    State(runtime_client): State<Option<Arc<crate::runtime_client::RuntimeClient>>>,
    State(agent_catalog): State<Arc<runtara_dsl::agent_meta::AgentCatalog>>,
    Path(workflow_id): Path<String>,
) -> (StatusCode, Json<Value>) {
    let repository = Arc::new(WorkflowRepository::new(pool.clone()));

    // Own-scoped authorization, like every other workflow mutation.
    let owner = repository
        .owner(&tenant_id, &workflow_id)
        .await
        .ok()
        .flatten();
    if let Err(denial) = crate::middleware::authorization::require_ownership(
        crate::auth::membership_policy(),
        &tenant_id,
        role,
        crate::authz::Permission::WorkflowUpdate,
        owner.as_deref(),
        &user_id,
    ) {
        return (StatusCode::FORBIDDEN, Json(denial.json_body()));
    }

    // The slug is the capability id — a publish without one is ambiguous.
    let slug = match repository.get_slug(&tenant_id, &workflow_id).await {
        Ok(Some(slug)) => slug,
        Ok(None) => {
            return (
                StatusCode::CONFLICT,
                Json(json!({
                    "success": false,
                    "message": "Workflow has no slug; set one via PUT /api/runtime/workflows/{id}/slug before publishing"
                })),
            );
        }
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"success": false, "message": format!("Failed to load slug: {e}")})),
            );
        }
    };

    // Publish the CURRENT (or latest) version — the same resolution execute uses.
    let version = match repository
        .get_current_or_latest_version(&tenant_id, &workflow_id)
        .await
    {
        Ok(Some(version)) => version,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({"success": false, "message": "Workflow not found"})),
            );
        }
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(
                    json!({"success": false, "message": format!("Failed to resolve version: {e}")}),
                ),
            );
        }
    };

    let connection_service_url = std::env::var("CONNECTION_SERVICE_URL").ok();
    let compilation_service = crate::api::services::compilation::CompilationService::new(
        repository,
        connection_service_url,
        runtime_client,
    )
    .with_agent_catalog(agent_catalog)
    .with_direct_compilation(
        crate::api::services::compilation::direct_compilation_settings_from_config(),
    );

    match compilation_service
        .publish_workflow_agent(&tenant_id, &workflow_id, version, slug)
        .await
    {
        Ok(result) => (
            StatusCode::OK,
            Json(json!({
                "success": true,
                "message": "Workflow published as agent",
                "data": result
            })),
        ),
        Err(e) => {
            use crate::api::services::compilation::ServiceError as CompilationServiceError;
            let status = match &e {
                CompilationServiceError::NotFound(_) => StatusCode::NOT_FOUND,
                _ => StatusCode::INTERNAL_SERVER_ERROR,
            };
            (
                status,
                Json(json!({"success": false, "message": e.to_string()})),
            )
        }
    }
}

/// List all workflows for a tenant with pagination and optional folder filtering
#[utoipa::path(
    get,
    path = "/api/runtime/workflows",
    params(
        ("page" = Option<i32>, Query, description = "Page number (1-based, default: 1)"),
        ("pageSize" = Option<i32>, Query, description = "Page size (default: 20, max: 100)"),
        ("path" = Option<String>, Query, description = "Filter by folder path (e.g., '/Sales/'). If not provided, returns all workflows."),
        ("recursive" = bool, Query, description = "If true and path is provided, includes workflows in subfolders (default: false)"),
        ("search" = Option<String>, Query, description = "Search workflows by name (case-insensitive substring match)")
    ),
    responses(
        (status = 200, description = "List of workflows retrieved successfully", body = ApiResponse<PageWorkflowDto>),
        (status = 400, description = "Invalid path format", body = Value),
        (status = 500, description = "Internal server error", body = Value)
    ),
    tag = "workflow-controller"
)]
pub async fn list_workflows_handler(
    crate::middleware::tenant_auth::OrgId(tenant_id): crate::middleware::tenant_auth::OrgId,
    State(pool): State<PgPool>,
    State(connections): State<Arc<ConnectionsFacade>>,
    State(agent_catalog): State<Arc<runtara_dsl::agent_meta::AgentCatalog>>,

    Query(query): Query<ListWorkflowsQuery>,
) -> (StatusCode, Json<Value>) {
    let handler_start = std::time::Instant::now();

    // Create repository and service
    let repository = Arc::new(WorkflowRepository::new(pool.clone()));
    let service = WorkflowService::new(repository, connections.clone(), agent_catalog.clone());

    // Delegate to service with pagination
    let page = query.page.unwrap_or(1);
    let page_size = query.page_size.unwrap_or(20);

    let query_start = std::time::Instant::now();

    match service
        .list_workflows(
            &tenant_id,
            page,
            page_size,
            query.path.as_deref(),
            query.recursive,
            query.search.as_deref(),
        )
        .await
    {
        Ok((workflows, total, current_page, current_page_size)) => {
            let query_duration = query_start.elapsed();
            let total_duration = handler_start.elapsed();
            tracing::debug!(
                query_ms = query_duration.as_millis(),
                total_ms = total_duration.as_millis(),
                workflow_count = workflows.len(),
                total_count = total,
                "list_workflows: completed"
            );
            let page_dto = PageWorkflowDto::new(workflows, total, current_page, current_page_size);
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
                "list_workflows: failed"
            );
            map_service_error_to_response(e)
        }
    }
}

/// Get a specific workflow by ID and optional version
#[utoipa::path(
    get,
    path = "/api/runtime/workflows/{id}",
    params(
        ("id" = String, Path, description = "Workflow identifier"),
        ("versionNumber" = Option<i32>, Query, description = "Version number (defaults to latest)")
    ),
    responses(
        (status = 200, description = "Workflow retrieved successfully", body = ApiResponse<WorkflowDto>),
        (status = 404, description = "Workflow not found", body = Value),
        (status = 500, description = "Internal server error", body = Value)
    ),
    tag = "workflow-controller"
)]
pub async fn get_workflow_handler(
    crate::middleware::tenant_auth::OrgId(tenant_id): crate::middleware::tenant_auth::OrgId,
    State(pool): State<PgPool>,
    State(connections): State<Arc<ConnectionsFacade>>,
    State(agent_catalog): State<Arc<runtara_dsl::agent_meta::AgentCatalog>>,

    Path(workflow_id): Path<String>,
    Query(query): Query<GetWorkflowQuery>,
) -> (StatusCode, Json<Value>) {
    // Create repository and service
    let repository = Arc::new(WorkflowRepository::new(pool.clone()));
    let service = WorkflowService::new(repository, connections.clone(), agent_catalog.clone());

    // Delegate to service
    match service
        .get_workflow(&tenant_id, &workflow_id, query.version_number)
        .await
    {
        Ok(workflow_dto) => {
            let response = ApiResponse::success(workflow_dto);
            (
                StatusCode::OK,
                Json(serde_json::to_value(response).unwrap()),
            )
        }
        Err(e) => map_service_error_to_response(e),
    }
}

/// Get all versions of a specific workflow
#[utoipa::path(
    get,
    path = "/api/runtime/workflows/{id}/versions",
    params(
        ("id" = String, Path, description = "Workflow identifier")
    ),
    responses(
        (status = 200, description = "Workflow versions retrieved successfully", body = ApiResponse<Vec<WorkflowVersionInfoDto>>),
        (status = 404, description = "Workflow not found", body = Value),
        (status = 500, description = "Internal server error", body = Value)
    ),
    tag = "workflow-controller"
)]
pub async fn list_workflow_versions_handler(
    crate::middleware::tenant_auth::OrgId(tenant_id): crate::middleware::tenant_auth::OrgId,
    State(pool): State<PgPool>,
    State(connections): State<Arc<ConnectionsFacade>>,
    State(agent_catalog): State<Arc<runtara_dsl::agent_meta::AgentCatalog>>,

    Path(workflow_id): Path<String>,
) -> (StatusCode, Json<Value>) {
    // Create repository and service
    let repository = Arc::new(WorkflowRepository::new(pool.clone()));
    let service = WorkflowService::new(repository, connections.clone(), agent_catalog.clone());

    // Delegate to service
    match service.list_versions(&tenant_id, &workflow_id).await {
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

/// Delete a workflow and all its versions (soft delete)
#[utoipa::path(
    post,
    path = "/api/runtime/workflows/{id}/delete",
    params(
        ("id" = String, Path, description = "Workflow identifier")
    ),
    responses(
        (status = 200, description = "Workflow deleted successfully", body = Value),
        (status = 404, description = "Workflow not found", body = Value),
        (status = 500, description = "Internal server error", body = Value)
    ),
    tag = "workflow-controller"
)]
#[allow(clippy::too_many_arguments)]
pub async fn delete_workflow_handler(
    crate::middleware::tenant_auth::OrgId(tenant_id): crate::middleware::tenant_auth::OrgId,
    crate::middleware::tenant_auth::Caller { user_id, role }: crate::middleware::tenant_auth::Caller,
    State(pool): State<PgPool>,
    State(connections): State<Arc<ConnectionsFacade>>,
    State(agent_catalog): State<Arc<runtara_dsl::agent_meta::AgentCatalog>>,
    State(events): State<ProductEventSink>,
    Extension(ctx): Extension<AuthContext>,
    Source(source): Source,
    Path(workflow_id): Path<String>,
) -> (StatusCode, Json<Value>) {
    // Create repository and service
    let repository = Arc::new(WorkflowRepository::new(pool.clone()));

    // Own-scoped authorization: a Member may delete only workflows they created.
    let owner = repository
        .owner(&tenant_id, &workflow_id)
        .await
        .ok()
        .flatten();
    if let Err(denial) = crate::middleware::authorization::require_ownership(
        crate::auth::membership_policy(),
        &tenant_id,
        role,
        crate::authz::Permission::WorkflowDelete,
        owner.as_deref(),
        &user_id,
    ) {
        return (StatusCode::FORBIDDEN, Json(denial.json_body()));
    }

    let service = WorkflowService::new(repository, connections.clone(), agent_catalog.clone());

    // Delegate to service
    match service.delete_workflow(&tenant_id, &workflow_id).await {
        Ok(rows_affected) => {
            events.emit(
                ProductEvent::from_auth(EventType::WorkflowDeleted, &ctx)
                    .resource(&workflow_id, "workflow")
                    .source(source),
            );
            let response = json!({
                "success": true,
                "message": format!("Workflow '{}' marked as deleted ({} definitions deleted)", workflow_id, rows_affected),
                "workflowId": workflow_id,
                "definitionsDeleted": rows_affected,
                "timestamp": chrono::Utc::now().to_rfc3339()
            });
            (StatusCode::OK, Json(response))
        }
        Err(e) => map_service_error_to_response(e),
    }
}

/// Clone a workflow with all its versions
#[utoipa::path(
    post,
    path = "/api/runtime/workflows/{id}/clone",
    request_body = CloneWorkflowRequest,
    params(
        ("id" = String, Path, description = "Source workflow identifier")
    ),
    responses(
        (status = 200, description = "Workflow cloned successfully", body = Value),
        (status = 400, description = "Validation error", body = Value),
        (status = 404, description = "Source workflow not found", body = Value),
        (status = 500, description = "Internal server error", body = Value)
    ),
    tag = "workflow-controller"
)]
#[allow(clippy::too_many_arguments)]
pub async fn clone_workflow_handler(
    crate::middleware::tenant_auth::OrgId(tenant_id): crate::middleware::tenant_auth::OrgId,
    crate::middleware::tenant_auth::CallerId(user_id): crate::middleware::tenant_auth::CallerId,
    State(pool): State<PgPool>,
    State(connections): State<Arc<ConnectionsFacade>>,
    State(agent_catalog): State<Arc<runtara_dsl::agent_meta::AgentCatalog>>,
    State(events): State<ProductEventSink>,
    Extension(ctx): Extension<AuthContext>,
    Source(source): Source,
    Path(workflow_id): Path<String>,
    Json(request): Json<CloneWorkflowRequest>,
) -> (StatusCode, Json<Value>) {
    // Create repository and service
    let repository = Arc::new(WorkflowRepository::new(pool.clone()));
    let service = WorkflowService::new(repository, connections.clone(), agent_catalog.clone());

    // Delegate to service
    match service
        .clone_workflow(&tenant_id, &workflow_id, &request.name, &user_id)
        .await
    {
        Ok((new_workflow_id, versions_cloned)) => {
            // A clone produces a brand-new workflow — record it as a creation, keyed on the
            // *new* workflow id.
            events.emit(
                ProductEvent::from_auth(EventType::WorkflowCreated, &ctx)
                    .resource(&new_workflow_id, "workflow")
                    .source(source),
            );
            let response = json!({
                "success": true,
                "message": format!("Workflow '{}' cloned successfully", workflow_id),
                "sourceWorkflowId": workflow_id,
                "newWorkflowId": new_workflow_id,
                "newName": request.name,
                "versionsCloned": versions_cloned,
                "timestamp": chrono::Utc::now().to_rfc3339()
            });
            (StatusCode::OK, Json(response))
        }
        Err(e) => {
            // `maxWorkflows` is the only entitlement gate `clone_workflow` can hit.
            if let ServiceError::EntitlementDenied(ref denial) = e {
                crate::product_events::emit_quota_exceeded(
                    &events,
                    ProductEvent::from_auth(EventType::QuotaExceeded, &ctx).source(source),
                    denial,
                );
            }
            map_service_error_to_response(e)
        }
    }
}

// ============================================================================
// Compilation Handlers
// ============================================================================

/// Compile a specific workflow by tenant ID, workflow ID, and version
#[utoipa::path(
    post,
    path = "/api/runtime/workflows/{id}/versions/{version}/compile",
    params(
        ("workflow_id" = String, Path, description = "Workflow identifier"),
        ("version" = String, Path, description = "Version number (positive integer)")
    ),
    responses(
        (status = 200, description = "Workflow compiled successfully", body = CompileWorkflowResponse),
        (status = 400, description = "Invalid version format", body = ErrorResponse),
        (status = 404, description = "Workflow not found", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    ),
    tag = "workflow-controller"
)]
#[instrument(skip(pool, runtime_client, _connections, ctx, source, events), fields(workflow_id = %workflow_id, version = %version))]
#[allow(clippy::too_many_arguments)]
pub async fn compile_workflow_handler(
    crate::middleware::tenant_auth::OrgId(tenant_id): crate::middleware::tenant_auth::OrgId,
    State(pool): State<PgPool>,
    State(runtime_client): State<Option<Arc<crate::runtime_client::RuntimeClient>>>,
    State(_connections): State<Arc<ConnectionsFacade>>,
    State(events): State<ProductEventSink>,
    Path((workflow_id, version)): Path<(String, String)>,
    Extension(ctx): Extension<AuthContext>,
    Source(source): Source,
    Query(query): Query<CompileWorkflowQuery>,
) -> (StatusCode, Json<Value>) {
    // Validate version is a positive integer
    let version_num = match version.parse::<i32>() {
        Err(_) | Ok(0) | Ok(i32::MIN..=0) => {
            let error_response = json!({
                "success": false,
                "error": "Invalid version format",
                "message": "Version must be a positive integer (greater than 0).",
                "workflowId": workflow_id,
                "version": version
            });
            return (StatusCode::BAD_REQUEST, Json(error_response));
        }
        Ok(v) => v,
    };

    // Re-walk the persisted graph — and its full EmbedWorkflow closure —
    // against the current entitlement snapshot. Update/patch already gate
    // at write time, but a graph that was valid when persisted can later
    // become invalid if the tenant's `enabled_agents` allowlist changes
    // across a restart. We must reject here even when a cached compiled
    // binary exists; otherwise the "already compiled" fast path below would
    // let a stale workflow keep running with a now-forbidden agent, whether
    // that agent lives in the root graph or in a previously-saved embedded
    // child (children only ever got structural validation at save time, not
    // this allowlist check).
    {
        let repository = WorkflowRepository::new(pool.clone());
        match repository
            .get_definition(&tenant_id, &workflow_id, version_num)
            .await
        {
            Ok(Some(definition)) => {
                let workflow_wrapper = serde_json::json!({ "executionGraph": definition.clone() });
                if let Ok(workflow) =
                    serde_json::from_value::<runtara_dsl::Workflow>(workflow_wrapper)
                {
                    let child_graphs: Vec<runtara_dsl::ExecutionGraph> =
                        match crate::compiler::child_workflows::load_child_workflows_for_validation(
                            &pool,
                            &tenant_id,
                            &definition,
                        )
                        .await
                        {
                            Ok(child_infos) => child_infos
                                .into_iter()
                                .filter_map(|info| {
                                    serde_json::from_value::<runtara_dsl::ExecutionGraph>(
                                        info.execution_graph,
                                    )
                                    .ok()
                                })
                                .collect(),
                            Err(e) => {
                                tracing::warn!(
                                    error = %e,
                                    "Failed to load embedded child workflows for entitlement gate, checking root graph only"
                                );
                                Vec::new()
                            }
                        };

                    if let Err(denial) = crate::middleware::entitlement::walk_closure_for_agents(
                        crate::config::entitlements(),
                        &workflow.execution_graph,
                        child_graphs.iter(),
                        &crate::workflow_agents::published_agent_ids(&tenant_id),
                    ) {
                        return (StatusCode::FORBIDDEN, Json(denial.json_body()));
                    }
                }
                // If the persisted graph fails to parse, fall through —
                // the existing compile path will surface a meaningful error.
            }
            Ok(None) => {} // Not found; existing flow returns 404.
            Err(e) => {
                tracing::warn!(error = %e, "Failed to fetch definition for entitlement gate, falling through");
            }
        }
    }

    let force_recompile = query.force_recompile.unwrap_or(false);
    if force_recompile {
        // Stamp `queued` in Redis BEFORE invalidating the DB row. Without
        // this, the frontend's first compilation-progress poll can race
        // the handler: Redis is empty (worker hasn't started yet) and the
        // DB still holds the previous `success` row, so the endpoint
        // returns the stale terminal state — the rebuild's toolbar lights
        // up green instantly even though a real rebuild is starting. The
        // mark_queued write is a no-op if Valkey isn't configured.
        if let Some(valkey_config) = crate::valkey::ValkeyConfig::from_env() {
            let redis_url = valkey_config.connection_url();
            if let Ok(m) = crate::valkey::get_or_create_manager(&redis_url).await {
                crate::valkey::compilation_progress::mark_queued(
                    &m,
                    &tenant_id,
                    &workflow_id,
                    version_num,
                )
                .await;
            }
        }
        let repository = WorkflowRepository::new(pool.clone());
        if let Err(e) = repository
            .invalidate_compilation(&tenant_id, &workflow_id, version_num)
            .await
        {
            let error_response = json!({
                "success": false,
                "error": "Compilation invalidation failed",
                "message": format!("Failed to invalidate existing compiled artifact: {}", e),
                "workflowId": workflow_id,
                "version": version
            });
            return (StatusCode::INTERNAL_SERVER_ERROR, Json(error_response));
        }
    }

    // Route compilation through the queue if Valkey is available.
    // This ensures all compilations are serialized through the compilation worker,
    // preventing OOM from concurrent compiler processes.
    if let Some(valkey_config) = crate::valkey::ValkeyConfig::from_env() {
        let redis_url = valkey_config.connection_url();

        // Direct WASM is the only compile path now; defer the cache decision
        // to CompilationService so it can evaluate the desired compiler mode
        // before deciding whether the cache is fresh.
        tracing::debug!(
            workflow_id = %workflow_id,
            version = version_num,
            "Deferring cache decision to compilation service"
        );

        // Enqueue with a pre-built event attributed to this caller and surface. The compilation
        // worker emits it on completion, so `workflow.compiled` lands exactly once — even if the
        // wait below times out.
        let compiled_event = ProductEvent::from_auth(EventType::WorkflowCompiled, &ctx)
            .resource(&workflow_id, "workflow")
            .source(source);
        match crate::workers::compilation_worker::enqueue_compilation_with_event(
            &redis_url,
            &tenant_id,
            &workflow_id,
            version_num,
            force_recompile,
            compiled_event,
        )
        .await
        {
            Ok(_) => {
                tracing::info!(
                    tenant_id = %tenant_id,
                    workflow_id = %workflow_id,
                    version = version_num,
                    "Compilation request queued via API"
                );
            }
            Err(e) => {
                let error_response = json!({
                    "success": false,
                    "error": "Failed to queue compilation",
                    "message": format!("Failed to enqueue compilation: {}", e),
                    "workflowId": workflow_id,
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
            &workflow_id,
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
                    "Compilation for workflow '{}' version {} timed out after 5 minutes",
                    workflow_id, version_num
                ),
                "workflowId": workflow_id,
                "version": version
            });
            return (StatusCode::GATEWAY_TIMEOUT, Json(error_response));
        }

        // Query DB for the compilation result
        return match query_compilation_result(&pool, &tenant_id, &workflow_id, version_num).await {
            Ok(result) => {
                // `workflow.compiled` is emitted by the compilation worker (see enqueue above),
                // so this handler does not emit it.
                if result.success {
                    let mut response = json!({
                        "success": true,
                        "message": "Workflow compiled successfully",
                        "workflowId": workflow_id,
                        "version": version,
                        "recompiled": true,
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
                        "workflowId": workflow_id,
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
                    "workflowId": workflow_id,
                    "version": version
                });
                (StatusCode::INTERNAL_SERVER_ERROR, Json(error_response))
            }
        };
    }

    // Fallback: Valkey not configured, compile directly (still protected by semaphore)
    tracing::warn!("Valkey not configured, compiling directly (no queue)");
    let repository = Arc::new(WorkflowRepository::new(pool));
    let connection_service_url = std::env::var("CONNECTION_SERVICE_URL").ok();
    let compilation_service = crate::api::services::compilation::CompilationService::new(
        repository,
        connection_service_url,
        runtime_client,
    )
    .with_direct_compilation(
        crate::api::services::compilation::direct_compilation_settings_from_config(),
    );

    match compilation_service
        .compile_workflow(&tenant_id, &workflow_id, version_num, force_recompile)
        .await
    {
        Ok(result) => {
            // Synchronous (no-queue) compile: the worker isn't involved, so this handler is the
            // emit point for `workflow.compiled` on this path.
            events.emit(
                ProductEvent::from_auth(EventType::WorkflowCompiled, &ctx)
                    .resource(&workflow_id, "workflow")
                    .source(source)
                    .properties(json!({ "success": true })),
            );
            let mut response = json!({
                "success": true,
                "message": "Workflow compiled successfully",
                "workflowId": result.workflow_id,
                "version": result.version.to_string(),
                "buildDir": result.build_dir,
                "binarySize": result.binary_size,
                "binaryChecksum": result.binary_checksum,
                "recompiled": true,
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
                "error": "Workflow not found",
                "message": msg,
                "workflowId": workflow_id,
                "version": version
            });
            (StatusCode::NOT_FOUND, Json(error_response))
        }
        Err(crate::api::services::compilation::ServiceError::CompilationError(msg)) => {
            // The compile actually ran and failed (vs. NotFound/DatabaseError, which are
            // pre-compile failures) — record it as a failed compile on the synchronous path.
            events.emit(
                ProductEvent::from_auth(EventType::WorkflowCompiled, &ctx)
                    .resource(&workflow_id, "workflow")
                    .source(source)
                    .properties(json!({ "success": false })),
            );
            let error_response = json!({
                "success": false,
                "error": "Compilation failed",
                "message": msg,
                "workflowId": workflow_id,
                "version": version
            });
            (StatusCode::INTERNAL_SERVER_ERROR, Json(error_response))
        }
        Err(crate::api::services::compilation::ServiceError::DatabaseError(msg)) => {
            let error_response = json!({
                "success": false,
                "error": "Database error",
                "message": msg,
                "workflowId": workflow_id,
                "version": version
            });
            (StatusCode::INTERNAL_SERVER_ERROR, Json(error_response))
        }
        Err(crate::api::services::compilation::ServiceError::RegistrationError(msg)) => {
            let error_response = json!({
                "success": false,
                "error": "Registration failed",
                "message": msg,
                "workflowId": workflow_id,
                "version": version
            });
            (StatusCode::INTERNAL_SERVER_ERROR, Json(error_response))
        }
    }
}

/// Response body for the compilation-progress endpoint. Mirrors the Redis
/// hash plus terminal state from `workflow_compilations`.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CompilationProgressResponse {
    /// One of `queued`, `in_progress`, `success`, `failed`, `unknown`.
    pub status: String,
    /// Stage name when status is `queued` or `in_progress`, else `null`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stage: Option<String>,
    /// 1-based stage index when in progress.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stage_index: Option<u8>,
    /// Total number of stages (constant for now).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_stages: Option<u8>,
    /// Free-text message ("Compiling agent-foo", "Linking workflow components", …).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// Epoch millis when this compilation entered the queue.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at: Option<i64>,
    /// Epoch millis of the last stage update.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<i64>,
    /// Image ID after successful registration.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_id: Option<String>,
    /// Error message when status is `failed`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
}

/// Get current compilation progress for a workflow version. Reads Redis for
/// intermediate state (queued, preparing, generating, building, composing,
/// registering); falls through to the DB for terminal state (success or
/// failed); returns `unknown` if neither has it. Designed for polling
/// (~1s) from the frontend save flow.
#[utoipa::path(
    get,
    path = "/api/runtime/workflows/{id}/versions/{version}/compilation-progress",
    params(
        ("id" = String, Path, description = "Workflow identifier"),
        ("version" = i32, Path, description = "Version number")
    ),
    responses(
        (status = 200, description = "Current compilation state", body = CompilationProgressResponse),
        (status = 400, description = "Invalid version format", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    ),
    tag = "workflow-controller"
)]
pub async fn compilation_progress_handler(
    crate::middleware::tenant_auth::OrgId(tenant_id): crate::middleware::tenant_auth::OrgId,
    State(pool): State<PgPool>,
    Path((workflow_id, version)): Path<(String, String)>,
) -> (StatusCode, Json<Value>) {
    let version_num = match version.parse::<i32>() {
        Ok(v) if v > 0 => v,
        _ => {
            let error_response = json!({
                "success": false,
                "error": "Invalid version format",
                "message": "Version must be a positive integer",
            });
            return (StatusCode::BAD_REQUEST, Json(error_response));
        }
    };

    // Redis first — covers every state from `queued` through `registering`.
    if let Some(valkey_config) = crate::valkey::ValkeyConfig::from_env() {
        let redis_url = valkey_config.connection_url();
        if let Ok(manager) = crate::valkey::get_or_create_manager(&redis_url).await
            && let Some(p) = crate::valkey::compilation_progress::read_progress(
                &manager,
                &tenant_id,
                &workflow_id,
                version_num,
            )
            .await
        {
            let status = if p.stage == "queued" {
                "queued"
            } else {
                "in_progress"
            };
            let response = CompilationProgressResponse {
                status: status.to_string(),
                stage: Some(p.stage),
                stage_index: Some(p.stage_index),
                total_stages: Some(p.total_stages),
                message: Some(p.message),
                started_at: Some(p.started_at),
                updated_at: Some(p.updated_at),
                image_id: None,
                error_message: None,
            };
            return (
                StatusCode::OK,
                Json(serde_json::to_value(response).unwrap()),
            );
        }
    }

    // Fall through to the DB for terminal state. `query_compilation_result`
    // synthesizes a fake error for the no-row case, which would look like a
    // real failure here — so we query the row directly to keep the three
    // outcomes (success / failed / unknown) distinct.
    let row: Result<Option<CompilationRow>, sqlx::Error> = sqlx::query_as(
        "SELECT compilation_status, registered_image_id, wasm_size, error_message \
         FROM workflow_compilations \
         WHERE tenant_id = $1 AND workflow_id = $2 AND version = $3",
    )
    .bind(&tenant_id)
    .bind(&workflow_id)
    .bind(version_num)
    .fetch_optional(&pool)
    .await;

    match row {
        Ok(Some(row))
            if row.compilation_status == "success" && row.registered_image_id.is_some() =>
        {
            let response = CompilationProgressResponse {
                status: "success".to_string(),
                stage: None,
                stage_index: None,
                total_stages: None,
                message: None,
                started_at: None,
                updated_at: None,
                image_id: row.registered_image_id,
                error_message: None,
            };
            (
                StatusCode::OK,
                Json(serde_json::to_value(response).unwrap()),
            )
        }
        Ok(Some(row)) if row.compilation_status == "failed" => {
            let response = CompilationProgressResponse {
                status: "failed".to_string(),
                stage: None,
                stage_index: None,
                total_stages: None,
                message: None,
                started_at: None,
                updated_at: None,
                image_id: None,
                error_message: row
                    .error_message
                    .or_else(|| Some("Compilation failed".to_string())),
            };
            (
                StatusCode::OK,
                Json(serde_json::to_value(response).unwrap()),
            )
        }
        Ok(_) => {
            // Either no row at all, or a partial row (no image yet, no
            // failure recorded). From the frontend's perspective, no
            // useful state — keep polling.
            let response = CompilationProgressResponse {
                status: "unknown".to_string(),
                stage: None,
                stage_index: None,
                total_stages: None,
                message: None,
                started_at: None,
                updated_at: None,
                image_id: None,
                error_message: None,
            };
            (
                StatusCode::OK,
                Json(serde_json::to_value(response).unwrap()),
            )
        }
        Err(e) => {
            let error_response = json!({
                "success": false,
                "error": "Database error",
                "message": format!("Failed to read compilation status: {}", e),
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

/// Raw row from workflow_compilations (avoids sqlx::query! macro which requires offline cache update)
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
    workflow_id: &str,
    version: i32,
) -> Result<CompilationQueryResult, sqlx::Error> {
    let result: Option<CompilationRow> = sqlx::query_as(
        "SELECT compilation_status, registered_image_id, wasm_size, error_message \
         FROM workflow_compilations \
         WHERE tenant_id = $1 AND workflow_id = $2 AND version = $3",
    )
    .bind(tenant_id)
    .bind(workflow_id)
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
    pub workflow_id: String,
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

/// Validate workflow mappings without full compilation
/// Returns validation issues (errors and warnings) for reference paths, types, and connections
#[utoipa::path(
    post,
    path = "/api/runtime/workflows/{id}/validate-mappings",
    params(
        ("id" = String, Path, description = "Workflow identifier"),
        ("versionNumber" = Option<i32>, Query, description = "Version number (defaults to latest)")
    ),
    responses(
        (status = 200, description = "Validation completed", body = ValidateMappingsResponse),
        (status = 404, description = "Workflow not found", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    ),
    tag = "workflow-controller"
)]
#[instrument(skip(pool, connections, agent_catalog), fields(workflow_id = %workflow_id))]
pub async fn validate_mappings_handler(
    crate::middleware::tenant_auth::OrgId(tenant_id): crate::middleware::tenant_auth::OrgId,
    State(pool): State<PgPool>,
    State(connections): State<Arc<ConnectionsFacade>>,
    State(agent_catalog): State<Arc<runtara_dsl::agent_meta::AgentCatalog>>,
    Path(workflow_id): Path<String>,
    Query(query): Query<ValidateMappingsQuery>,
) -> (StatusCode, Json<Value>) {
    // Create repositories and service
    let workflow_repository = Arc::new(WorkflowRepository::new(pool.clone()));
    let service = WorkflowService::new(
        workflow_repository,
        connections.clone(),
        agent_catalog.clone(),
    );

    // Validate mappings
    match service
        .validate_mappings(&tenant_id, &workflow_id, query.version_number)
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
                "workflowId": workflow_id,
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
                "error": "Workflow not found",
                "message": msg,
                "workflowId": workflow_id
            });
            (StatusCode::NOT_FOUND, Json(error_response))
        }
        Err(ServiceError::ValidationError(msg)) => {
            let error_response = json!({
                "success": false,
                "error": "Validation error",
                "message": msg,
                "workflowId": workflow_id
            });
            (StatusCode::BAD_REQUEST, Json(error_response))
        }
        Err(e) => {
            let error_response = json!({
                "success": false,
                "error": "Internal error",
                "message": e.to_string(),
                "workflowId": workflow_id
            });
            (StatusCode::INTERNAL_SERVER_ERROR, Json(error_response))
        }
    }
}

// ============================================================================
// Execution Handlers
// ============================================================================

/// Execute a workflow by scheduling it with inputs (defaults to active version)
#[utoipa::path(
    post,
    path = "/api/runtime/workflows/{id}/execute",
    request_body = ExecuteWorkflowRequest,
    params(
        ("id" = String, Path, description = "Workflow identifier"),
        ("version" = Option<i32>, Query, description = "Specific version to execute (defaults to current)")
    ),
    responses(
        (status = 400, description = "Validation error", body = ErrorResponse),
        (status = 200, description = "Workflow scheduled successfully", body = ExecuteWorkflowResponse),
        (status = 404, description = "Workflow not found", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    ),
    tag = "workflow-controller"
)]
#[instrument(skip(engine, request), fields(workflow_id = %workflow_id))]
pub async fn execute_workflow_handler(
    crate::middleware::tenant_auth::OrgId(tenant_id): crate::middleware::tenant_auth::OrgId,
    State(engine): State<Arc<ExecutionEngine>>,
    Path(workflow_id): Path<String>,
    Query(query): Query<ExecuteWorkflowQuery>,
    Json(request): Json<ExecuteWorkflowRequest>,
) -> (StatusCode, Json<Value>) {
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

    // Validate inputs match canonical format: {"data": {...}, "variables": {...}}
    let validated_inputs = match validate_workflow_inputs(request.inputs) {
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

    match engine
        .queue(QueueRequest {
            tenant_id: &tenant_id,
            workflow_id: &workflow_id,
            version,
            inputs: validated_inputs,
            debug,
            correlation_id: None,
            trigger_source: TriggerSource::HttpApi,
        })
        .await
    {
        Ok(result) => {
            let response_data = json!({
                "instanceId": result.instance_id.to_string(),
                "status": result.status
            });
            let response = ApiResponse::success_with_message(
                "Workflow execution queued successfully",
                response_data,
            );
            (
                StatusCode::OK,
                Json(serde_json::to_value(response).unwrap()),
            )
        }
        Err(e) => execution_error_response(&e),
    }
}

/// Query for the instance-detail endpoints. `full=true` returns the complete
/// input/output payload (including inlined base64 file uploads); omitted or any
/// other value elides large strings for a lean default fetch — the full value
/// stays retrievable via `?full=true` (used by copy-to-clipboard / MCP trace).
#[derive(Debug, Default, Deserialize)]
pub struct InstanceDetailQuery {
    #[serde(default)]
    pub full: Option<String>,
}

impl InstanceDetailQuery {
    fn want_full(&self) -> bool {
        matches!(self.full.as_deref(), Some("true") | Some("1") | Some("yes"))
    }
}

/// Get execution results for a workflow instance
#[utoipa::path(
    get,
    path = "/api/runtime/workflows/instances/{instance_id}",
    params(
        ("instance_id" = String, Path, description = "Instance identifier (UUID)")
    ),
    responses(
        (status = 200, description = "Execution results retrieved successfully", body = WorkflowInstanceDto),
        (status = 404, description = "Instance not found", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    ),
    tag = "workflow-controller"
)]
#[instrument(skip(engine), fields(instance_id = %instance_id))]
pub async fn get_execution_metrics_handler(
    crate::middleware::tenant_auth::OrgId(tenant_id): crate::middleware::tenant_auth::OrgId,
    State(engine): State<Arc<ExecutionEngine>>,
    Path(instance_id): Path<String>,
    Query(query): Query<InstanceDetailQuery>,
) -> (StatusCode, Json<Value>) {
    match engine.get_execution(&tenant_id, &instance_id).await {
        Ok(mut instance) => {
            if !query.want_full() {
                crate::workers::runtara_dto::elide_instance_io(&mut instance);
            }
            let response = ApiResponse::success(instance);
            (
                StatusCode::OK,
                Json(serde_json::to_value(response).unwrap()),
            )
        }
        Err(e) => execution_error_response(&e),
    }
}

/// Get a workflow instance by workflow_id and instance_id with all available data
#[utoipa::path(
    get,
    path = "/api/runtime/workflows/{workflow_id}/instances/{instance_id}",
    params(
        ("workflow_id" = String, Path, description = "Workflow identifier"),
        ("instance_id" = String, Path, description = "Instance identifier (UUID)")
    ),
    responses(
        (status = 200, description = "Workflow instance retrieved successfully", body = WorkflowInstanceDto),
        (status = 400, description = "Invalid instance ID format", body = ErrorResponse),
        (status = 404, description = "Instance not found", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    ),
    tag = "workflow-controller"
)]
#[instrument(skip(engine), fields(workflow_id = %workflow_id, instance_id = %instance_id))]
pub async fn get_instance_handler(
    crate::middleware::tenant_auth::OrgId(tenant_id): crate::middleware::tenant_auth::OrgId,
    State(engine): State<Arc<ExecutionEngine>>,
    Path((workflow_id, instance_id)): Path<(String, String)>,
    Query(query): Query<InstanceDetailQuery>,
) -> (StatusCode, Json<Value>) {
    match engine
        .get_execution_with_metadata(&workflow_id, &instance_id, &tenant_id)
        .await
    {
        Ok(mut execution_data) => {
            if !query.want_full() {
                crate::workers::runtara_dto::elide_instance_io(&mut execution_data.instance);
            }
            // Build extended response with metadata
            let response_data = json!({
                "instance": execution_data.instance,
                "metadata": {
                    "workflowName": execution_data.workflow_name,
                    "workflowDescription": execution_data.workflow_description,
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
        Err(e) => execution_error_response(&e),
    }
}

/// List all workflow instances for a given tenant and workflow
#[utoipa::path(
    get,
    path = "/api/runtime/workflows/{workflow_id}/instances",
    params(
        ("workflow_id" = String, Path, description = "Workflow identifier"),
        ("page" = Option<i32>, Query, description = "Page number (default: 0)"),
        ("size" = Option<i32>, Query, description = "Page size (default: 10)")
    ),
    responses(
        (status = 200, description = "Workflow instances retrieved successfully", body = PageWorkflowInstanceHistoryDto),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    ),
    tag = "workflow-controller"
)]
#[instrument(skip(engine), fields(workflow_id = %workflow_id))]
pub async fn list_instances_handler(
    crate::middleware::tenant_auth::OrgId(tenant_id): crate::middleware::tenant_auth::OrgId,
    State(engine): State<Arc<ExecutionEngine>>,
    Path(workflow_id): Path<String>,
    Query(query): Query<ListInstancesQuery>,
) -> (StatusCode, Json<Value>) {
    match engine
        .list_executions(&tenant_id, &workflow_id, query.page, query.size)
        .await
    {
        Ok(page_dto) => {
            let response = ApiResponse::success(page_dto);
            (StatusCode::OK, Json(json!(response)))
        }
        Err(e) => execution_error_response_with(&e, json!({ "workflowId": workflow_id })),
    }
}

// ============================================================================
// Checkpoint Handlers
// ============================================================================

/// List checkpoints for a workflow instance via runtara management SDK
#[utoipa::path(
    get,
    path = "/api/runtime/workflows/{workflow_id}/instances/{instance_id}/checkpoints",
    params(
        ("workflow_id" = String, Path, description = "Workflow identifier"),
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
    tag = "workflow-controller"
)]
#[instrument(skip(_pool, runtime_client), fields(instance_id = %instance_id))]
pub async fn list_instance_checkpoints_handler(
    crate::middleware::tenant_auth::OrgId(_tenant_id): crate::middleware::tenant_auth::OrgId,
    State(_pool): State<PgPool>,
    State(runtime_client): State<Option<Arc<RuntimeClient>>>,
    Path((_workflow_id, instance_id)): Path<(String, String)>,
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

/// Replay a workflow instance with the same inputs
#[utoipa::path(
    post,
    path = "/api/runtime/workflows/instances/{instance_id}/replay",
    params(
        ("instance_id" = String, Path, description = "Instance identifier (UUID)")
    ),
    responses(
        (status = 200, description = "Workflow replay scheduled successfully", body = ExecuteWorkflowResponse),
        (status = 400, description = "Invalid instance ID", body = ErrorResponse),
        (status = 404, description = "Instance not found", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    ),
    tag = "workflow-controller"
)]
#[instrument(skip(engine), fields(instance_id = %instance_id))]
pub async fn replay_instance_handler(
    crate::middleware::tenant_auth::OrgId(tenant_id): crate::middleware::tenant_auth::OrgId,
    State(engine): State<Arc<ExecutionEngine>>,
    Path(instance_id): Path<String>,
) -> (StatusCode, Json<Value>) {
    match engine.replay(&tenant_id, &instance_id).await {
        Ok(result) => {
            let response_data = json!({
                "instanceId": result.instance_id.to_string(),
                "status": result.status,
                "workflowId": result.workflow_id,
                "version": result.version,
            });
            let response = ApiResponse::success_with_message(
                "Workflow replay queued successfully",
                response_data,
            );
            (
                StatusCode::OK,
                Json(serde_json::to_value(response).unwrap()),
            )
        }
        Err(e) => execution_error_response_with(&e, json!({ "instanceId": instance_id })),
    }
}

// ============================================================================
// Control Handlers
// ============================================================================

/// Stop a running workflow instance
#[utoipa::path(
    post,
    path = "/api/runtime/workflows/instances/{instance_id}/stop",
    params(
        ("instance_id" = String, Path, description = "Instance identifier (UUID)")
    ),
    responses(
        (status = 200, description = "Instance stopped successfully", body = Value),
        (status = 400, description = "Invalid instance ID", body = ErrorResponse),
        (status = 404, description = "Instance not found", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    ),
    tag = "workflow-controller"
)]
#[instrument(skip(engine), fields(instance_id = %instance_id))]
pub async fn stop_instance_handler(
    crate::middleware::tenant_auth::OrgId(_tenant_id): crate::middleware::tenant_auth::OrgId,
    State(engine): State<Arc<ExecutionEngine>>,
    Path(instance_id): Path<String>,
) -> (StatusCode, Json<Value>) {
    match engine.stop(&instance_id).await {
        Ok(StopOutcome::AlreadyStopped { status }) => {
            let response = ApiResponse::success_with_message(
                format!(
                    "Instance {} is already stopped (status: {})",
                    instance_id, status
                ),
                serde_json::Value::Null,
            );
            (StatusCode::OK, Json(json!(response)))
        }
        Ok(StopOutcome::Stopped { previous_status }) => {
            let response = ApiResponse::success_with_message(
                format!(
                    "Instance {} stopped successfully (was: {})",
                    instance_id, previous_status
                ),
                serde_json::Value::Null,
            );
            (StatusCode::OK, Json(json!(response)))
        }
        Err(e) => execution_error_response_with(&e, json!({ "instanceId": instance_id })),
    }
}

/// Pause a running workflow instance
///
/// Sends a pause signal to the instance. The instance will checkpoint its state
/// and suspend execution until resumed.
#[utoipa::path(
    post,
    path = "/api/runtime/workflows/instances/{instance_id}/pause",
    params(
        ("instance_id" = String, Path, description = "Instance UUID to pause")
    ),
    responses(
        (status = 200, description = "Instance paused successfully", body = Value),
        (status = 400, description = "Invalid instance ID or instance not pausable", body = ErrorResponse),
        (status = 404, description = "Instance not found", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    ),
    tag = "workflow-controller"
)]
#[instrument(skip(engine), fields(instance_id = %instance_id))]
pub async fn pause_instance_handler(
    crate::middleware::tenant_auth::OrgId(_tenant_id): crate::middleware::tenant_auth::OrgId,
    State(engine): State<Arc<ExecutionEngine>>,
    Path(instance_id): Path<String>,
) -> (StatusCode, Json<Value>) {
    match engine.pause(&instance_id).await {
        Ok(PauseOutcome::AlreadyPaused) => {
            let response = ApiResponse::success_with_message(
                format!("Instance {} is already paused", instance_id),
                serde_json::Value::Null,
            );
            (StatusCode::OK, Json(json!(response)))
        }
        Ok(PauseOutcome::Paused { previous_status }) => {
            let response = ApiResponse::success_with_message(
                format!(
                    "Instance {} paused successfully (was: {})",
                    instance_id, previous_status
                ),
                serde_json::Value::Null,
            );
            (StatusCode::OK, Json(json!(response)))
        }
        Ok(PauseOutcome::NotPausable { status }) => {
            let error_response = json!({
                "success": false,
                "error": "Instance not pausable",
                "message": format!("Instance is in '{}' state and cannot be paused. Only running instances can be paused.", status),
                "instanceId": instance_id,
                "currentStatus": status
            });
            (StatusCode::BAD_REQUEST, Json(error_response))
        }
        Err(e) => execution_error_response_with(&e, json!({ "instanceId": instance_id })),
    }
}

/// Resume a paused workflow instance
///
/// Sends a resume signal to the instance. The instance will resume execution
/// from its last checkpoint.
#[utoipa::path(
    post,
    path = "/api/runtime/workflows/instances/{instance_id}/resume",
    params(
        ("instance_id" = String, Path, description = "Instance UUID to resume")
    ),
    responses(
        (status = 200, description = "Instance resumed successfully", body = Value),
        (status = 400, description = "Invalid instance ID or instance not resumable", body = ErrorResponse),
        (status = 404, description = "Instance not found", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    ),
    tag = "workflow-controller"
)]
#[instrument(skip(engine), fields(instance_id = %instance_id))]
pub async fn resume_instance_handler(
    crate::middleware::tenant_auth::OrgId(_tenant_id): crate::middleware::tenant_auth::OrgId,
    State(engine): State<Arc<ExecutionEngine>>,
    Path(instance_id): Path<String>,
) -> (StatusCode, Json<Value>) {
    match engine.resume(&instance_id).await {
        Ok(ResumeOutcome::AlreadyRunning) => {
            let response = ApiResponse::success_with_message(
                format!("Instance {} is already running", instance_id),
                serde_json::Value::Null,
            );
            (StatusCode::OK, Json(json!(response)))
        }
        Ok(ResumeOutcome::Resumed { previous_status }) => {
            let response = ApiResponse::success_with_message(
                format!(
                    "Instance {} resumed successfully (was: {})",
                    instance_id, previous_status
                ),
                serde_json::Value::Null,
            );
            (StatusCode::OK, Json(json!(response)))
        }
        Ok(ResumeOutcome::NotResumable { status }) => {
            let error_response = json!({
                "success": false,
                "error": "Instance not resumable",
                "message": format!("Instance is in '{}' state and cannot be resumed. Only suspended instances can be resumed.", status),
                "instanceId": instance_id,
                "currentStatus": status
            });
            (StatusCode::BAD_REQUEST, Json(error_response))
        }
        Err(e) => execution_error_response_with(&e, json!({ "instanceId": instance_id })),
    }
}

/// Schedule a workflow execution (placeholder - not implemented)
#[utoipa::path(
    post,
    path = "/api/runtime/workflows/{id}/schedule",
    params(
        ("id" = String, Path, description = "Workflow identifier")
    ),
    request_body = Value,
    responses(
        (status = 501, description = "Not implemented", body = Value)
    ),
    tag = "workflow-controller"
)]
#[instrument(fields(workflow_id = %id))]
pub async fn schedule_workflow_handler(
    crate::middleware::tenant_auth::OrgId(tenant_id): crate::middleware::tenant_auth::OrgId,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> (StatusCode, Json<Value>) {
    // Scheduling requires additional infrastructure:
    // 1. workflow_schedules table
    // 2. Background scheduler service (e.g., tokio-cron-scheduler)
    // 3. Schedule execution logic
    let response = json!({
        "success": false,
        "error": "Not implemented",
        "message": "Workflow scheduling requires additional infrastructure (scheduling service, cron scheduler). This endpoint is a placeholder for future implementation.",
        "endpoint": format!("/api/runtime/{}/workflows/{}/schedule", tenant_id, id),
        "workflowId": id,
        "requestedSchedule": body.get("schedule"),
        "status": 501,
        "suggestion": "Use the execute endpoint to run workflows immediately, or implement a scheduling service externally"
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
        ServiceError::EntitlementDenied(denial) => {
            // Surface AGENT_NOT_ENABLED with the stable `code` that the UI
            // / MCP clients switch on. Body matches what the per-handler
            // agent gates emit, so callers see one shape.
            (StatusCode::FORBIDDEN, Json(denial.json_body()))
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

/// Set the current version for a workflow
///
/// Updates which version is marked as "current" for execution.
/// Note: Requires database migration to add current_version column.
#[utoipa::path(
    post,
    path = "/api/runtime/workflows/{workflow_id}/versions/{version_number}/set-current",
    params(
        ("workflow_id" = String, Path, description = "Workflow identifier"),
        ("version_number" = i32, Path, description = "Version number to set as current")
    ),
    responses(
        (status = 200, description = "Current version updated successfully"),
        (status = 400, description = "Invalid request", body = ErrorResponse),
        (status = 404, description = "Workflow or version not found", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    ),
    tag = "workflow-controller"
)]
#[allow(clippy::too_many_arguments)]
#[instrument(skip(pool, connections, agent_catalog, user_id, role), fields(workflow_id = %workflow_id, version_number = %version_number))]
pub async fn set_current_version_handler(
    crate::middleware::tenant_auth::OrgId(tenant_id): crate::middleware::tenant_auth::OrgId,
    crate::middleware::tenant_auth::Caller { user_id, role }: crate::middleware::tenant_auth::Caller,
    State(pool): State<PgPool>,
    State(connections): State<Arc<ConnectionsFacade>>,
    State(agent_catalog): State<Arc<runtara_dsl::agent_meta::AgentCatalog>>,

    Path((workflow_id, version_number)): Path<(String, i32)>,
) -> (StatusCode, Json<Value>) {
    // Create repository and service
    let repository = Arc::new(WorkflowRepository::new(pool.clone()));

    // Own-scoped authorization: a Member may edit only workflows they created.
    let owner = repository
        .owner(&tenant_id, &workflow_id)
        .await
        .ok()
        .flatten();
    if let Err(denial) = crate::middleware::authorization::require_ownership(
        crate::auth::membership_policy(),
        &tenant_id,
        role,
        crate::authz::Permission::WorkflowUpdate,
        owner.as_deref(),
        &user_id,
    ) {
        return (StatusCode::FORBIDDEN, Json(denial.json_body()));
    }

    let service = WorkflowService::new(repository, connections.clone(), agent_catalog.clone());

    // Delegate to service
    match service
        .set_current_version(&tenant_id, &workflow_id, version_number)
        .await
    {
        Ok(()) => {
            let response = json!({
                "success": true,
                "message": format!("Current version set to {} for workflow '{}'", version_number, workflow_id),
                "workflowId": workflow_id,
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
    path = "/api/runtime/workflows/graph/validate",
    request_body = Value,
    responses(
        (status = 200, description = "Validation completed"),
        (status = 400, description = "Validation failed")
    ),
    tag = "workflow-controller"
)]
#[instrument(skip(pool, catalog, body))]
pub async fn validate_graph_handler(
    crate::middleware::tenant_auth::OrgId(tenant_id): crate::middleware::tenant_auth::OrgId,
    State(pool): State<PgPool>,
    State(catalog): State<std::sync::Arc<runtara_dsl::agent_meta::AgentCatalog>>,
    Json(body): Json<Value>,
) -> (StatusCode, Json<Value>) {
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

    // Try to parse as runtara-dsl Workflow and validate with runtara-workflows
    match serde_json::from_value::<runtara_dsl::Workflow>(json!({
        "executionGraph": body.clone()
    })) {
        Ok(workflow) => {
            // Load the embed closure so validation is recursive — same
            // semantics as the save gate: dangling child references and
            // errors inside embedded (grand)children are real errors.
            let child_infos =
                match crate::compiler::child_workflows::load_child_workflows_for_validation(
                    &pool, &tenant_id, &body,
                )
                .await
                {
                    Ok(infos) => infos,
                    Err(e) => {
                        let error_response = json!({
                            "success": false,
                            "valid": false,
                            "error": "Failed to load child workflows",
                            "message": e
                        });
                        return (StatusCode::INTERNAL_SERVER_ERROR, Json(error_response));
                    }
                };
            let closure_children: Vec<runtara_workflows::ClosureChildGraph> = child_infos
                .into_iter()
                .filter_map(|info| {
                    serde_json::from_value(info.execution_graph).ok().map(|g| {
                        runtara_workflows::ClosureChildGraph {
                            workflow_id: info.workflow_ref.workflow_id,
                            version: info.workflow_ref.version,
                            execution_graph: g,
                        }
                    })
                })
                .collect();

            let root_id = workflow
                .execution_graph
                .name
                .clone()
                .unwrap_or_else(|| "root".to_string());
            let report = runtara_workflows::validate_workflow_closure(
                &root_id,
                &workflow.execution_graph,
                &catalog,
                &closure_children,
            );

            let attribute = |origin: Option<(&str, i32)>, text: String| match origin {
                Some((child_id, _)) => format!("in child workflow '{}': {}", child_id, text),
                None => text,
            };
            let errors: Vec<String> = report
                .errors()
                .map(|(origin, e)| attribute(origin, e.to_string()))
                .collect();
            let warnings: Vec<String> = report
                .warnings()
                .map(|(origin, w)| attribute(origin, w.to_string()))
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
                "message": "Graph validation failed: invalid workflow format",
                "timestamp": chrono::Utc::now().to_rfc3339()
            });
            (StatusCode::OK, Json(response))
        }
    }
}

/// List all supported step types
///
/// Returns registry-backed metadata about available step types.
/// No database or external dependencies - just static DSL metadata.
#[utoipa::path(
    get,
    path = "/api/runtime/steps",
    responses(
        (status = 200, description = "Step types retrieved successfully", body = ListStepTypesResponse),
        (status = 500, description = "Internal server error")
    ),
    tag = "workflow-controller"
)]
#[instrument]
pub async fn list_step_types_handler() -> Result<Json<ListStepTypesResponse>, StatusCode> {
    // Start step is virtual (no struct), add it first
    let mut step_types = vec![StepTypeInfo {
        id: "Start".to_string(),
        name: "Start".to_string(),
        description: "Entry point - receives workflow inputs".to_string(),
        category: "control".to_string(),
    }];

    // Add all registered step types from the static registry.
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
    path = "/api/runtime/workflows/instances/{instance_id}/steps/{step_id}/subinstances",
    params(
        ("instance_id" = String, Path, description = "Instance identifier (UUID)"),
        ("step_id" = String, Path, description = "Step identifier")
    ),
    responses(
        (status = 501, description = "Not implemented", body = Value),
        (status = 404, description = "Instance not found", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    ),
    tag = "workflow-controller"
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

/// Get all dependencies for a workflow
///
/// Returns all child workflows that this workflow depends on (via EmbedWorkflow steps).
/// Can query all versions or a specific version.
#[utoipa::path(
    get,
    path = "/api/runtime/workflows/{id}/dependencies",
    params(
        ("id" = String, Path, description = "Workflow ID"),
        ("version" = Option<i32>, Query, description = "Optional version number (returns all versions if not specified)")
    ),
    responses(
        (status = 200, description = "Dependencies retrieved successfully", body = GetDependenciesResponse),
        (status = 401, description = "Unauthorized"),
        (status = 500, description = "Internal server error")
    ),
    tag = "workflow-controller"
)]
#[instrument(skip(pool))]
pub async fn get_workflow_dependencies_handler(
    crate::middleware::tenant_auth::OrgId(tenant_id): crate::middleware::tenant_auth::OrgId,
    State(pool): State<PgPool>,
    Path(workflow_id): Path<String>,
    Query(params): Query<serde_json::Value>,
) -> Result<Json<GetDependenciesResponse>, (StatusCode, Json<ErrorResponse>)> {
    let version = params
        .get("version")
        .and_then(|v| v.as_i64())
        .map(|v| v as i32);

    let repo = WorkflowRepository::new(pool);
    match repo
        .get_dependencies(&tenant_id, &workflow_id, version)
        .await
    {
        Ok(deps) => {
            let dependencies = deps
                .into_iter()
                .map(
                    |(parent_version, child_id, child_requested, child_resolved, step_id)| {
                        WorkflowDependency {
                            parent_version,
                            child_workflow_id: child_id,
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

/// Get all parent workflows that depend on this workflow
///
/// Returns all parent workflows that reference this workflow in EmbedWorkflow steps.
/// Can query all versions or a specific version.
#[utoipa::path(
    get,
    path = "/api/runtime/workflows/{id}/dependents",
    params(
        ("id" = String, Path, description = "Workflow ID"),
        ("version" = Option<i32>, Query, description = "Optional version number (returns all versions if not specified)")
    ),
    responses(
        (status = 200, description = "Dependents retrieved successfully", body = GetDependentsResponse),
        (status = 401, description = "Unauthorized"),
        (status = 500, description = "Internal server error")
    ),
    tag = "workflow-controller"
)]
#[instrument(skip(pool))]
pub async fn get_workflow_dependents_handler(
    crate::middleware::tenant_auth::OrgId(tenant_id): crate::middleware::tenant_auth::OrgId,
    State(pool): State<PgPool>,
    Path(workflow_id): Path<String>,
    Query(params): Query<serde_json::Value>,
) -> Result<Json<GetDependentsResponse>, (StatusCode, Json<ErrorResponse>)> {
    let version = params
        .get("version")
        .and_then(|v| v.as_i64())
        .map(|v| v as i32);

    let repo = WorkflowRepository::new(pool);
    match repo.get_dependents(&tenant_id, &workflow_id, version).await {
        Ok(deps) => {
            let dependents = deps
                .into_iter()
                .map(
                    |(parent_id, parent_version, child_resolved, step_id)| WorkflowDependent {
                        parent_workflow_id: parent_id,
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

/// Get schemas for a specific workflow version
///
/// Returns the input schema, output schema, and variables from the execution graph
/// of a specific workflow version.
#[utoipa::path(
    get,
    path = "/api/runtime/workflows/{id}/versions/{version}/schemas",
    params(
        ("id" = String, Path, description = "Workflow ID"),
        ("version" = i32, Path, description = "Version number")
    ),
    responses(
        (status = 200, description = "Schemas retrieved successfully", body = VersionSchemasResponse),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Workflow or version not found"),
        (status = 500, description = "Internal server error")
    ),
    tag = "workflow-controller"
)]
#[instrument(skip(pool, connections))]
pub async fn get_version_schemas_handler(
    crate::middleware::tenant_auth::OrgId(tenant_id): crate::middleware::tenant_auth::OrgId,
    State(pool): State<PgPool>,
    State(connections): State<Arc<ConnectionsFacade>>,
    State(agent_catalog): State<Arc<runtara_dsl::agent_meta::AgentCatalog>>,
    Path((workflow_id, version)): Path<(String, i32)>,
) -> Result<Json<VersionSchemasResponse>, (StatusCode, Json<ErrorResponse>)> {
    let repo = Arc::new(WorkflowRepository::new(pool.clone()));
    let service = WorkflowService::new(repo, connections.clone(), agent_catalog.clone());

    match service
        .get_version_schemas(&tenant_id, &workflow_id, version)
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

/// Move a workflow to a different folder.
///
/// Authorization: gated by `workflow:update` (see `permission_for`). For Member that is
/// tenant-wide `Allow` (collaborative editing — workflows are versioned), so no per-resource
/// ownership check is needed here; move is consistently permitted for anyone who may update.
/// If `workflow:update` is ever narrowed back to `Own` for Member, this handler must extract the
/// caller and call `require_ownership` (as `update`/`delete` do) — otherwise it fails open.
#[utoipa::path(
    put,
    path = "/api/runtime/workflows/{id}/move",
    request_body = MoveWorkflowRequest,
    params(
        ("id" = String, Path, description = "Workflow identifier")
    ),
    responses(
        (status = 200, description = "Workflow moved successfully", body = ApiResponse<MoveWorkflowResponse>),
        (status = 400, description = "Invalid path format", body = Value),
        (status = 404, description = "Workflow not found", body = Value),
        (status = 500, description = "Internal server error", body = Value)
    ),
    tag = "workflow-controller"
)]
pub async fn move_workflow_handler(
    crate::middleware::tenant_auth::OrgId(tenant_id): crate::middleware::tenant_auth::OrgId,
    State(pool): State<PgPool>,
    State(connections): State<Arc<ConnectionsFacade>>,
    State(agent_catalog): State<Arc<runtara_dsl::agent_meta::AgentCatalog>>,
    Path(id): Path<String>,
    Json(request): Json<MoveWorkflowRequest>,
) -> (StatusCode, Json<Value>) {
    // Create repository and service
    let repository = Arc::new(WorkflowRepository::new(pool.clone()));
    let service = WorkflowService::new(repository, connections.clone(), agent_catalog.clone());

    match service.move_workflow(&tenant_id, &id, &request.path).await {
        Ok(response) => {
            let api_response =
                ApiResponse::success_with_message("Workflow moved successfully", response);
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
    path = "/api/runtime/workflows/folders",
    responses(
        (status = 200, description = "Folders retrieved successfully", body = FoldersResponse),
        (status = 500, description = "Internal server error", body = Value)
    ),
    tag = "workflow-controller"
)]
pub async fn list_folders_handler(
    crate::middleware::tenant_auth::OrgId(tenant_id): crate::middleware::tenant_auth::OrgId,
    State(pool): State<PgPool>,
    State(connections): State<Arc<ConnectionsFacade>>,
    State(agent_catalog): State<Arc<runtara_dsl::agent_meta::AgentCatalog>>,
) -> (StatusCode, Json<Value>) {
    // Create repository and service
    let repository = Arc::new(WorkflowRepository::new(pool.clone()));
    let service = WorkflowService::new(repository, connections.clone(), agent_catalog.clone());

    match service.list_folders(&tenant_id).await {
        Ok(response) => (
            StatusCode::OK,
            Json(serde_json::to_value(response).unwrap()),
        ),
        Err(e) => map_service_error_to_response(e),
    }
}

/// Rename a folder (updates all workflows with matching path prefix).
///
/// Authorization: gated by `workflow:folder_rename` (see `permission_for`), which is Owner/Admin
/// only. This is a tenant-wide bulk op — it rewrites the `path` of every workflow under the
/// prefix, other members' included — so it is deliberately kept off the Member-`Allow`
/// `workflow:update` permission. The route gate is the whole control; no per-resource ownership
/// check applies (the op spans many resources with different owners by design).
#[utoipa::path(
    put,
    path = "/api/runtime/workflows/folders/rename",
    request_body = RenameFolderRequest,
    responses(
        (status = 200, description = "Folder renamed successfully", body = ApiResponse<RenameFolderResponse>),
        (status = 400, description = "Invalid path format or cannot rename root", body = Value),
        (status = 500, description = "Internal server error", body = Value)
    ),
    tag = "workflow-controller"
)]
pub async fn rename_folder_handler(
    crate::middleware::tenant_auth::OrgId(tenant_id): crate::middleware::tenant_auth::OrgId,
    State(pool): State<PgPool>,
    State(connections): State<Arc<ConnectionsFacade>>,
    State(agent_catalog): State<Arc<runtara_dsl::agent_meta::AgentCatalog>>,
    Json(request): Json<RenameFolderRequest>,
) -> (StatusCode, Json<Value>) {
    // Create repository and service
    let repository = Arc::new(WorkflowRepository::new(pool.clone()));
    let service = WorkflowService::new(repository, connections.clone(), agent_catalog.clone());

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

#[cfg(test)]
mod capability_walk_tests {
    use super::collect_workflow_capabilities;
    use serde_json::json;

    #[test]
    fn collects_distinct_pairs_including_nested_subgraphs() {
        // One top-level Agent, plus an Agent nested inside a Split subgraph.
        let graph = json!({
            "entryPoint": "split1",
            "steps": {
                "split1": {
                    "stepType": "Split",
                    "id": "split1",
                    "subgraph": {
                        "entryPoint": "inner",
                        "steps": {
                            "inner": {
                                "stepType": "Agent",
                                "id": "inner",
                                "agentId": "transform",
                                "capabilityId": "group-by"
                            }
                        }
                    }
                },
                "top": {
                    "stepType": "Agent",
                    "id": "top",
                    "agentId": "http",
                    "capabilityId": "request"
                }
            }
        });

        let pairs = collect_workflow_capabilities(&graph);
        // BTreeSet ordering: sorted by (agent_id, capability_id).
        assert_eq!(
            pairs,
            vec![
                ("http".to_string(), "request".to_string()),
                ("transform".to_string(), "group-by".to_string()),
            ]
        );
    }

    #[test]
    fn deduplicates_repeated_capabilities() {
        let graph = json!({
            "entryPoint": "a",
            "steps": {
                "a": {"stepType": "Agent", "id": "a", "agentId": "http", "capabilityId": "request"},
                "b": {"stepType": "Agent", "id": "b", "agentId": "http", "capabilityId": "request"}
            }
        });

        let pairs = collect_workflow_capabilities(&graph);
        assert_eq!(pairs, vec![("http".to_string(), "request".to_string())]);
    }

    #[test]
    fn ignores_non_agent_steps() {
        let graph = json!({
            "entryPoint": "f",
            "steps": {
                "f": {"stepType": "Finish", "id": "f"}
            }
        });
        assert!(collect_workflow_capabilities(&graph).is_empty());
    }

    #[test]
    fn unparseable_graph_yields_no_pairs() {
        // Missing required `steps`/`entryPoint` — best-effort returns empty, never panics.
        assert!(collect_workflow_capabilities(&json!({"nonsense": true})).is_empty());
    }
}
