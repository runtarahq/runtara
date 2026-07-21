use std::sync::Arc;
use std::time::Instant;

use axum::{
    Extension,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::Json,
};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use uuid::Uuid;

use crate::api::dto::reports::*;
use crate::api::repositories::object_model::ObjectStoreManager;
use crate::api::services::reports::{ReportService, ReportServiceError};
use crate::auth::AuthContext;
use crate::runtime_client::RuntimeClient;
use crate::workers::execution_engine::ExecutionEngine;

pub async fn get_report_definition_schema() -> (StatusCode, Json<Value>) {
    (
        StatusCode::OK,
        Json(json!({
            "success": true,
            "schema": ReportService::report_definition_json_schema(),
        })),
    )
}

fn parse_report_request<T: DeserializeOwned>(value: Value) -> Result<T, (StatusCode, Json<Value>)> {
    let Some(definition) = value.get("definition") else {
        return Err(error_response(ReportServiceError::Validation(
            "Report request must include definition".to_string(),
        )));
    };
    let syntax_issues = ReportService::validate_report_definition_json_syntax_issues(definition)
        .map_err(error_response)?;
    if let Some(issue) = syntax_issues.into_iter().next() {
        return Err(error_response(ReportServiceError::ValidationIssue(issue)));
    }
    serde_json::from_value(value).map_err(|err| {
        error_response(ReportServiceError::Validation(format!(
            "Report request is invalid: {}",
            err
        )))
    })
}

pub async fn list_reports(
    crate::middleware::tenant_auth::OrgId(tenant_id): crate::middleware::tenant_auth::OrgId,
    State(pool): State<PgPool>,
    State(manager): State<Arc<ObjectStoreManager>>,
    State(connections): State<Arc<runtara_connections::ConnectionsFacade>>,
) -> Result<(StatusCode, Json<ListReportsResponse>), (StatusCode, Json<Value>)> {
    let service = ReportService::new(pool, manager, connections);

    match service.list_reports(&tenant_id).await {
        Ok(reports) => Ok((
            StatusCode::OK,
            Json(ListReportsResponse {
                success: true,
                reports,
            }),
        )),
        Err(error) => Err(error_response(error)),
    }
}

pub async fn get_report(
    crate::middleware::tenant_auth::OrgId(tenant_id): crate::middleware::tenant_auth::OrgId,
    State(pool): State<PgPool>,
    State(manager): State<Arc<ObjectStoreManager>>,
    State(connections): State<Arc<runtara_connections::ConnectionsFacade>>,
    Path(report_id): Path<String>,
) -> Result<(StatusCode, Json<GetReportResponse>), (StatusCode, Json<Value>)> {
    let service = ReportService::new(pool, manager, connections);

    match service.get_report(&tenant_id, &report_id).await {
        Ok(report) => Ok((
            StatusCode::OK,
            Json(GetReportResponse {
                success: true,
                report,
            }),
        )),
        Err(error) => Err(error_response(error)),
    }
}

pub async fn create_report(
    crate::middleware::tenant_auth::OrgId(tenant_id): crate::middleware::tenant_auth::OrgId,
    crate::middleware::tenant_auth::CallerId(user_id): crate::middleware::tenant_auth::CallerId,
    State(pool): State<PgPool>,
    State(manager): State<Arc<ObjectStoreManager>>,
    State(connections): State<Arc<runtara_connections::ConnectionsFacade>>,
    Json(request): Json<Value>,
) -> Result<(StatusCode, Json<GetReportResponse>), (StatusCode, Json<Value>)> {
    let request = parse_report_request::<CreateReportRequest>(request)?;
    let service = ReportService::new(pool, manager, connections);

    match service.create_report(&tenant_id, request, &user_id).await {
        Ok(report) => Ok((
            StatusCode::CREATED,
            Json(GetReportResponse {
                success: true,
                report,
            }),
        )),
        Err(error) => Err(error_response(error)),
    }
}

pub async fn update_report(
    crate::middleware::tenant_auth::OrgId(tenant_id): crate::middleware::tenant_auth::OrgId,
    crate::middleware::tenant_auth::Caller { user_id, role }: crate::middleware::tenant_auth::Caller,
    State(pool): State<PgPool>,
    State(manager): State<Arc<ObjectStoreManager>>,
    State(connections): State<Arc<runtara_connections::ConnectionsFacade>>,
    Path(report_id): Path<String>,
    Json(request): Json<Value>,
) -> Result<(StatusCode, Json<GetReportResponse>), (StatusCode, Json<Value>)> {
    let request = parse_report_request::<UpdateReportRequest>(request)?;
    let service = ReportService::new(pool, manager, connections);

    // Own-scoped authorization: a Member may update only reports they created.
    let owner = service.owner(&tenant_id, &report_id).await;
    if let Err(denial) = crate::middleware::authorization::require_ownership(
        crate::auth::membership_policy(),
        &tenant_id,
        role,
        crate::authz::Permission::ReportUpdate,
        owner.as_deref(),
        &user_id,
    ) {
        return Err((StatusCode::FORBIDDEN, Json(denial.json_body())));
    }

    match service.update_report(&tenant_id, &report_id, request).await {
        Ok(report) => Ok((
            StatusCode::OK,
            Json(GetReportResponse {
                success: true,
                report,
            }),
        )),
        Err(error) => Err(error_response(error)),
    }
}

pub async fn delete_report(
    crate::middleware::tenant_auth::OrgId(tenant_id): crate::middleware::tenant_auth::OrgId,
    crate::middleware::tenant_auth::Caller { user_id, role }: crate::middleware::tenant_auth::Caller,
    State(pool): State<PgPool>,
    State(manager): State<Arc<ObjectStoreManager>>,
    State(connections): State<Arc<runtara_connections::ConnectionsFacade>>,
    Path(report_id): Path<String>,
) -> Result<(StatusCode, Json<DeleteReportResponse>), (StatusCode, Json<Value>)> {
    let service = ReportService::new(pool, manager, connections);

    // Own-scoped authorization: a Member may delete only reports they created.
    let owner = service.owner(&tenant_id, &report_id).await;
    if let Err(denial) = crate::middleware::authorization::require_ownership(
        crate::auth::membership_policy(),
        &tenant_id,
        role,
        crate::authz::Permission::ReportDelete,
        owner.as_deref(),
        &user_id,
    ) {
        return Err((StatusCode::FORBIDDEN, Json(denial.json_body())));
    }

    match service.delete_report(&tenant_id, &report_id).await {
        Ok(()) => Ok((
            StatusCode::OK,
            Json(DeleteReportResponse {
                success: true,
                message: "Report deleted".to_string(),
            }),
        )),
        Err(error) => Err(error_response(error)),
    }
}

pub async fn validate_report(
    crate::middleware::tenant_auth::OrgId(tenant_id): crate::middleware::tenant_auth::OrgId,
    State(pool): State<PgPool>,
    State(manager): State<Arc<ObjectStoreManager>>,
    State(connections): State<Arc<runtara_connections::ConnectionsFacade>>,
    Json(request): Json<Value>,
) -> Result<(StatusCode, Json<ValidateReportResponse>), (StatusCode, Json<Value>)> {
    let Some(definition) = request.get("definition") else {
        return Ok((
            StatusCode::OK,
            Json(ValidateReportResponse {
                valid: false,
                errors: vec![ReportValidationIssue {
                    path: "$.definition".to_string(),
                    code: "MISSING_REPORT_DEFINITION".to_string(),
                    message: "Report request must include definition".to_string(),
                    hint: Some("Pass {\"definition\": <ReportDefinition>}.".to_string()),
                }],
                warnings: vec![],
            }),
        ));
    };
    let syntax_issues = ReportService::validate_report_definition_json_syntax_issues(definition)
        .map_err(error_response)?;
    if !syntax_issues.is_empty() {
        return Ok((
            StatusCode::OK,
            Json(ValidateReportResponse {
                valid: false,
                errors: syntax_issues,
                warnings: vec![],
            }),
        ));
    }
    let request = serde_json::from_value::<ValidateReportRequest>(request).map_err(|err| {
        error_response(ReportServiceError::Validation(format!(
            "Report request is invalid: {}",
            err
        )))
    })?;
    let service = ReportService::new(pool, manager, connections);
    let response = service
        .validate_report(&tenant_id, &request.definition)
        .await;

    Ok((StatusCode::OK, Json(response)))
}

pub async fn preview_report(
    crate::middleware::tenant_auth::OrgId(tenant_id): crate::middleware::tenant_auth::OrgId,
    State(pool): State<PgPool>,
    State(manager): State<Arc<ObjectStoreManager>>,
    State(connections): State<Arc<runtara_connections::ConnectionsFacade>>,
    Json(request): Json<Value>,
) -> Result<(StatusCode, Json<ReportRenderResponse>), (StatusCode, Json<Value>)> {
    let Some(definition) = request.get("definition") else {
        return Err(error_response(ReportServiceError::Validation(
            "Report preview request must include definition".to_string(),
        )));
    };
    let syntax_issues = ReportService::validate_report_definition_json_syntax_issues(definition)
        .map_err(error_response)?;
    if let Some(issue) = syntax_issues.into_iter().next() {
        return Err(error_response(ReportServiceError::ValidationIssue(issue)));
    }
    let request = serde_json::from_value::<ReportPreviewRequest>(request).map_err(|err| {
        error_response(ReportServiceError::Validation(format!(
            "Report preview request is invalid: {}",
            err
        )))
    })?;
    let service = ReportService::new(pool, manager, connections);

    match service.preview_report(&tenant_id, request).await {
        Ok(response) => Ok((StatusCode::OK, Json(response))),
        Err(error) => Err(error_response(error)),
    }
}

#[allow(clippy::too_many_arguments)]
pub async fn render_report(
    crate::middleware::tenant_auth::OrgId(tenant_id): crate::middleware::tenant_auth::OrgId,
    State(pool): State<PgPool>,
    State(manager): State<Arc<ObjectStoreManager>>,
    State(connections): State<Arc<runtara_connections::ConnectionsFacade>>,
    State(engine): State<Arc<ExecutionEngine>>,
    State(runtime_client): State<Option<Arc<RuntimeClient>>>,
    Path(report_id): Path<String>,
    Json(request): Json<ReportRenderRequest>,
) -> Result<(StatusCode, Json<ReportRenderResponse>), (StatusCode, Json<Value>)> {
    let service =
        ReportService::new(pool, manager, connections).with_runtime(engine, runtime_client);

    match service.render_report(&tenant_id, &report_id, request).await {
        Ok(response) => Ok((StatusCode::OK, Json(response))),
        Err(error) => Err(error_response(error)),
    }
}

#[utoipa::path(
    post,
    path = "/api/runtime/reports/{report_id}/blocks/{block_id}/workflow-actions/{action_id}/execute",
    params(
        ("report_id" = String, Path, description = "Report identifier or slug"),
        ("block_id" = String, Path, description = "Origin report block"),
        ("action_id" = String, Path, description = "Stable workflow action identity")
    ),
    request_body = ExecuteReportWorkflowActionRequest,
    responses(
        (status = 200, description = "Workflow completed within the observation window", body = ExecuteReportWorkflowActionResponse),
        (status = 202, description = "Workflow remains queued or running", body = ExecuteReportWorkflowActionResponse),
        (status = 400, description = "Invalid report action request"),
        (status = 403, description = "Insufficient report or workflow permissions"),
        (status = 409, description = "Action is stale, inaccessible, hidden, disabled, or conflicts with the idempotency key")
    ),
    tag = "reports-controller"
)]
#[allow(clippy::too_many_arguments)]
pub async fn execute_report_workflow_action(
    crate::middleware::tenant_auth::OrgId(tenant_id): crate::middleware::tenant_auth::OrgId,
    Extension(auth_context): Extension<AuthContext>,
    State(pool): State<PgPool>,
    State(manager): State<Arc<ObjectStoreManager>>,
    State(connections): State<Arc<runtara_connections::ConnectionsFacade>>,
    State(engine): State<Arc<ExecutionEngine>>,
    State(runtime_client): State<Option<Arc<RuntimeClient>>>,
    State(valkey): State<Option<redis::aio::ConnectionManager>>,
    Path((report_id, block_id, action_id)): Path<(String, String, String)>,
    headers: HeaderMap,
    Json(request): Json<ExecuteReportWorkflowActionRequest>,
) -> Result<(StatusCode, Json<ExecuteReportWorkflowActionResponse>), (StatusCode, Json<Value>)> {
    if let Err(denial) = crate::middleware::authorization::require_permission(
        crate::auth::membership_policy(),
        auth_context.role,
        crate::authz::Permission::ReportRead,
    ) {
        return Err((StatusCode::FORBIDDEN, Json(denial.json_body())));
    }
    let idempotency_key = headers
        .get("Idempotency-Key")
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty() && value.len() <= 200)
        .ok_or_else(|| {
            error_response(ReportServiceError::Validation(
                "Idempotency-Key header is required and must be at most 200 characters".to_string(),
            ))
        })?;
    let identity = format!("{tenant_id}:{report_id}:{block_id}:{action_id}:{idempotency_key}");
    let instance_id = Uuid::new_v5(&Uuid::NAMESPACE_URL, identity.as_bytes());
    let idempotent_replay = reserve_report_action_idempotency(
        valkey,
        &identity,
        report_action_request_fingerprint(&request),
    )
    .await?;

    let service =
        ReportService::new(pool, manager, connections).with_runtime(engine, runtime_client);
    let started_at = Instant::now();
    match service
        .execute_report_workflow_action(
            &tenant_id,
            &report_id,
            &block_id,
            &action_id,
            request,
            instance_id,
            idempotent_replay,
        )
        .await
    {
        Ok(response) => {
            let status = if response.completed_within_wait {
                StatusCode::OK
            } else {
                StatusCode::ACCEPTED
            };
            tracing::info!(
                tenant_id,
                report_id,
                block_id,
                action_id,
                instance_id = %response.execution.instance_id,
                workflow_id = %response.execution.workflow_id,
                workflow_status = %response.execution.status,
                completed_within_wait = response.completed_within_wait,
                refresh_required = response.refresh_required,
                idempotent_replay,
                endpoint_duration_ms = started_at.elapsed().as_millis() as u64,
                workflow_duration_ms = response.execution.duration_ms,
                "Executed report workflow action"
            );
            Ok((status, Json(response)))
        }
        Err(error) => {
            tracing::warn!(
                tenant_id,
                report_id,
                block_id,
                action_id,
                %instance_id,
                idempotent_replay,
                endpoint_duration_ms = started_at.elapsed().as_millis() as u64,
                error = %error,
                "Report workflow action failed"
            );
            Err(error_response(error))
        }
    }
}

fn report_action_request_fingerprint(request: &ExecuteReportWorkflowActionRequest) -> String {
    let bytes = serde_json::to_vec(&json!({
        "trigger": &request.trigger,
        "render": &request.render,
    }))
    .unwrap_or_default();
    hex::encode(Sha256::digest(bytes))
}

async fn reserve_report_action_idempotency(
    valkey: Option<redis::aio::ConnectionManager>,
    identity: &str,
    fingerprint: String,
) -> Result<bool, (StatusCode, Json<Value>)> {
    let Some(mut connection) = valkey else {
        // Queueing itself will report a runtime configuration error when the
        // trigger stream is unavailable. Deterministic instance IDs still
        // protect deployments whose trigger stream is configured separately.
        return Ok(false);
    };
    let key = format!(
        "report_workflow_action:{}",
        hex::encode(Sha256::digest(identity.as_bytes()))
    );
    let inserted: Option<String> = redis::cmd("SET")
        .arg(&key)
        .arg(&fingerprint)
        .arg("NX")
        .arg("EX")
        .arg(600)
        .query_async(&mut connection)
        .await
        .map_err(|error| {
            error_response(ReportServiceError::Database(format!(
                "Failed to reserve report workflow action idempotency key: {error}"
            )))
        })?;
    if inserted.is_some() {
        return Ok(false);
    }
    let existing: Option<String> = redis::cmd("GET")
        .arg(&key)
        .query_async(&mut connection)
        .await
        .map_err(|error| {
            error_response(ReportServiceError::Database(format!(
                "Failed to read report workflow action idempotency key: {error}"
            )))
        })?;
    if existing
        .as_deref()
        .is_some_and(|value| value != fingerprint)
    {
        return Err(error_response(ReportServiceError::Conflict(
            "Idempotency-Key was already used with a different report action payload".to_string(),
        )));
    }
    Ok(true)
}

#[allow(clippy::too_many_arguments)]
pub async fn get_report_block_data(
    crate::middleware::tenant_auth::OrgId(tenant_id): crate::middleware::tenant_auth::OrgId,
    State(pool): State<PgPool>,
    State(manager): State<Arc<ObjectStoreManager>>,
    State(connections): State<Arc<runtara_connections::ConnectionsFacade>>,
    State(engine): State<Arc<ExecutionEngine>>,
    State(runtime_client): State<Option<Arc<RuntimeClient>>>,
    Path((report_id, block_id)): Path<(String, String)>,
    Json(request): Json<ReportBlockOnlyDataRequest>,
) -> Result<(StatusCode, Json<ReportBlockRenderResult>), (StatusCode, Json<Value>)> {
    let service =
        ReportService::new(pool, manager, connections).with_runtime(engine, runtime_client);

    match service
        .render_report_block(&tenant_id, &report_id, &block_id, request)
        .await
    {
        Ok(response) => Ok((StatusCode::OK, Json(response))),
        Err(error) => Err(error_response(error)),
    }
}

#[allow(clippy::too_many_arguments)]
pub async fn submit_report_workflow_action(
    crate::middleware::tenant_auth::OrgId(tenant_id): crate::middleware::tenant_auth::OrgId,
    Extension(auth_context): Extension<AuthContext>,
    State(pool): State<PgPool>,
    State(manager): State<Arc<ObjectStoreManager>>,
    State(connections): State<Arc<runtara_connections::ConnectionsFacade>>,
    State(engine): State<Arc<ExecutionEngine>>,
    State(runtime_client): State<Option<Arc<RuntimeClient>>>,
    Path((report_id, block_id, action_id)): Path<(String, String, String)>,
    Json(request): Json<SubmitReportWorkflowActionRequest>,
) -> Result<(StatusCode, Json<Value>), (StatusCode, Json<Value>)> {
    let service =
        ReportService::new(pool, manager, connections).with_runtime(engine, runtime_client);

    match service
        .submit_report_workflow_action(
            &tenant_id,
            &report_id,
            &block_id,
            &action_id,
            request,
            &auth_context,
        )
        .await
    {
        Ok(response) => Ok((StatusCode::ACCEPTED, Json(response))),
        Err(error) => Err(error_response(error)),
    }
}

pub async fn get_report_filter_options(
    crate::middleware::tenant_auth::OrgId(tenant_id): crate::middleware::tenant_auth::OrgId,
    State(pool): State<PgPool>,
    State(manager): State<Arc<ObjectStoreManager>>,
    State(connections): State<Arc<runtara_connections::ConnectionsFacade>>,
    Path((report_id, filter_id)): Path<(String, String)>,
    Json(request): Json<ReportFilterOptionsRequest>,
) -> Result<(StatusCode, Json<ReportFilterOptionsResponse>), (StatusCode, Json<Value>)> {
    let service = ReportService::new(pool, manager, connections);

    match service
        .get_filter_options(&tenant_id, &report_id, &filter_id, request)
        .await
    {
        Ok(response) => Ok((StatusCode::OK, Json(response))),
        Err(error) => Err(error_response(error)),
    }
}

pub async fn get_report_lookup_options(
    crate::middleware::tenant_auth::OrgId(tenant_id): crate::middleware::tenant_auth::OrgId,
    State(pool): State<PgPool>,
    State(manager): State<Arc<ObjectStoreManager>>,
    State(connections): State<Arc<runtara_connections::ConnectionsFacade>>,
    Path((report_id, block_id, field)): Path<(String, String, String)>,
    Json(request): Json<ReportLookupOptionsRequest>,
) -> Result<(StatusCode, Json<ReportLookupOptionsResponse>), (StatusCode, Json<Value>)> {
    let service = ReportService::new(pool, manager, connections);

    match service
        .get_lookup_options(&tenant_id, &report_id, &block_id, &field, request)
        .await
    {
        Ok(response) => Ok((StatusCode::OK, Json(response))),
        Err(error) => Err(error_response(error)),
    }
}

pub async fn query_report_dataset(
    crate::middleware::tenant_auth::OrgId(tenant_id): crate::middleware::tenant_auth::OrgId,
    State(pool): State<PgPool>,
    State(manager): State<Arc<ObjectStoreManager>>,
    State(connections): State<Arc<runtara_connections::ConnectionsFacade>>,
    Path((report_id, dataset_id)): Path<(String, String)>,
    Json(request): Json<ReportDatasetQueryRequest>,
) -> Result<(StatusCode, Json<ReportDatasetQueryResponse>), (StatusCode, Json<Value>)> {
    let service = ReportService::new(pool, manager, connections);

    match service
        .query_dataset(&tenant_id, &report_id, &dataset_id, request)
        .await
    {
        Ok(response) => Ok((StatusCode::OK, Json(response))),
        Err(error) => Err(error_response(error)),
    }
}

#[derive(Debug, serde::Deserialize, utoipa::ToSchema)]
pub struct EditReportRequest {
    /// Atomic batch of edit operations applied in order; if any op
    /// fails the entire batch is rolled back.
    #[serde(default)]
    pub ops: Vec<runtara_report_dsl::edit_ops::ReportEditOp>,
}

#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
pub struct EditReportResponse {
    pub success: bool,
    pub report: ReportDto,
}

/// Phase 6 canonical edit endpoint. Accepts a batch of `ReportEditOp`s
/// and applies them atomically. The legacy per-op REST + MCP handlers
/// have all been deleted (Phase 8) so this is the only mutation entry
/// point for layout + block changes.
#[utoipa::path(
    post,
    path = "/api/runtime/reports/{report_id}/edit",
    tag = "reports-controller",
    params(
        ("report_id" = String, Path, description = "Report id or slug"),
    ),
    request_body = EditReportRequest,
    responses(
        (status = 200, description = "Batch applied", body = EditReportResponse),
        (status = 400, description = "Validation error"),
        (status = 404, description = "Report not found"),
    )
)]
pub async fn edit_report(
    crate::middleware::tenant_auth::OrgId(tenant_id): crate::middleware::tenant_auth::OrgId,
    State(pool): State<PgPool>,
    State(manager): State<Arc<ObjectStoreManager>>,
    State(connections): State<Arc<runtara_connections::ConnectionsFacade>>,
    Path(report_id): Path<String>,
    Json(raw): Json<Value>,
) -> Result<(StatusCode, Json<EditReportResponse>), (StatusCode, Json<Value>)> {
    // Take the raw ops array and run the strict pre-parse BEFORE serde
    // deserializes into the typed `ReportEditOp` enum. The enum is internally
    // tagged and cannot carry `#[serde(deny_unknown_fields)]`, so without this
    // pass a misplaced/misspelled top-level field (e.g. `parentNodeId` or
    // `beforeNodeId` that belongs under `target`) would be silently dropped and
    // the op would no-op or land in the wrong place with a 200. The request
    // body schema stays typed via `request_body = EditReportRequest`.
    let bad_request = |message: String, code: Option<&str>| {
        let mut body = json!({ "success": false, "message": message });
        if let Some(code) = code {
            body["code"] = Value::String(code.to_string());
        }
        (StatusCode::BAD_REQUEST, Json(body))
    };
    let raw_ops = match raw.get("ops") {
        Some(Value::Array(ops)) => ops.clone(),
        None | Some(Value::Null) => Vec::new(),
        Some(_) => {
            return Err(bad_request(
                "`ops` must be an array of edit operations".to_string(),
                None,
            ));
        }
    };
    if let Err(err) = runtara_report_dsl::edit_ops::validate_edit_ops_json(&raw_ops) {
        return Err(bad_request(err.message, Some(err.code)));
    }
    let mut ops = Vec::with_capacity(raw_ops.len());
    for (index, raw_op) in raw_ops.into_iter().enumerate() {
        match serde_json::from_value::<runtara_report_dsl::edit_ops::ReportEditOp>(raw_op) {
            Ok(op) => ops.push(op),
            Err(err) => {
                return Err(bad_request(format!("op {index}: {err}"), None));
            }
        }
    }

    let service = ReportService::new(pool, manager, connections);
    match service.edit_report(&tenant_id, &report_id, &ops).await {
        Ok(report) => Ok((
            StatusCode::OK,
            Json(EditReportResponse {
                success: true,
                report,
            }),
        )),
        Err(error) => Err(error_response(error)),
    }
}

fn error_response(error: ReportServiceError) -> (StatusCode, Json<Value>) {
    match error {
        ReportServiceError::NotFound => (
            StatusCode::NOT_FOUND,
            Json(json!({ "success": false, "message": "Report not found" })),
        ),
        ReportServiceError::Validation(message) => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "success": false, "message": message })),
        ),
        ReportServiceError::ValidationIssue(issue) => (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "success": false,
                "message": issue.message.clone(),
                "issue": issue,
            })),
        ),
        ReportServiceError::Conflict(message) => (
            StatusCode::CONFLICT,
            Json(json!({ "success": false, "message": message })),
        ),
        ReportServiceError::Database(message) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "success": false, "message": message })),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn execute_request(wait_ms: u64, stage: &str) -> ExecuteReportWorkflowActionRequest {
        ExecuteReportWorkflowActionRequest {
            trigger: ExecuteReportWorkflowActionTrigger {
                row: json!({"id": "case-123", "stage": stage}),
                value: Some(json!(stage)),
                field: Some("stage".to_string()),
                selected_rows: vec![],
            },
            render: ReportRenderRequest {
                filters: HashMap::new(),
                view_id: Some(stage.to_string()),
                blocks: None,
                timezone: Some("Europe/Warsaw".to_string()),
            },
            wait_ms: Some(wait_ms),
        }
    }

    #[test]
    fn report_action_fingerprint_ignores_observation_window() {
        assert_eq!(
            report_action_request_fingerprint(&execute_request(100, "intake")),
            report_action_request_fingerprint(&execute_request(5_000, "intake"))
        );
    }

    #[test]
    fn report_action_fingerprint_changes_with_semantic_payload() {
        assert_ne!(
            report_action_request_fingerprint(&execute_request(2_000, "intake")),
            report_action_request_fingerprint(&execute_request(2_000, "review"))
        );
    }
}
