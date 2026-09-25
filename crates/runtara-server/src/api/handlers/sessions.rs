use crate::runtime_types::ListEventsOptions;
use axum::{
    Json,
    extract::{Path, State},
    http::{StatusCode, header},
    response::{
        IntoResponse, Sse,
        sse::{Event, KeepAlive},
    },
};
use futures::stream::Stream;
use redis::aio::ConnectionManager;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::PgPool;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::sleep;
use tracing::debug;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::api::handlers::chat::{
    ChatEvent, chat_event_type, extract_message_from_outputs, make_event, parse_debug_event,
};
use crate::api::handlers::common::execution_error_response;
use crate::api::services::session_queue;
use crate::api::services::session_queue::managed::{self, QueueError, QueueScope};
use crate::runtime_client::RuntimeClient;
use crate::workers::execution_engine::{ExecutionEngine, QueueRequest, TriggerSource};

mod delivery;
pub use delivery::*;

/// Request body for creating a session.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateSessionRequest {
    /// Input data for the workflow
    #[serde(default)]
    pub data: Value,

    /// Variables for the workflow
    #[serde(default)]
    pub variables: Value,

    /// Workflow version to execute (defaults to current)
    pub version: Option<i32>,
}

/// Request body for submitting an event to a session.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SubmitEventRequest {
    pub message_id: String,
    pub operation_id: String,
    /// Simple text message (wrapped as `{"message": value}`)
    pub message: Option<String>,

    /// Structured payload (used directly)
    pub payload: Option<Value>,

    /// The open input request this message answers. Optional only when exactly
    /// one request is open; the binding is fixed when the message is accepted.
    pub request_id: Option<String>,
}

/// Create a new session, start execution, and return an SSE stream.
///
/// POST /api/runtime/workflows/{id}/sessions
#[allow(clippy::too_many_arguments)]
pub async fn create_session(
    crate::middleware::tenant_auth::OrgId(tenant_id): crate::middleware::tenant_auth::OrgId,
    State(pool): State<PgPool>,
    State(runtime_client): State<Option<Arc<RuntimeClient>>>,
    State(valkey_conn): State<Option<ConnectionManager>>,
    State(engine): State<Arc<ExecutionEngine>>,
    Path(workflow_id): Path<String>,
    body: axum::body::Bytes,
) -> Result<axum::response::Response, (StatusCode, Json<Value>)> {
    let runtime_client = runtime_client.ok_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"success": false, "message": "Runtime client not configured"})),
        )
    })?;

    let mut valkey = valkey_conn.ok_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"success": false, "message": "Valkey not configured"})),
        )
    })?;

    let request: CreateSessionRequest = if body.is_empty() {
        CreateSessionRequest {
            data: json!({}),
            variables: json!({}),
            version: None,
        }
    } else {
        serde_json::from_slice(&body).map_err(|e| {
            (
                StatusCode::BAD_REQUEST,
                Json(json!({"success": false, "message": format!("Invalid request body: {}", e)})),
            )
        })?
    };

    // Generate session ID
    let session_id = Uuid::new_v4().to_string();

    // Inject sessionId into inputs.data
    let mut data = if request.data.is_object() {
        request.data.clone()
    } else {
        json!({})
    };
    if let Some(obj) = data.as_object_mut() {
        obj.insert("sessionId".to_string(), json!(session_id));
    }

    let inputs = json!({
        "data": data,
        "variables": request.variables,
    });

    // Queue execution via the shared engine
    let result = engine
        .queue(QueueRequest {
            run_label: None,
            tenant_id: &tenant_id,
            workflow_id: &workflow_id,
            version: request.version,
            inputs: inputs.clone(),
            debug: false,
            correlation_id: None,
            idempotency_key: None,
            trigger_source: TriggerSource::Session,
            instance_id: None,
        })
        .await
        .map_err(|e| execution_error_response(&e))?;

    let instance_id = result.instance_id.to_string();

    let scope = QueueScope::new(&tenant_id, &session_id).map_err(queue_error_response)?;
    managed::configure_route(
        &mut valkey,
        &scope,
        &managed::SessionRoute {
            workflow_id: workflow_id.clone(),
            instance_id: instance_id.clone(),
        },
    )
    .await
    .map_err(queue_error_response)?;
    let _ = pool;
    let stream = build_session_event_stream(SessionStreamParams {
        client: runtime_client,
        valkey,
        instance_id,
        tenant_id,
        session_id,
    });

    let sse = Sse::new(stream).keep_alive(KeepAlive::default());

    let headers = [
        (header::CACHE_CONTROL, "no-cache, no-store, must-revalidate"),
        (header::HeaderName::from_static("x-accel-buffering"), "no"),
    ];

    Ok((headers, sse).into_response())
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct DeliveryStatus {
    pub message_id: String,
    pub operation_id: String,
    pub state: managed::DeliveryState,
    pub reason: Option<managed::DeliveryReason>,
    pub receipt_id: Option<String>,
    pub instance_id: Option<String>,
    pub request_id: Option<String>,
    pub enqueued_at_ms: u64,
}
impl From<managed::Envelope> for DeliveryStatus {
    fn from(value: managed::Envelope) -> Self {
        Self {
            message_id: value.message_id,
            operation_id: value.operation_id,
            state: value.state,
            reason: value.reason,
            receipt_id: value.receipt_id,
            instance_id: value.target.as_ref().map(|t| t.instance_id.clone()),
            request_id: value.target.map(|t| t.request_id),
            enqueued_at_ms: value.enqueued_at_ms,
        }
    }
}
fn queue_error_response(error: QueueError) -> (StatusCode, Json<Value>) {
    let (status, code) = match error {
        QueueError::Invalid => (StatusCode::BAD_REQUEST, "QUEUE_INVALID_REQUEST"),
        QueueError::Conflict => (StatusCode::CONFLICT, "QUEUE_OPERATION_CONFLICT"),
        QueueError::LeaseLost => (StatusCode::CONFLICT, "QUEUE_LEASE_LOST"),
        QueueError::NotFound => (StatusCode::NOT_FOUND, "QUEUE_NOT_FOUND"),
        QueueError::Corrupt => (StatusCode::SERVICE_UNAVAILABLE, "QUEUE_CORRUPT"),
        QueueError::Backend(_) => (StatusCode::SERVICE_UNAVAILABLE, "QUEUE_UNAVAILABLE"),
    };
    (
        status,
        Json(json!({"success":false,"code":code,"message":error.to_string(),"data":null})),
    )
}

fn input_target_response(code: &str, message: &str) -> (StatusCode, Json<Value>) {
    (
        StatusCode::CONFLICT,
        Json(json!({"success":false,"code":code,"message":message,"data":null})),
    )
}

/// Choose the request a new message answers, while the sender can still see it.
/// Binding later (at delivery) could attach a late reply to a different wait.
async fn bind_session_message(
    client: &RuntimeClient,
    tenant_id: &str,
    instance_id: &str,
    requested: Option<&str>,
) -> Result<managed::InputTarget, (StatusCode, Json<Value>)> {
    use runtara_core::persistence::inputs::InputError;
    let page = match client
        .list_input_requests(tenant_id, &[instance_id.into()], 0, u32::MAX)
        .await
    {
        Ok(page) => page,
        Err(InputError::NotFound) => {
            return Err(input_target_response(
                "INPUT_NOT_WAITING",
                "The session is not waiting for input",
            ));
        }
        Err(_) => {
            return Err((
                StatusCode::SERVICE_UNAVAILABLE,
                Json(
                    json!({"success":false,"code":"INPUT_DISCOVERY_UNAVAILABLE","message":"Input discovery unavailable","data":null}),
                ),
            ));
        }
    };
    let selected = match requested {
        Some(requested) => page.requests.iter().find(|r| r.request_id == requested),
        None if page.requests.len() > 1 => {
            return Err(input_target_response(
                "INPUT_AMBIGUOUS",
                "Several inputs are waiting; specify requestId",
            ));
        }
        None => page.requests.first(),
    };
    let request = selected.ok_or_else(|| {
        input_target_response(
            "INPUT_NOT_WAITING",
            "The requested input is not waiting for a response",
        )
    })?;
    Ok(managed::InputTarget {
        instance_id: instance_id.into(),
        request_id: request.request_id.clone(),
    })
}

/// Durable queue acceptance is distinct from acceptance by a workflow wait. The
/// message is bound to its request here; a retry replays that original binding.
#[utoipa::path(post,path="/api/runtime/sessions/{sessionId}/events",params(("sessionId"=String,Path)),request_body=SubmitEventRequest,
    responses((status=200,description="Message retained or enqueue replayed",body=crate::api::dto::common::ApiResponse<DeliveryStatus>),(status=400,description="Invalid envelope"),(status=404,description="Session not found"),(status=409,description="Message identity conflict, or no/several inputs waiting"),(status=503,description="Queue unavailable")),tag="sessions")]
pub async fn submit_event(
    crate::middleware::tenant_auth::OrgId(tenant_id): crate::middleware::tenant_auth::OrgId,
    State(runtime_client): State<Option<Arc<RuntimeClient>>>,
    State(valkey_conn): State<Option<ConnectionManager>>,
    Path(session_id): Path<String>,
    body: Result<Json<SubmitEventRequest>, axum::extract::rejection::JsonRejection>,
) -> (StatusCode, Json<Value>) {
    let Ok(Json(request)) = body else {
        return queue_error_response(QueueError::Invalid);
    };
    let event = match (request.message, request.payload) {
        (Some(message), None) => json!({"message":message}),
        (None, Some(payload)) => payload,
        _ => return queue_error_response(QueueError::Invalid),
    };
    let Some(mut conn) = valkey_conn else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"success":false,"code":"QUEUE_UNAVAILABLE","message":"Queue unavailable"})),
        );
    };
    let scope = match QueueScope::new(&tenant_id, &session_id) {
        Ok(scope) => scope,
        Err(error) => return queue_error_response(error),
    };
    let route = match managed::session_route(&mut conn, &scope).await {
        Ok(route) => route,
        Err(error) => return queue_error_response(error),
    };
    // A retried message keeps the target it was first bound to, even after
    // that request was answered; the queue replays or reports the conflict.
    let existing = match managed::get(&mut conn, &scope, &request.message_id).await {
        Ok(existing) => Some(existing),
        Err(QueueError::NotFound) => None,
        Err(error) => return queue_error_response(error),
    };
    let target = match existing {
        Some(existing) => {
            if request.request_id.is_some()
                && existing.target.as_ref().map(|t| &t.request_id) != request.request_id.as_ref()
            {
                return queue_error_response(QueueError::Conflict);
            }
            existing.target
        }
        None => {
            let Some(client) = runtime_client else {
                return (
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(
                        json!({"success":false,"code":"RUNTIME_UNAVAILABLE","message":"Runtime client not configured"}),
                    ),
                );
            };
            match bind_session_message(
                &client,
                &tenant_id,
                &route.instance_id,
                request.request_id.as_deref(),
            )
            .await
            {
                Ok(target) => Some(target),
                Err(response) => return response,
            }
        }
    };
    let enqueued = match &target {
        Some(target) => {
            managed::enqueue_targeted(
                &mut conn,
                &scope,
                &request.message_id,
                &request.operation_id,
                &event,
                target,
            )
            .await
        }
        None => {
            managed::enqueue(
                &mut conn,
                &scope,
                &request.message_id,
                &request.operation_id,
                &event,
            )
            .await
        }
    };
    match enqueued {
        Ok(envelope) => (
            StatusCode::OK,
            Json(json!({"success":true,"data":DeliveryStatus::from(envelope)})),
        ),
        Err(error) => queue_error_response(error),
    }
}

/// SSE event stream for an existing session (reconnect).
///
/// GET /api/runtime/sessions/{sessionId}/events
#[allow(clippy::too_many_arguments)]
pub async fn session_event_stream(
    crate::middleware::tenant_auth::OrgId(tenant_id): crate::middleware::tenant_auth::OrgId,
    State(pool): State<PgPool>,
    State(runtime_client): State<Option<Arc<RuntimeClient>>>,
    State(valkey_conn): State<Option<ConnectionManager>>,
    Path(session_id): Path<String>,
) -> Result<axum::response::Response, (StatusCode, Json<Value>)> {
    let runtime_client = runtime_client.ok_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"success": false, "message": "Runtime client not configured"})),
        )
    })?;

    let mut valkey = valkey_conn.ok_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"success": false, "message": "Valkey not configured"})),
        )
    })?;

    // Read session metadata from Valkey
    let meta = session_queue::get_session_meta(&mut valkey, &tenant_id, &session_id)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(
                    json!({"success": false, "message": format!("Failed to read session: {}", e)}),
                ),
            )
        })?
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                Json(json!({"success": false, "message": "Session not found"})),
            )
        })?;

    // `pool` is retained for route/state compatibility; execution admission
    // has already gone through the shared durable engine.
    let _ = pool;
    let stream = build_session_event_stream(SessionStreamParams {
        client: runtime_client,
        valkey,
        instance_id: meta.instance_id,
        tenant_id,
        session_id,
    });

    let sse = Sse::new(stream).keep_alive(KeepAlive::default());

    let headers = [
        (header::CACHE_CONTROL, "no-cache, no-store, must-revalidate"),
        (header::HeaderName::from_static("x-accel-buffering"), "no"),
    ];

    Ok((headers, sse).into_response())
}

/// Get pending input for a session.
///
/// GET /api/runtime/sessions/{sessionId}/pending-input
#[utoipa::path(
    get,
    path = "/api/runtime/sessions/{sessionId}/pending-input",
    params(("sessionId" = String, Path, description = "Session ID")),
    responses(
        (status = 200, description = "Authoritative pending requests", body = crate::api::dto::common::ApiResponse<crate::api::services::pending_inputs::PendingInputPage>),
        (status = 404, description = "Session or request not found"),
        (status = 503, description = "Discovery unavailable", body = crate::api::services::workflow_runtime::InputSubmissionErrorResponse),
    ),
    tag = "sessions"
)]
pub async fn session_pending_input(
    crate::middleware::tenant_auth::OrgId(tenant_id): crate::middleware::tenant_auth::OrgId,
    State(runtime_client): State<Option<Arc<RuntimeClient>>>,
    State(valkey_conn): State<Option<ConnectionManager>>,
    Path(session_id): Path<String>,
) -> (StatusCode, Json<Value>) {
    let client = match runtime_client {
        Some(c) => c,
        None => {
            return crate::api::handlers::step_events::workflow_runtime_error_response(
                crate::api::services::workflow_runtime::WorkflowRuntimeError::RuntimeUnavailable,
            );
        }
    };

    let mut valkey = match valkey_conn {
        Some(c) => c,
        None => {
            return crate::api::handlers::step_events::workflow_runtime_error_response(
                crate::api::services::workflow_runtime::WorkflowRuntimeError::RuntimeUnavailable,
            );
        }
    };

    let meta = match session_queue::get_session_meta(&mut valkey, &tenant_id, &session_id).await {
        Ok(Some(m)) => m,
        Ok(None) => {
            return crate::api::handlers::step_events::workflow_runtime_error_response(
                runtara_core::persistence::inputs::InputError::NotFound.into(),
            );
        }
        Err(_) => {
            return crate::api::handlers::step_events::workflow_runtime_error_response(
                crate::api::services::workflow_runtime::WorkflowRuntimeError::RuntimeUnavailable,
            );
        }
    };

    let instance_id = meta.instance_id;

    match crate::api::services::pending_inputs::pending_input_page(
        &client,
        &tenant_id,
        &instance_id,
    )
    .await
    {
        Ok(page) => (StatusCode::OK, Json(json!({"success":true,"data":page}))),
        Err(error) => {
            crate::api::handlers::step_events::workflow_runtime_error_response(error.into())
        }
    }
}

/// SSE observes durable session routing. Disconnecting a viewer does not cancel
/// execution or own delivery; the managed queue worker runs independently.
struct SessionStreamParams {
    client: Arc<RuntimeClient>,
    valkey: ConnectionManager,
    instance_id: String,
    tenant_id: String,
    session_id: String,
}
fn build_session_event_stream(
    params: SessionStreamParams,
) -> impl Stream<Item = Result<Event, std::convert::Infallible>> {
    async_stream::stream! {
        let SessionStreamParams {client,mut valkey,instance_id,tenant_id,session_id}=params;
        let created=json!({"sessionId":session_id,"instanceId":instance_id});
        yield Ok(Event::default().event("session_created").json_data(&created).unwrap_or_else(|_|Event::default().event("error").data("serialization error")));
        let mut current=instance_id;
        let mut offset=0;
        let mut terminal_reported=false;
        let started=std::time::Instant::now();
        while started.elapsed()<Duration::from_secs(600) {
            if let Ok(Some(meta))=session_queue::get_session_meta(&mut valkey,&tenant_id,&session_id).await
                && meta.instance_id != current {
                current=meta.instance_id;
                offset=0;
                terminal_reported=false;
                yield Ok(Event::default().event("started").json_data(json!({"instance_id":current})).unwrap_or_default());
            }
            let info=match client.get_instance_info(&current).await {
                Ok(info) if info.tenant_id==tenant_id=>info,
                Ok(_)=>{ yield Ok(make_event("error",&ChatEvent::Error {message:"Session execution unavailable".into()})); break; }
                Err(_)=>{ sleep(Duration::from_millis(500)).await; continue; }
            };
            let mut more_events=false;
            match client.list_events(&current,Some(ListEventsOptions {
                event_type:Some("custom".into()),sort_order:Some(crate::runtime_types::EventSortOrder::Asc),
                limit:Some(100),offset:Some(offset),..Default::default()
            })).await {
                Ok(page)=>{
                    more_events=page.events.len()==100;
                    for event in page.events {
                        if let Some(payload)=event.payload {
                            for chat_event in parse_debug_event(event.subtype.as_deref(),&payload) {
                                yield Ok(make_event(chat_event_type(&chat_event),&chat_event));
                            }
                        }
                        offset+=1;
                    }
                }
                Err(error)=>debug!(instance_id=%current,error=%error,"Session history temporarily unavailable"),
            }
            if info.status.is_terminal() && !terminal_reported && !more_events {
                terminal_reported=true;
                if info.status==crate::runtime_types::InstanceStatus::Failed {
                    yield Ok(make_event("error",&ChatEvent::Error {message:info.error.or(info.stderr).unwrap_or_else(||"Execution failed".into())}));
                } else {
                    if let Some(message)=extract_message_from_outputs(info.output.as_ref()) { yield Ok(make_event("message",&message)); }
                    let duration=match (info.started_at,info.finished_at) {(Some(a),Some(b))=>Some((b-a).num_milliseconds() as f64/1000.0),_=>None};
                    yield Ok(make_event("done",&ChatEvent::Done {outputs:info.output,duration_seconds:duration}));
                }
            }
            sleep(Duration::from_millis(500)).await;
        }
    }
}
