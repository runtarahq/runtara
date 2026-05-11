use std::sync::Arc;

use axum::{
    Extension,
    extract::{Path, State},
    http::StatusCode,
    response::Json,
};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use sqlx::PgPool;

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
    State(pool): State<PgPool>,
    State(manager): State<Arc<ObjectStoreManager>>,
    State(connections): State<Arc<runtara_connections::ConnectionsFacade>>,
    Json(request): Json<Value>,
) -> Result<(StatusCode, Json<GetReportResponse>), (StatusCode, Json<Value>)> {
    let request = parse_report_request::<CreateReportRequest>(request)?;
    let service = ReportService::new(pool, manager, connections);

    match service.create_report(&tenant_id, request).await {
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
    State(pool): State<PgPool>,
    State(manager): State<Arc<ObjectStoreManager>>,
    State(connections): State<Arc<runtara_connections::ConnectionsFacade>>,
    Path(report_id): Path<String>,
    Json(request): Json<Value>,
) -> Result<(StatusCode, Json<GetReportResponse>), (StatusCode, Json<Value>)> {
    let request = parse_report_request::<UpdateReportRequest>(request)?;
    let service = ReportService::new(pool, manager, connections);

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
    State(pool): State<PgPool>,
    State(manager): State<Arc<ObjectStoreManager>>,
    State(connections): State<Arc<runtara_connections::ConnectionsFacade>>,
    Path(report_id): Path<String>,
) -> Result<(StatusCode, Json<DeleteReportResponse>), (StatusCode, Json<Value>)> {
    let service = ReportService::new(pool, manager, connections);

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

pub async fn add_report_block(
    crate::middleware::tenant_auth::OrgId(tenant_id): crate::middleware::tenant_auth::OrgId,
    State(pool): State<PgPool>,
    State(manager): State<Arc<ObjectStoreManager>>,
    State(connections): State<Arc<runtara_connections::ConnectionsFacade>>,
    Path(report_id): Path<String>,
    Json(request): Json<AddReportBlockRequest>,
) -> Result<(StatusCode, Json<ReportBlockMutationResponse>), (StatusCode, Json<Value>)> {
    let service = ReportService::new(pool, manager, connections);

    match service
        .add_report_block(&tenant_id, &report_id, request)
        .await
    {
        Ok(response) => Ok((StatusCode::OK, Json(response))),
        Err(error) => Err(error_response(error)),
    }
}

pub async fn replace_report_block(
    crate::middleware::tenant_auth::OrgId(tenant_id): crate::middleware::tenant_auth::OrgId,
    State(pool): State<PgPool>,
    State(manager): State<Arc<ObjectStoreManager>>,
    State(connections): State<Arc<runtara_connections::ConnectionsFacade>>,
    Path((report_id, block_id)): Path<(String, String)>,
    Json(request): Json<ReplaceReportBlockRequest>,
) -> Result<(StatusCode, Json<ReportBlockMutationResponse>), (StatusCode, Json<Value>)> {
    let service = ReportService::new(pool, manager, connections);

    match service
        .replace_report_block(&tenant_id, &report_id, &block_id, request)
        .await
    {
        Ok(response) => Ok((StatusCode::OK, Json(response))),
        Err(error) => Err(error_response(error)),
    }
}

pub async fn patch_report_block(
    crate::middleware::tenant_auth::OrgId(tenant_id): crate::middleware::tenant_auth::OrgId,
    State(pool): State<PgPool>,
    State(manager): State<Arc<ObjectStoreManager>>,
    State(connections): State<Arc<runtara_connections::ConnectionsFacade>>,
    Path((report_id, block_id)): Path<(String, String)>,
    Json(request): Json<PatchReportBlockRequest>,
) -> Result<(StatusCode, Json<ReportBlockMutationResponse>), (StatusCode, Json<Value>)> {
    let service = ReportService::new(pool, manager, connections);

    match service
        .patch_report_block(&tenant_id, &report_id, &block_id, request)
        .await
    {
        Ok(response) => Ok((StatusCode::OK, Json(response))),
        Err(error) => Err(error_response(error)),
    }
}

pub async fn move_report_block(
    crate::middleware::tenant_auth::OrgId(tenant_id): crate::middleware::tenant_auth::OrgId,
    State(pool): State<PgPool>,
    State(manager): State<Arc<ObjectStoreManager>>,
    State(connections): State<Arc<runtara_connections::ConnectionsFacade>>,
    Path((report_id, block_id)): Path<(String, String)>,
    Json(request): Json<MoveReportBlockRequest>,
) -> Result<(StatusCode, Json<ReportBlockMutationResponse>), (StatusCode, Json<Value>)> {
    let service = ReportService::new(pool, manager, connections);

    match service
        .move_report_block(&tenant_id, &report_id, &block_id, request)
        .await
    {
        Ok(response) => Ok((StatusCode::OK, Json(response))),
        Err(error) => Err(error_response(error)),
    }
}

pub async fn remove_report_block(
    crate::middleware::tenant_auth::OrgId(tenant_id): crate::middleware::tenant_auth::OrgId,
    State(pool): State<PgPool>,
    State(manager): State<Arc<ObjectStoreManager>>,
    State(connections): State<Arc<runtara_connections::ConnectionsFacade>>,
    Path((report_id, block_id)): Path<(String, String)>,
    body: Option<Json<RemoveReportBlockRequest>>,
) -> Result<(StatusCode, Json<ReportBlockMutationResponse>), (StatusCode, Json<Value>)> {
    let service = ReportService::new(pool, manager, connections);
    let request = body
        .map(|Json(request)| request)
        .unwrap_or(RemoveReportBlockRequest {});

    match service
        .remove_report_block(&tenant_id, &report_id, &block_id, request)
        .await
    {
        Ok(response) => Ok((StatusCode::OK, Json(response))),
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
