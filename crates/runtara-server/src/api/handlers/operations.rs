use crate::{
    api::{
        dto::operations::*, handlers::executions::parse_query,
        repositories::operations::OperationsRepository, services::operations::discover_queues,
    },
    middleware::tenant_auth::OrgId,
    runtime_client::RuntimeClient,
    workers::execution_engine::ExecutionEngine,
};
use axum::{Json, extract::State, http::StatusCode};
use serde_json::{Value, json};
use sqlx::PgPool;
use std::sync::Arc;

fn error(status: StatusCode, message: impl ToString) -> (StatusCode, Json<Value>) {
    (
        status,
        Json(json!({"success":false,"message":message.to_string()})),
    )
}

#[utoipa::path(get, path = "/api/runtime/operations/queues", responses((status=200, body=crate::api::dto::common::ApiResponse<Vec<OperationQueue>>)), tag="operations-controller")]
pub async fn operation_queues(
    OrgId(tenant): OrgId,
    State(pool): State<PgPool>,
    State(client): State<Option<Arc<RuntimeClient>>>,
) -> (StatusCode, Json<Value>) {
    let Some(client) = client else {
        return error(StatusCode::SERVICE_UNAVAILABLE, "Runtime unavailable");
    };
    match discover_queues(&OperationsRepository::new(pool), &client, &tenant).await {
        Ok(queues) => (StatusCode::OK, Json(json!({"success":true,"data":queues}))),
        Err(e) => {
            tracing::error!(error=%e,"Operations queue discovery failed");
            error(StatusCode::INTERNAL_SERVER_ERROR, "Could not load queues")
        }
    }
}

#[utoipa::path(post, path = "/api/runtime/operations/requests/query", request_body=QueryOperationRequests, responses((status=200, body=crate::api::dto::common::ApiResponse<OperationRequestPage>)), tag="operations-controller")]
pub async fn query_operation_requests(
    OrgId(tenant): OrgId,
    State(engine): State<Arc<ExecutionEngine>>,
    State(client): State<Option<Arc<RuntimeClient>>>,
    Json(request): Json<QueryOperationRequests>,
) -> (StatusCode, Json<Value>) {
    let Some(client) = client else {
        return error(StatusCode::SERVICE_UNAVAILABLE, "Runtime unavailable");
    };
    if request.workflow_id.is_empty()
        || request.workflow_id.len() > 256
        || request.workflow_id.contains(':')
        || request.action_key.is_empty()
        || request.action_key.len() > 256
        || request.action_key.chars().any(char::is_control)
    {
        return error(
            StatusCode::BAD_REQUEST,
            "One workflow and an action key are required",
        );
    }
    if request
        .query
        .workflow_id
        .as_ref()
        .is_some_and(|id| id != &request.workflow_id)
    {
        return error(StatusCode::BAD_REQUEST, "A queue must use one workflow");
    }
    let filters = match parse_query(&request.query) {
        Ok(f) => f,
        Err(e) => return error(StatusCode::BAD_REQUEST, e),
    };
    let page = request.query.page.unwrap_or(0).max(0);
    let size = request.query.size.unwrap_or(25).clamp(1, 100);
    let options = match engine
        .execution_listing_options(&tenant, page, size, &filters)
        .await
    {
        Ok(o) => o,
        Err(e) => return error(StatusCode::BAD_REQUEST, e),
    };
    let found = match client
        .operation_requests(
            &tenant,
            &request.workflow_id,
            &request.action_key,
            &options,
            &filters.state_fields,
        )
        .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::error!(error=%e,"Operations request query failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "Could not load requests");
        }
    };
    let content = found
        .rows
        .into_iter()
        .map(|row| {
            let spec: runtara_core::persistence::inputs::InputRequestSpec =
                serde_json::from_str(&row.spec).map_err(|e| e.to_string())?;
            Ok(OperationRequest {
                instance_id: row.instance_id,
                workflow_id: request.workflow_id.clone(),
                used_version: row
                    .image_name
                    .split(':')
                    .nth(1)
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(0),
                run_label: row.run_label,
                request_id: row.request_id,
                action_key: row.action_key,
                requested_at: row.requested_at,
                deadline: row.deadline,
                input_schema: spec.response_schema,
                context: spec
                    .metadata
                    .get("context")
                    .cloned()
                    .unwrap_or_else(|| json!({})),
                label: spec
                    .metadata
                    .get("step_name")
                    .and_then(Value::as_str)
                    .unwrap_or(&spec.signal_id)
                    .into(),
                message: spec
                    .metadata
                    .get("message")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                state: row.state,
            })
        })
        .collect::<Result<Vec<_>, String>>();
    match content {
        Ok(content) => (
            StatusCode::OK,
            Json(
                json!({"success":true,"data":OperationRequestPage {content,total_elements:found.total,total_pages:(found.total+i64::from(size)-1)/i64::from(size),number:page,size}}),
            ),
        ),
        Err(e) => {
            tracing::error!(error=%e,"Invalid stored request spec");
            error(StatusCode::INTERNAL_SERVER_ERROR, "Could not load requests")
        }
    }
}

#[utoipa::path(get,path="/api/runtime/operations/views",responses((status=200,body=crate::api::dto::common::ApiResponse<Vec<SavedOperationView>>)),tag="operations-controller")]
pub async fn list_operation_views(
    OrgId(tenant): OrgId,
    State(pool): State<PgPool>,
) -> (StatusCode, Json<Value>) {
    match OperationsRepository::new(pool).list_views(&tenant).await {
        Ok(views) => (StatusCode::OK, Json(json!({"success":true,"data":views}))),
        Err(e) => {
            tracing::error!(error=%e,"Operations view listing failed");
            error(StatusCode::INTERNAL_SERVER_ERROR, "Could not load views")
        }
    }
}

#[utoipa::path(post,path="/api/runtime/operations/views",request_body=SaveOperationView,responses((status=201,body=crate::api::dto::common::ApiResponse<SavedOperationView>),(status=400),(status=403),(status=404)),tag="operations-controller")]
pub async fn create_operation_view(
    OrgId(tenant): OrgId,
    State(pool): State<PgPool>,
    axum::Extension(auth): axum::Extension<crate::auth::AuthContext>,
    Json(request): Json<SaveOperationView>,
) -> (StatusCode, Json<Value>) {
    save_view(&tenant, pool, &auth, None, request).await
}
#[utoipa::path(put,path="/api/runtime/operations/views/{id}",params(("id"=String,Path)),request_body=SaveOperationView,responses((status=200,body=crate::api::dto::common::ApiResponse<SavedOperationView>),(status=400),(status=403),(status=404),(status=409)),tag="operations-controller")]
pub async fn update_operation_view(
    OrgId(tenant): OrgId,
    State(pool): State<PgPool>,
    axum::Extension(auth): axum::Extension<crate::auth::AuthContext>,
    axum::extract::Path(id): axum::extract::Path<String>,
    Json(request): Json<SaveOperationView>,
) -> (StatusCode, Json<Value>) {
    save_view(&tenant, pool, &auth, Some(&id), request).await
}

async fn authorize_view_workflow(
    tenant: &str,
    pool: &PgPool,
    auth: &crate::auth::AuthContext,
    workflow: &str,
) -> Result<(), (StatusCode, Json<Value>)> {
    if auth.org_id != tenant {
        return Err(error(StatusCode::NOT_FOUND, "Workflow not found"));
    }
    let repository = crate::api::repositories::workflows::WorkflowRepository::new(pool.clone());
    // Test existence separately: older workflows may have no recorded author.
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM workflows WHERE tenant_id=$1 AND workflow_id=$2)",
    )
    .bind(tenant)
    .bind(workflow)
    .fetch_one(pool)
    .await
    .map_err(|_| error(StatusCode::INTERNAL_SERVER_ERROR, "Could not read workflow"))?;
    if !exists {
        return Err(error(StatusCode::NOT_FOUND, "Workflow not found"));
    }
    let owner = repository
        .owner(tenant, workflow)
        .await
        .map_err(|_| error(StatusCode::INTERNAL_SERVER_ERROR, "Could not read workflow"))?;
    crate::middleware::authorization::require_ownership(
        crate::auth::membership_policy(),
        tenant,
        auth.role,
        crate::authz::Permission::WorkflowUpdate,
        owner.as_deref(),
        &auth.user_id,
    )
    .map_err(|denial| (StatusCode::FORBIDDEN, Json(denial.json_body())))
}
async fn save_view(
    tenant: &str,
    pool: PgPool,
    auth: &crate::auth::AuthContext,
    id: Option<&str>,
    request: SaveOperationView,
) -> (StatusCode, Json<Value>) {
    if let Err(e) = crate::api::services::operations::validate_view(&request.configuration) {
        return error(StatusCode::BAD_REQUEST, e);
    }
    let repo = OperationsRepository::new(pool.clone());
    if let Some(id) = id {
        match repo.get_view(tenant, id).await {
            Ok(Some(existing)) => {
                if existing.configuration.workflow != request.configuration.workflow {
                    return error(
                        StatusCode::BAD_REQUEST,
                        "Create a new view to change its workflow",
                    );
                }
                if request.revision.is_none() {
                    return error(
                        StatusCode::BAD_REQUEST,
                        "Editing a view requires its revision",
                    );
                }
            }
            Ok(None) => return error(StatusCode::NOT_FOUND, "View not found"),
            Err(_) => return error(StatusCode::INTERNAL_SERVER_ERROR, "Could not load view"),
        }
    }
    if let Err(response) =
        authorize_view_workflow(tenant, &pool, auth, &request.configuration.workflow).await
    {
        return response;
    }
    match repo.save_view(tenant, &auth.user_id, id, &request).await {
        Ok(Some(view)) => (
            if id.is_some() {
                StatusCode::OK
            } else {
                StatusCode::CREATED
            },
            Json(json!({"success":true,"data":view})),
        ),
        Ok(None) => error(
            StatusCode::CONFLICT,
            "This view changed. Reload it before saving.",
        ),
        Err(e) => {
            tracing::error!(error=%e,"Operations view save failed");
            error(StatusCode::INTERNAL_SERVER_ERROR, "Could not save view")
        }
    }
}

#[derive(serde::Deserialize, utoipa::IntoParams)]
pub struct ViewRevision {
    pub revision: i32,
}
#[utoipa::path(delete,path="/api/runtime/operations/views/{id}",params(("id"=String,Path),ViewRevision),responses((status=200),(status=403),(status=404),(status=409)),tag="operations-controller")]
pub async fn delete_operation_view(
    OrgId(tenant): OrgId,
    State(pool): State<PgPool>,
    axum::Extension(auth): axum::Extension<crate::auth::AuthContext>,
    axum::extract::Path(id): axum::extract::Path<String>,
    axum::extract::Query(revision): axum::extract::Query<ViewRevision>,
) -> (StatusCode, Json<Value>) {
    let repo = OperationsRepository::new(pool.clone());
    let view = match repo.get_view(&tenant, &id).await {
        Ok(Some(view)) => view,
        Ok(None) => return error(StatusCode::NOT_FOUND, "View not found"),
        Err(_) => return error(StatusCode::INTERNAL_SERVER_ERROR, "Could not load view"),
    };
    if let Err(response) =
        authorize_view_workflow(&tenant, &pool, &auth, &view.configuration.workflow).await
    {
        return response;
    }
    match repo.delete_view(&tenant, &id, revision.revision).await {
        Ok(true) => (StatusCode::OK, Json(json!({"success":true}))),
        Ok(false) => error(
            StatusCode::CONFLICT,
            "This view changed. Reload it before deleting.",
        ),
        Err(_) => error(StatusCode::INTERNAL_SERVER_ERROR, "Could not delete view"),
    }
}

#[utoipa::path(get,path="/api/runtime/operations/processes",responses((status=200,body=crate::api::dto::common::ApiResponse<Vec<OperationProcess>>)),tag="operations-controller")]
pub async fn operation_processes(
    OrgId(tenant): OrgId,
    State(pool): State<PgPool>,
) -> (StatusCode, Json<Value>) {
    match OperationsRepository::new(pool)
        .current_graphs(&tenant)
        .await
    {
        Ok(graphs) => {
            let data = graphs
                .into_iter()
                .map(|(id, graph)| OperationProcess {
                    name: graph
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or(&id)
                        .into(),
                    workflow_id: id,
                    state_schema: graph
                        .get("stateSchema")
                        .cloned()
                        .unwrap_or_else(|| json!({})),
                })
                .collect::<Vec<_>>();
            (StatusCode::OK, Json(json!({"success":true,"data":data})))
        }
        Err(_) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Could not load processes",
        ),
    }
}
