use std::sync::Arc;

use axum::{
    Extension,
    extract::{Path, State},
    http::StatusCode,
    response::Json,
};
use serde_json::{Value, json};
use sqlx::PgPool;

use crate::api::dto::reports::*;
use crate::api::repositories::object_model::ObjectStoreManager;
use crate::api::services::reports::{ReportService, ReportServiceError};
use crate::auth::AuthContext;
use crate::runtime_client::RuntimeClient;
use crate::workers::execution_engine::ExecutionEngine;

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
    Json(request): Json<CreateReportRequest>,
) -> Result<(StatusCode, Json<GetReportResponse>), (StatusCode, Json<Value>)> {
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
    Json(request): Json<UpdateReportRequest>,
) -> Result<(StatusCode, Json<GetReportResponse>), (StatusCode, Json<Value>)> {
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
    Json(request): Json<ValidateReportRequest>,
) -> Result<(StatusCode, Json<ValidateReportResponse>), (StatusCode, Json<Value>)> {
    let service = ReportService::new(pool, manager, connections);
    let response = service
        .validate_report(&tenant_id, &request.definition)
        .await;

    Ok((StatusCode::OK, Json(response)))
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
        .unwrap_or(RemoveReportBlockRequest {
            remove_markdown_placeholder: true,
        });

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
