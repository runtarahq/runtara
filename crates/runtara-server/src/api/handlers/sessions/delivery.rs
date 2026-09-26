//! Owner-visible outcomes and conditional resolution of retained session messages.
use super::*;
use crate::api::handlers::step_events::workflow_runtime_error_response;
use crate::api::services::workflow_runtime::WorkflowRuntimeError;
use axum::extract::{Query, rejection::QueryRejection};

type Reply = (StatusCode, Json<Value>);

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeliveryQuery {
    #[serde(default = "initial_cursor")]
    pub cursor: String,
    #[serde(default = "page_size")]
    pub limit: u32,
}
fn page_size() -> u32 {
    50
}
fn initial_cursor() -> String {
    "0".into()
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct DeliveryPage {
    pub deliveries: Vec<DeliveryStatus>,
    /// Zero ends the scan. Pages may overlap while deliveries change.
    pub next_cursor: String,
}

#[derive(Deserialize, ToSchema)]
#[serde(tag = "action", rename_all = "camelCase", deny_unknown_fields)]
pub enum ResolveDeliveryRequest {
    Fail,
    Select {
        #[serde(rename = "instanceId")]
        instance_id: String,
        #[serde(rename = "requestId")]
        request_id: String,
    },
}

async fn authorized_scope(
    connection: Option<ConnectionManager>,
    tenant: &str,
    session: &str,
) -> Result<(ConnectionManager, QueueScope, managed::SessionRoute), Reply> {
    let Some(mut conn) = connection else {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({
                "success": false, "code": "QUEUE_UNAVAILABLE", "message": "Queue unavailable", "data": null
            })),
        ));
    };
    let scope = QueueScope::new(tenant, session).map_err(queue_error_response)?;
    let route = managed::session_route(&mut conn, &scope)
        .await
        .map_err(queue_error_response)?;
    Ok((conn, scope, route))
}

#[utoipa::path(get, path = "/api/runtime/sessions/{sessionId}/deliveries",
    params(("sessionId" = String, Path), ("cursor" = Option<String>, Query), ("limit" = Option<u32>, Query)),
    responses((status = 200, body = crate::api::dto::common::ApiResponse<DeliveryPage>),
        (status = 400, description = "Invalid page"), (status = 404, description = "Session not found"),
        (status = 503, description = "Queue unavailable")), tag = "sessions")]
pub async fn list_session_deliveries(
    crate::middleware::tenant_auth::OrgId(tenant): crate::middleware::tenant_auth::OrgId,
    State(connection): State<Option<ConnectionManager>>,
    Path(session): Path<String>,
    query: Result<Query<DeliveryQuery>, QueryRejection>,
) -> Reply {
    let Ok(Query(query)) = query else {
        return queue_error_response(QueueError::Invalid);
    };
    let (mut conn, scope, _) = match authorized_scope(connection, &tenant, &session).await {
        Ok(scope) => scope,
        Err(error) => return error,
    };
    let Ok(cursor) = query.cursor.parse::<u64>() else {
        return queue_error_response(QueueError::Invalid);
    };
    match managed::scan_envelopes(&mut conn, &scope, cursor, query.limit).await {
        Ok(page) => (
            StatusCode::OK,
            Json(json!({"success":true,"data":DeliveryPage {
                deliveries: page.envelopes.into_iter().map(DeliveryStatus::from).collect(),
            next_cursor: page.cursor.to_string(),
            }})),
        ),
        Err(error) => queue_error_response(error),
    }
}

#[utoipa::path(get, path = "/api/runtime/sessions/{sessionId}/deliveries/{messageId}",
    params(("sessionId" = String, Path), ("messageId" = String, Path)),
    responses((status = 200, body = crate::api::dto::common::ApiResponse<DeliveryStatus>),
        (status = 404, description = "Session or message not found"), (status = 503, description = "Queue unavailable")), tag = "sessions")]
pub async fn get_session_delivery(
    crate::middleware::tenant_auth::OrgId(tenant): crate::middleware::tenant_auth::OrgId,
    State(connection): State<Option<ConnectionManager>>,
    Path((session, message)): Path<(String, String)>,
) -> Reply {
    let (mut conn, scope, _) = match authorized_scope(connection, &tenant, &session).await {
        Ok(scope) => scope,
        Err(error) => return error,
    };
    delivery_reply(managed::get(&mut conn, &scope, &message).await)
}

#[utoipa::path(post, path = "/api/runtime/sessions/{sessionId}/deliveries/{messageId}/resolve",
    params(("sessionId" = String, Path), ("messageId" = String, Path)),
    request_body = ResolveDeliveryRequest,
    responses((status = 200, body = crate::api::dto::common::ApiResponse<DeliveryStatus>),
        (status = 400, description = "Invalid resolution"), (status = 404, description = "Target not found"),
        (status = 409, description = "Delivery state changed or target conflicts"),
        (status = 503, description = "Service unavailable")), tag = "sessions")]
pub async fn resolve_session_delivery(
    crate::middleware::tenant_auth::OrgId(tenant): crate::middleware::tenant_auth::OrgId,
    State(connection): State<Option<ConnectionManager>>,
    State(engine): State<Arc<ExecutionEngine>>,
    State(client): State<Option<Arc<RuntimeClient>>>,
    Path((session, message)): Path<(String, String)>,
    body: Result<Json<ResolveDeliveryRequest>, axum::extract::rejection::JsonRejection>,
) -> Reply {
    let Ok(Json(request)) = body else {
        return queue_error_response(QueueError::Invalid);
    };
    let (mut conn, scope, route) = match authorized_scope(connection, &tenant, &session).await {
        Ok(scope) => scope,
        Err(error) => return error,
    };
    match request {
        ResolveDeliveryRequest::Fail => {
            delivery_reply(managed::fail_blocked(&mut conn, &scope, &message).await)
        }
        ResolveDeliveryRequest::Select {
            instance_id,
            request_id,
        } => {
            let Some(client) = client else {
                return workflow_runtime_error_response(WorkflowRuntimeError::RuntimeUnavailable);
            };
            if let Err(error) = engine
                .authorize_execution(&route.workflow_id, &instance_id, &tenant)
                .await
            {
                return workflow_runtime_error_response(
                    crate::api::services::workflow_runtime::map_execution_error(error),
                );
            }
            if let Err(error) = client
                .get_input_request(&tenant, &instance_id, &request_id)
                .await
            {
                return workflow_runtime_error_response(error.into());
            }
            delivery_reply(
                managed::resolve(
                    &mut conn,
                    &scope,
                    &message,
                    &managed::InputTarget {
                        instance_id,
                        request_id,
                    },
                )
                .await,
            )
        }
    }
}

fn delivery_reply(result: managed::QueueResult<managed::Envelope>) -> Reply {
    match result {
        Ok(envelope) => (
            StatusCode::OK,
            Json(json!({"success":true,"data":DeliveryStatus::from(envelope)})),
        ),
        Err(error) => queue_error_response(error),
    }
}
