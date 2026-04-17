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
use runtara_management_sdk::ListEventsOptions;
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::PgPool;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::sleep;
use tracing::{debug, error, info};
use uuid::Uuid;

use crate::api::handlers::chat::{ChatEvent, chat_event_type, make_event, parse_debug_event};
use crate::api::handlers::common::execution_error_response;
use crate::api::repositories::trigger_stream::TriggerStreamPublisher;
use crate::api::services::{session_queue, session_token};
use crate::runtime_client::RuntimeClient;
use crate::workers::execution_engine::{ExecutionEngine, QueueRequest, TriggerSource};

/// Request body for creating a session.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateSessionRequest {
    /// Input data for the scenario
    #[serde(default)]
    pub data: Value,

    /// Variables for the scenario
    #[serde(default)]
    pub variables: Value,

    /// Scenario version to execute (defaults to current)
    pub version: Option<i32>,
}

/// Request body for submitting an event to a session.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubmitEventRequest {
    /// Simple text message (wrapped as `{"message": value}`)
    pub message: Option<String>,

    /// Structured payload (used directly)
    pub payload: Option<Value>,
}

/// Create a new session, start execution, and return an SSE stream.
///
/// POST /api/runtime/scenarios/{id}/sessions
#[allow(clippy::too_many_arguments)]
pub async fn create_session(
    crate::middleware::tenant_auth::OrgId(tenant_id): crate::middleware::tenant_auth::OrgId,
    State(pool): State<PgPool>,
    State(trigger_stream): State<Option<Arc<TriggerStreamPublisher>>>,
    State(runtime_client): State<Option<Arc<RuntimeClient>>>,
    State(valkey_conn): State<Option<ConnectionManager>>,
    State(engine): State<Arc<ExecutionEngine>>,
    Path(scenario_id): Path<String>,
    body: axum::body::Bytes,
) -> Result<axum::response::Response, (StatusCode, Json<Value>)> {
    let runtime_client = runtime_client.ok_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"success": false, "message": "Runtime client not configured"})),
        )
    })?;

    let trigger_stream = trigger_stream.ok_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"success": false, "message": "Trigger stream not configured"})),
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

    // Sign session token (for future public API use)
    let token = session_token::sign(&tenant_id, &scenario_id, &session_id).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"success": false, "message": format!("Failed to sign session token: {}", e)})),
        )
    })?;

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
            tenant_id: &tenant_id,
            scenario_id: &scenario_id,
            version: request.version,
            inputs,
            debug: false,
            correlation_id: None,
            trigger_source: TriggerSource::Session,
        })
        .await
        .map_err(|e| execution_error_response(&e))?;

    let instance_id = result.instance_id.to_string();

    // Store session metadata in Valkey
    if let Err(e) = session_queue::set_session_meta(
        &mut valkey,
        &tenant_id,
        &session_id,
        &instance_id,
        &scenario_id,
    )
    .await
    {
        error!(error = %e, "Failed to store session metadata in Valkey");
    }

    // Build SSE stream with session_created preamble + queue-drain bridge.
    // `pool` and `trigger_stream` states are retained as configuration probes
    // (presence validated above); queue operations go through `engine`.
    let _ = pool;
    let _ = trigger_stream;
    let stream = build_session_event_stream(SessionStreamParams {
        client: runtime_client,
        valkey,
        engine,
        instance_id,
        scenario_id,
        tenant_id,
        session_id,
        token,
    });

    let sse = Sse::new(stream).keep_alive(KeepAlive::default());

    let headers = [
        (header::CACHE_CONTROL, "no-cache, no-store, must-revalidate"),
        (header::HeaderName::from_static("x-accel-buffering"), "no"),
    ];

    Ok((headers, sse).into_response())
}

/// Submit an event to a session queue.
///
/// POST /api/runtime/sessions/{sessionId}/events
pub async fn submit_event(
    crate::middleware::tenant_auth::OrgId(tenant_id): crate::middleware::tenant_auth::OrgId,
    State(valkey_conn): State<Option<ConnectionManager>>,
    Path(session_id): Path<String>,
    Json(request): Json<SubmitEventRequest>,
) -> (StatusCode, Json<Value>) {
    let mut valkey = match valkey_conn {
        Some(c) => c,
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({"success": false, "message": "Valkey not configured"})),
            );
        }
    };

    // Build event payload
    let event = if let Some(payload) = request.payload {
        payload
    } else if let Some(message) = request.message {
        json!({"message": message})
    } else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"success": false, "message": "Either 'message' or 'payload' is required"})),
        );
    };

    // Push to queue
    if let Err(e) = session_queue::push_event(&mut valkey, &tenant_id, &session_id, &event).await {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"success": false, "message": format!("Failed to queue event: {}", e)})),
        );
    }

    (StatusCode::OK, Json(json!({"success": true})))
}

/// SSE event stream for an existing session (reconnect).
///
/// GET /api/runtime/sessions/{sessionId}/events
#[allow(clippy::too_many_arguments)]
pub async fn session_event_stream(
    crate::middleware::tenant_auth::OrgId(tenant_id): crate::middleware::tenant_auth::OrgId,
    State(pool): State<PgPool>,
    State(trigger_stream): State<Option<Arc<TriggerStreamPublisher>>>,
    State(runtime_client): State<Option<Arc<RuntimeClient>>>,
    State(valkey_conn): State<Option<ConnectionManager>>,
    State(engine): State<Arc<ExecutionEngine>>,
    Path(session_id): Path<String>,
) -> Result<axum::response::Response, (StatusCode, Json<Value>)> {
    let runtime_client = runtime_client.ok_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"success": false, "message": "Runtime client not configured"})),
        )
    })?;

    let trigger_stream = trigger_stream.ok_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"success": false, "message": "Trigger stream not configured"})),
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

    // Generate token for the response
    let token = session_token::sign(&tenant_id, &meta.scenario_id, &session_id).unwrap_or_default();

    // `pool` / `trigger_stream` are kept as configuration probes (presence
    // validated above); queue operations go through the shared engine.
    let _ = pool;
    let _ = trigger_stream;
    let stream = build_session_event_stream(SessionStreamParams {
        client: runtime_client,
        valkey,
        engine,
        instance_id: meta.instance_id,
        scenario_id: meta.scenario_id,
        tenant_id,
        session_id,
        token,
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
pub async fn session_pending_input(
    crate::middleware::tenant_auth::OrgId(tenant_id): crate::middleware::tenant_auth::OrgId,
    State(runtime_client): State<Option<Arc<RuntimeClient>>>,
    State(valkey_conn): State<Option<ConnectionManager>>,
    Path(session_id): Path<String>,
) -> (StatusCode, Json<Value>) {
    let client = match runtime_client {
        Some(c) => c,
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({"success": false, "message": "Runtime client not configured"})),
            );
        }
    };

    let mut valkey = match valkey_conn {
        Some(c) => c,
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({"success": false, "message": "Valkey not configured"})),
            );
        }
    };

    let meta = match session_queue::get_session_meta(&mut valkey, &tenant_id, &session_id).await {
        Ok(Some(m)) => m,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({"success": false, "message": "Session not found"})),
            );
        }
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(
                    json!({"success": false, "message": format!("Failed to read session: {}", e)}),
                ),
            );
        }
    };

    let instance_id = meta.instance_id;

    // Query pending input events
    let input_options = ListEventsOptions::new()
        .with_limit(100)
        .with_event_type("custom")
        .with_subtype("external_input_requested")
        .with_sort_order(runtara_management_sdk::EventSortOrder::Asc);

    let input_events = match client.list_events(&instance_id, Some(input_options)).await {
        Ok(result) => result.events,
        Err(e) => {
            let msg = e.to_string();
            if msg.contains("not found") {
                return (
                    StatusCode::NOT_FOUND,
                    Json(
                        json!({"success": false, "message": format!("Instance not found: {}", instance_id)}),
                    ),
                );
            }
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(
                    json!({"success": false, "message": format!("Failed to query events: {}", msg)}),
                ),
            );
        }
    };

    let end_options = ListEventsOptions::new()
        .with_limit(1000)
        .with_event_type("custom")
        .with_subtype("step_debug_end");

    let end_events = match client.list_events(&instance_id, Some(end_options)).await {
        Ok(result) => result.events,
        Err(_) => vec![],
    };

    let completed_tool_ids: std::collections::HashSet<String> = end_events
        .iter()
        .filter_map(|event| {
            event
                .payload
                .as_ref()
                .and_then(|p| p.get("step_id"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        })
        .collect();

    let pending: Vec<Value> = input_events
        .iter()
        .filter_map(|event| {
            let data = event.payload.as_ref()?;
            let signal_id = data.get("signal_id")?.as_str()?.to_string();
            let ai_step_id = data.get("ai_agent_step_id").and_then(|v| v.as_str());
            let tool_name = data.get("tool_name").and_then(|v| v.as_str());
            let step_id = data.get("step_id").and_then(|v| v.as_str());
            let call_number = data
                .get("call_number")
                .and_then(|v| v.as_u64())
                .map(|v| v as u32);

            let check_step_id = match (ai_step_id, tool_name, call_number) {
                (Some(step), Some(tool), Some(num)) => {
                    format!("{}.tool.{}.{}", step, tool, num)
                }
                _ => step_id.unwrap_or("").to_string(),
            };

            if !check_step_id.is_empty() && completed_tool_ids.contains(&check_step_id) {
                return None;
            }

            Some(json!({
                "signalId": signal_id,
                "toolName": tool_name,
                "message": data.get("message")
                    .or_else(|| data.get("step_name"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("External input requested"),
                "responseSchema": data.get("response_schema"),
            }))
        })
        .collect();

    let count = pending.len();

    (
        StatusCode::OK,
        Json(json!({
            "success": true,
            "data": {
                "instanceId": instance_id,
                "pendingInputs": pending,
                "count": count
            }
        })),
    )
}

/// Guard that stops an instance when dropped (e.g., client disconnects SSE stream).
struct InstanceStopGuard {
    client: Arc<RuntimeClient>,
    instance_id: String,
}

impl Drop for InstanceStopGuard {
    fn drop(&mut self) {
        let client = self.client.clone();
        let instance_id = self.instance_id.clone();
        tokio::spawn(async move {
            if let Err(e) = client.stop_instance(&instance_id).await {
                debug!(
                    instance_id = %instance_id,
                    error = %e,
                    "Failed to stop instance on stream drop (may already be completed)"
                );
            } else {
                debug!(instance_id = %instance_id, "Instance stopped on session stream drop");
            }
        });
    }
}

/// Find the most recent pending signal_id for an instance.
async fn find_pending_signal_id(client: &Arc<RuntimeClient>, instance_id: &str) -> Option<String> {
    let options = ListEventsOptions::new()
        .with_limit(10)
        .with_event_type("custom")
        .with_subtype("external_input_requested")
        .with_sort_order(runtara_management_sdk::EventSortOrder::Desc);

    let result = client.list_events(instance_id, Some(options)).await.ok()?;
    result
        .events
        .first()
        .and_then(|ev| ev.payload.as_ref())
        .and_then(|p| p.get("signal_id"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

/// Parameters for the session SSE stream.
struct SessionStreamParams {
    client: Arc<RuntimeClient>,
    valkey: ConnectionManager,
    engine: Arc<ExecutionEngine>,
    instance_id: String,
    scenario_id: String,
    tenant_id: String,
    session_id: String,
    token: String,
}

/// Start a new instance for the session, returning the new instance_id.
///
/// The message is NOT passed as input data — it stays in the queue and will be
/// delivered as a signal when the instance hits WaitForSignal.
async fn start_new_instance(
    engine: &Arc<ExecutionEngine>,
    valkey: &mut ConnectionManager,
    tenant_id: &str,
    scenario_id: &str,
    session_id: &str,
) -> Option<String> {
    let inputs = json!({
        "data": { "sessionId": session_id },
        "variables": {},
    });

    match engine
        .queue(QueueRequest {
            tenant_id,
            scenario_id,
            version: None,
            inputs,
            debug: false,
            correlation_id: None,
            trigger_source: TriggerSource::Session,
        })
        .await
    {
        Ok(result) => {
            let new_instance_id = result.instance_id.to_string();
            info!(
                session_id = %session_id,
                instance_id = %new_instance_id,
                "Started new instance for session"
            );
            // Update session metadata with new instance_id
            let _ = session_queue::set_session_meta(
                valkey,
                tenant_id,
                session_id,
                &new_instance_id,
                scenario_id,
            )
            .await;
            Some(new_instance_id)
        }
        Err(e) => {
            error!(error = ?e, session_id = %session_id, "Failed to start new instance for session");
            None
        }
    }
}

/// Build SSE stream for a session.
///
/// The stream follows instances across their lifecycle:
/// 1. Poll events from the current instance
/// 2. When instance completes, emit `done` and wait for a new message in the queue
/// 3. When a message arrives, start a new instance and resume polling
/// 4. Repeat until timeout or client disconnect
fn build_session_event_stream(
    params: SessionStreamParams,
) -> impl Stream<Item = Result<Event, std::convert::Infallible>> {
    async_stream::stream! {
        let SessionStreamParams {
            client,
            mut valkey,
            engine,
            instance_id: initial_instance_id,
            scenario_id,
            tenant_id: org_id,
            session_id,
            token,
        } = params;

        // Emit session_created event
        let created = json!({
            "type": "session_created",
            "token": token,
            "sessionId": session_id,
            "instanceId": initial_instance_id,
        });
        yield Ok(Event::default()
            .event("session_created")
            .json_data(&created)
            .unwrap_or_else(|_| Event::default().event("error").data("serialization error")));

        let mut current_instance_id = initial_instance_id;
        let mut _stop_guard = InstanceStopGuard {
            client: client.clone(),
            instance_id: current_instance_id.clone(),
        };

        // Wait for instance to register
        sleep(Duration::from_millis(500)).await;

        let poll_interval = Duration::from_millis(300);
        let idle_poll_interval = Duration::from_millis(500);
        let max_duration = Duration::from_secs(600); // 10 minute session timeout
        let start_time = std::time::Instant::now();
        let mut session_ended = false;

        while !session_ended && start_time.elapsed() < max_duration {
            // === INSTANCE LOOP: poll current instance until it terminates ===
            let mut event_offset: u32 = 0;
            let mut instance_done = false;
            let mut waiting_for_input = false;

            while !instance_done && !session_ended && start_time.elapsed() < max_duration {
                // Check instance status
                match client.get_instance_info(&current_instance_id).await {
                    Ok(info) => {
                        if info.status.is_terminal() {
                            // Flush remaining events
                            if let Ok(result) = client.list_events(&current_instance_id, Some(ListEventsOptions {
                                event_type: Some("custom".to_string()),
                                sort_order: Some(runtara_management_sdk::EventSortOrder::Asc),
                                limit: Some(100),
                                offset: Some(event_offset),
                                ..Default::default()
                            })).await {
                                for event in result.events {
                                    if let Some(payload) = &event.payload {
                                        let chat_events = parse_debug_event(
                                            event.subtype.as_deref(),
                                            payload,
                                        );
                                        for chat_event in chat_events {
                                            yield Ok(make_event(chat_event_type(&chat_event), &chat_event));
                                        }
                                    }
                                    event_offset += 1;
                                }
                            }

                            match info.status {
                                runtara_management_sdk::InstanceStatus::Completed => {
                                    let duration = match (info.started_at, info.finished_at) {
                                        (Some(s), Some(f)) => Some((f - s).num_milliseconds() as f64 / 1000.0),
                                        _ => None,
                                    };
                                    yield Ok(make_event("done", &ChatEvent::Done {
                                        outputs: info.output,
                                        duration_seconds: duration,
                                    }));
                                }
                                runtara_management_sdk::InstanceStatus::Failed => {
                                    let error_msg = info.error
                                        .or(info.stderr)
                                        .unwrap_or_else(|| "Execution failed".to_string());
                                    yield Ok(make_event("error", &ChatEvent::Error {
                                        message: error_msg,
                                    }));
                                }
                                _ => {
                                    yield Ok(make_event("done", &ChatEvent::Done {
                                        outputs: None,
                                        duration_seconds: None,
                                    }));
                                }
                            }
                            instance_done = true;
                            continue;
                        }

                        debug!(
                            instance_id = %current_instance_id,
                            status = ?info.status,
                            "Session polling"
                        );
                    }
                    Err(e) => {
                        if start_time.elapsed() > Duration::from_secs(30) {
                            error!(error = %e, "Session polling failed after 30s");
                            yield Ok(make_event("error", &ChatEvent::Error {
                                message: format!("Failed to get instance status: {}", e),
                            }));
                            session_ended = true;
                            continue;
                        }
                    }
                }

                // Fetch new events
                let options = ListEventsOptions {
                    event_type: Some("custom".to_string()),
                    sort_order: Some(runtara_management_sdk::EventSortOrder::Asc),
                    limit: Some(100),
                    offset: Some(event_offset),
                    ..Default::default()
                };

                match client.list_events(&current_instance_id, Some(options)).await {
                    Ok(result) => {
                        for event in result.events {
                            if let Some(payload) = &event.payload {
                                if event.subtype.as_deref() == Some("external_input_requested") {
                                    let has_schema = payload.get("response_schema")
                                        .map(|v| !v.is_null())
                                        .unwrap_or(false);

                                    if has_schema {
                                        waiting_for_input = true;
                                        let chat_events = parse_debug_event(event.subtype.as_deref(), payload);
                                        for chat_event in chat_events {
                                            yield Ok(make_event(chat_event_type(&chat_event), &chat_event));
                                        }
                                    } else {
                                        let signal_id = payload
                                            .get("signal_id")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or("")
                                            .to_string();

                                        match session_queue::pop_event(&mut valkey, &org_id, &session_id).await {
                                            Ok(Some(queued_event)) => {
                                                let payload_bytes = serde_json::to_vec(&queued_event).unwrap_or_default();
                                                if let Err(e) = client.send_custom_signal(&current_instance_id, &signal_id, Some(&payload_bytes)).await {
                                                    error!(error = %e, "Failed to auto-deliver queued signal");
                                                    waiting_for_input = true;
                                                    let chat_events = parse_debug_event(event.subtype.as_deref(), payload);
                                                    for chat_event in chat_events {
                                                        yield Ok(make_event(chat_event_type(&chat_event), &chat_event));
                                                    }
                                                } else {
                                                    waiting_for_input = false;
                                                }
                                            }
                                            Ok(None) => {
                                                waiting_for_input = true;
                                                let chat_events = parse_debug_event(event.subtype.as_deref(), payload);
                                                for chat_event in chat_events {
                                                    yield Ok(make_event(chat_event_type(&chat_event), &chat_event));
                                                }
                                            }
                                            Err(e) => {
                                                error!(error = %e, "Failed to pop event from queue");
                                                waiting_for_input = true;
                                                let chat_events = parse_debug_event(event.subtype.as_deref(), payload);
                                                for chat_event in chat_events {
                                                    yield Ok(make_event(chat_event_type(&chat_event), &chat_event));
                                                }
                                            }
                                        }
                                    }
                                } else {
                                    let chat_events = parse_debug_event(event.subtype.as_deref(), payload);
                                    for chat_event in chat_events {
                                        yield Ok(make_event(chat_event_type(&chat_event), &chat_event));
                                    }
                                }
                            }
                            event_offset += 1;
                        }

                        // Poll queue when waiting for input
                        if waiting_for_input
                            && let Ok(Some(queued_event)) = session_queue::pop_event(&mut valkey, &org_id, &session_id).await
                            && let Some(sig) = find_pending_signal_id(&client, &current_instance_id).await
                        {
                            let payload_bytes = serde_json::to_vec(&queued_event).unwrap_or_default();
                            if client.send_custom_signal(&current_instance_id, &sig, Some(&payload_bytes)).await.is_ok() {
                                waiting_for_input = false;
                            }
                        }
                    }
                    Err(_) => {
                        // Instance might not be ready yet
                    }
                }

                sleep(poll_interval).await;
            }

            // === IDLE PHASE: instance done, wait for next message in queue ===
            if instance_done && !session_ended {
                debug!(session_id = %session_id, "Instance done, waiting for next message");

                loop {
                    if start_time.elapsed() >= max_duration {
                        session_ended = true;
                        break;
                    }

                    match session_queue::has_events(&mut valkey, &org_id, &session_id).await {
                        Ok(true) => {
                            // Message waiting — start a new instance (message stays in queue
                            // for the queue-drain bridge to deliver on WaitForSignal)
                            match start_new_instance(
                                &engine,
                                &mut valkey,
                                &org_id,
                                &scenario_id,
                                &session_id,
                            ).await {
                                Some(new_id) => {
                                    // Emit instance_started event
                                    let started = json!({
                                        "type": "instance_started",
                                        "instanceId": new_id,
                                    });
                                    yield Ok(Event::default()
                                        .event("instance_started")
                                        .json_data(&started)
                                        .unwrap_or_else(|_| Event::default().event("error").data("serialization error")));

                                    current_instance_id = new_id.clone();
                                    _stop_guard = InstanceStopGuard {
                                        client: client.clone(),
                                        instance_id: new_id,
                                    };

                                    // Wait for new instance to register
                                    sleep(Duration::from_millis(500)).await;
                                    break; // Back to instance loop
                                }
                                None => {
                                    yield Ok(make_event("error", &ChatEvent::Error {
                                        message: "Failed to start new instance for session".to_string(),
                                    }));
                                    session_ended = true;
                                    break;
                                }
                            }
                        }
                        Ok(false) => {
                            // No message yet, keep waiting
                            sleep(idle_poll_interval).await;
                        }
                        Err(e) => {
                            error!(error = %e, "Failed to check queue during idle phase");
                            sleep(idle_poll_interval).await;
                        }
                    }
                }
            }
        }

        if start_time.elapsed() >= max_duration {
            yield Ok(make_event("error", &ChatEvent::Error {
                message: "Session timed out after 10 minutes".to_string(),
            }));
        }

        // _stop_guard is dropped here, stopping the current instance.
    }
}
