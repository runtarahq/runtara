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
use runtara_management_sdk::ListEventsOptions;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::PgPool;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::sleep;
use tracing::{debug, error};
use utoipa::ToSchema;

use crate::api::handlers::common::execution_error_response;
use crate::api::repositories::trigger_stream::TriggerStreamPublisher;
use crate::runtime_client::RuntimeClient;
use crate::workers::execution_engine::{ExecutionEngine, QueueRequest, TriggerSource};

/// Request body for starting a chat session with an initial message
#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ChatRequest {
    /// User message to send to the AI agent
    pub message: String,

    /// Input data for the scenario (merged with message)
    #[serde(default)]
    pub data: Value,

    /// Variables for the scenario
    #[serde(default)]
    pub variables: Value,

    /// Scenario version to execute (defaults to current)
    pub version: Option<i32>,
}

/// Request body for starting a chat session without an initial message
#[derive(Debug, Deserialize, ToSchema, Default)]
#[serde(rename_all = "camelCase")]
pub struct ChatStartRequest {
    /// Input data for the scenario
    #[serde(default)]
    pub data: Value,

    /// Variables for the scenario
    #[serde(default)]
    pub variables: Value,

    /// Scenario version to execute (defaults to current)
    pub version: Option<i32>,
}

/// SSE event types emitted during chat
#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case", tag = "type")]
pub(crate) enum ChatEvent {
    /// Chat session started — includes instance ID for signal delivery
    Started { instance_id: String },
    /// Memory loaded from previous conversation
    MemoryLoaded {
        message_count: i64,
        messages: Option<Vec<Value>>,
    },
    /// AI Agent is calling an LLM
    LlmStart {
        iteration: u32,
        model: Option<String>,
    },
    /// AI Agent received LLM response
    LlmEnd {
        iteration: u32,
        #[serde(skip_serializing_if = "Option::is_none")]
        response_preview: Option<String>,
    },
    /// AI Agent is calling a tool
    ToolCall {
        tool_name: String,
        iteration: u32,
        call_number: u32,
        arguments: Option<Value>,
    },
    /// Tool returned a result
    ToolResult {
        tool_name: String,
        iteration: u32,
        call_number: u32,
        result: Option<Value>,
        duration_ms: Option<u64>,
    },
    /// Execution is waiting for user input (WaitForSignal)
    WaitingForInput {
        signal_id: String,
        tool_name: Option<String>,
        message: String,
        response_schema: Option<Value>,
    },
    /// AI Agent produced a final text response
    Message {
        content: String,
        iterations: u32,
        tool_calls: Option<Vec<Value>>,
    },
    /// Memory saved after conversation
    MemorySaved { message_count: i64, success: bool },
    /// Generic step event (for non-AI-agent steps in the scenario)
    StepStart {
        step_id: String,
        step_name: Option<String>,
        step_type: String,
    },
    /// Step completed
    StepEnd {
        step_id: String,
        step_name: Option<String>,
        step_type: String,
        outputs: Option<Value>,
        duration_ms: Option<u64>,
    },
    /// Execution completed successfully
    Done {
        outputs: Option<Value>,
        duration_seconds: Option<f64>,
    },
    /// Execution failed
    Error { message: String },
}

/// Start a chat session with an initial message and stream events via SSE.
///
/// The scenario executes asynchronously while this endpoint streams execution events
/// (tool calls, LLM responses, memory operations, pending input requests) as Server-Sent Events.
///
/// For scenarios with WaitForSignal steps (human-in-the-loop), the stream emits a
/// `waiting_for_input` event with a `signal_id`. Use `POST /api/runtime/signals/{instanceId}`
/// to submit the response and resume execution.
#[utoipa::path(
    post,
    path = "/api/runtime/scenarios/{id}/chat",
    params(
        ("id" = String, Path, description = "Scenario ID"),
    ),
    request_body = ChatRequest,
    responses(
        (status = 200, description = "SSE stream of chat events", content_type = "text/event-stream"),
        (status = 400, description = "Invalid request"),
        (status = 404, description = "Scenario not found"),
    ),
    tag = "Chat"
)]
#[allow(clippy::too_many_arguments)]
pub async fn chat_handler(
    org_id: crate::middleware::tenant_auth::OrgId,
    State(pool): State<PgPool>,
    State(trigger_stream): State<Option<Arc<TriggerStreamPublisher>>>,
    State(runtime_client): State<Option<Arc<RuntimeClient>>>,
    State(engine): State<Arc<ExecutionEngine>>,
    Path(scenario_id): Path<String>,
    Json(request): Json<ChatRequest>,
) -> Result<axum::response::Response, (StatusCode, Json<Value>)> {
    // Build data with userMessage
    let mut data = if request.data.is_object() {
        request.data.clone()
    } else {
        json!({})
    };
    if let Some(obj) = data.as_object_mut() {
        obj.insert("userMessage".to_string(), json!(request.message));
    }

    start_chat_stream(
        org_id,
        pool,
        trigger_stream,
        runtime_client,
        engine,
        ChatStreamParams {
            scenario_id,
            data,
            variables: request.variables,
            version: request.version,
        },
    )
    .await
}

/// Start a chat session without an initial message and stream events via SSE.
///
/// Use this when the scenario doesn't require an initial user message to begin
/// (e.g., the AI agent starts the conversation proactively).
#[utoipa::path(
    post,
    path = "/api/runtime/scenarios/{id}/chat/start",
    params(
        ("id" = String, Path, description = "Scenario ID"),
    ),
    request_body(content = ChatStartRequest, content_type = "application/json"),
    responses(
        (status = 200, description = "SSE stream of chat events", content_type = "text/event-stream"),
        (status = 400, description = "Invalid request"),
        (status = 404, description = "Scenario not found"),
    ),
    tag = "Chat"
)]
#[allow(clippy::too_many_arguments)]
pub async fn chat_start_handler(
    org_id: crate::middleware::tenant_auth::OrgId,
    State(pool): State<PgPool>,
    State(trigger_stream): State<Option<Arc<TriggerStreamPublisher>>>,
    State(runtime_client): State<Option<Arc<RuntimeClient>>>,
    State(engine): State<Arc<ExecutionEngine>>,
    Path(scenario_id): Path<String>,
    body: axum::body::Bytes,
) -> Result<axum::response::Response, (StatusCode, Json<Value>)> {
    let request: ChatStartRequest = if body.is_empty() {
        ChatStartRequest::default()
    } else {
        serde_json::from_slice(&body).unwrap_or_default()
    };

    let data = if request.data.is_object() {
        request.data.clone()
    } else {
        json!({})
    };

    start_chat_stream(
        org_id,
        pool,
        trigger_stream,
        runtime_client,
        engine,
        ChatStreamParams {
            scenario_id,
            data,
            variables: request.variables,
            version: request.version,
        },
    )
    .await
}

/// Parameters for starting a chat stream execution.
struct ChatStreamParams {
    scenario_id: String,
    data: Value,
    variables: Value,
    version: Option<i32>,
}

/// Shared logic for starting a chat SSE stream
async fn start_chat_stream(
    crate::middleware::tenant_auth::OrgId(tenant_id): crate::middleware::tenant_auth::OrgId,
    pool: PgPool,
    trigger_stream: Option<Arc<TriggerStreamPublisher>>,
    runtime_client: Option<Arc<RuntimeClient>>,
    engine: Arc<ExecutionEngine>,
    params: ChatStreamParams,
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

    let inputs = json!({
        "data": params.data,
        "variables": params.variables,
    });

    // Queue execution via the shared engine. `pool` and `trigger_stream`
    // states are retained as configuration probes (presence validated above).
    let _ = pool;
    let _ = trigger_stream;
    let result = engine
        .queue(QueueRequest {
            tenant_id: &tenant_id,
            scenario_id: &params.scenario_id,
            version: params.version,
            inputs,
            debug: false,
            correlation_id: None,
            trigger_source: TriggerSource::Chat,
        })
        .await
        .map_err(|e| execution_error_response(&e))?;

    let instance_id = result.instance_id.to_string();

    // Build the SSE stream that polls for events
    let stream = build_event_stream(runtime_client, instance_id, params.scenario_id);

    let sse = Sse::new(stream).keep_alive(KeepAlive::default());

    // Disable nginx proxy buffering for SSE streaming
    let headers = [
        (header::CACHE_CONTROL, "no-cache, no-store, must-revalidate"),
        (header::HeaderName::from_static("x-accel-buffering"), "no"),
    ];

    Ok((headers, sse).into_response())
}

pub(crate) fn build_event_stream(
    client: Arc<RuntimeClient>,
    instance_id: String,
    _scenario_id: String,
) -> impl Stream<Item = Result<Event, std::convert::Infallible>> {
    async_stream::stream! {
        // Emit started event immediately
        yield Ok(make_event("started", &ChatEvent::Started {
            instance_id: instance_id.clone(),
        }));

        // Wait for the instance to register (it goes through trigger → compile → launch)
        sleep(Duration::from_millis(500)).await;

        let mut event_offset: u32 = 0;
        let mut completed = false;
        let poll_interval = Duration::from_millis(300);
        let max_duration = Duration::from_secs(300); // 5 minute timeout
        let start_time = std::time::Instant::now();

        while !completed && start_time.elapsed() < max_duration {
            // Check instance status
            match client.get_instance_info(&instance_id).await {
                Ok(info) => {
                    let status_str = format!("{:?}", info.status);
                    if info.status.is_terminal() {
                        match info.status {
                            runtara_management_sdk::InstanceStatus::Completed => {
                                // Fetch remaining events before sending done
                                if let Ok(result) = client.list_events(&instance_id, Some(ListEventsOptions {
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

                                let duration = match (info.started_at, info.finished_at) {
                                    (Some(s), Some(f)) => Some((f - s).num_milliseconds() as f64 / 1000.0),
                                    _ => None,
                                };

                                yield Ok(make_event("done", &ChatEvent::Done {
                                    outputs: info.output,
                                    duration_seconds: duration,
                                }));
                                completed = true;
                                continue;
                            }
                            runtara_management_sdk::InstanceStatus::Failed => {
                                let error_msg = info.error
                                    .or(info.stderr)
                                    .unwrap_or_else(|| "Execution failed".to_string());
                                yield Ok(make_event("error", &ChatEvent::Error {
                                    message: error_msg,
                                }));
                                completed = true;
                                continue;
                            }
                            _ => {
                                // Cancelled or other terminal
                                yield Ok(make_event("done", &ChatEvent::Done {
                                    outputs: None,
                                    duration_seconds: None,
                                }));
                                completed = true;
                                continue;
                            }
                        }
                    }

                    debug!(instance_id = %instance_id, status = %status_str, "Chat polling");
                }
                Err(e) => {
                    // Connection errors during initial startup are expected
                    if start_time.elapsed() > Duration::from_secs(30) {
                        error!(error = %e, "Chat polling failed after 30s");
                        yield Ok(make_event("error", &ChatEvent::Error {
                            message: format!("Failed to get instance status: {}", e),
                        }));
                        completed = true;
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

            match client.list_events(&instance_id, Some(options)).await {
                Ok(result) => {
                    let count = result.events.len() as u32;
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
                    }
                    event_offset += count;
                }
                Err(_) => {
                    // Instance might not be ready yet, continue polling
                }
            }

            sleep(poll_interval).await;
        }

        if !completed {
            yield Ok(make_event("error", &ChatEvent::Error {
                message: "Chat session timed out after 5 minutes".to_string(),
            }));
        }
    }
}

/// Parse a step debug event into chat-friendly events
pub(crate) fn parse_debug_event(subtype: Option<&str>, payload: &Value) -> Vec<ChatEvent> {
    let step_type = payload
        .get("step_type")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let step_id = payload
        .get("step_id")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let step_name = payload
        .get("step_name")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    match subtype {
        Some("step_debug_start") => parse_debug_start(step_type, step_id, step_name, payload),
        Some("step_debug_end") => parse_debug_end(step_type, step_id, step_name, payload),
        Some("external_input_requested") => {
            let signal_id = payload
                .get("signal_id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let tool_name = payload
                .get("tool_name")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let message = payload
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("Input required")
                .to_string();
            let response_schema = payload.get("response_schema").cloned();

            vec![ChatEvent::WaitingForInput {
                signal_id,
                tool_name,
                message,
                response_schema,
            }]
        }
        _ => vec![],
    }
}

fn parse_debug_start(
    step_type: &str,
    step_id: &str,
    step_name: Option<String>,
    payload: &Value,
) -> Vec<ChatEvent> {
    match step_type {
        "AiAgentMemoryLoad" => vec![], // Will emit on end
        "AiAgentMemorySave" => {
            // Memory save start has message_count in inputs
            let inputs = payload.get("inputs");
            let message_count = inputs
                .and_then(|i| i.get("message_count"))
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            vec![ChatEvent::MemorySaved {
                message_count,
                success: true,
            }]
        }
        "AiAgentMemoryCompaction" => vec![], // Internal, skip
        "AiAgentLlmCall" => {
            let inputs = payload.get("inputs");
            let iteration = inputs
                .and_then(|i| i.get("iteration"))
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u32;
            let model = inputs
                .and_then(|i| i.get("model"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            vec![ChatEvent::LlmStart { iteration, model }]
        }
        "AiAgentToolCall" => {
            let inputs = payload.get("inputs");
            let tool_name = inputs
                .and_then(|i| i.get("tool_name"))
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string();
            let iteration = inputs
                .and_then(|i| i.get("iteration"))
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u32;
            let call_number = inputs
                .and_then(|i| i.get("call_number"))
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u32;
            let arguments = inputs.and_then(|i| i.get("arguments")).cloned();

            vec![ChatEvent::ToolCall {
                tool_name,
                iteration,
                call_number,
                arguments,
            }]
        }
        "AiAgent" => vec![], // The overall AI Agent step — skip start, emit on end
        _ => {
            // Generic step start
            vec![ChatEvent::StepStart {
                step_id: step_id.to_string(),
                step_name,
                step_type: step_type.to_string(),
            }]
        }
    }
}

fn parse_debug_end(
    step_type: &str,
    step_id: &str,
    step_name: Option<String>,
    payload: &Value,
) -> Vec<ChatEvent> {
    // Debug events have structure: payload.outputs = { outputs: {actual data}, stepId, stepType }
    // The inner "outputs" contains the step's actual output data
    let outer_outputs = payload.get("outputs");
    let inner_outputs = outer_outputs.and_then(|o| o.get("outputs"));
    let duration_ms = payload.get("duration_ms").and_then(|v| v.as_u64());

    match step_type {
        "AiAgentMemoryLoad" => {
            let message_count = inner_outputs
                .and_then(|o| o.get("message_count"))
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            let messages = inner_outputs
                .and_then(|o| o.get("messages"))
                .and_then(|v| v.as_array())
                .cloned();
            vec![ChatEvent::MemoryLoaded {
                message_count,
                messages,
            }]
        }
        "AiAgentMemorySave" => vec![], // Emitted from start event (has message_count)
        "AiAgentMemoryCompaction" => vec![], // Internal, skip
        "AiAgentLlmCall" => {
            let iteration = inner_outputs
                .and_then(|o| o.get("iteration"))
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u32;
            let response_preview = inner_outputs
                .and_then(|o| o.get("response_preview"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            vec![ChatEvent::LlmEnd {
                iteration,
                response_preview,
            }]
        }
        "AiAgentToolCall" => {
            let tool_name = inner_outputs
                .and_then(|o| o.get("tool_name"))
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string();
            let iteration = inner_outputs
                .and_then(|o| o.get("iteration"))
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u32;
            let call_number = inner_outputs
                .and_then(|o| o.get("call_number"))
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u32;
            let result = inner_outputs.and_then(|o| o.get("result")).cloned();

            vec![ChatEvent::ToolResult {
                tool_name,
                iteration,
                call_number,
                result,
                duration_ms,
            }]
        }
        "AiAgent" => {
            // The AI Agent step completed — extract the final response
            // Structure: payload.outputs.outputs = { response, iterations, toolCalls }
            let response = inner_outputs
                .and_then(|o| o.get("response"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let iterations = inner_outputs
                .and_then(|o| o.get("iterations"))
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u32;
            let tool_calls = inner_outputs
                .and_then(|o| o.get("toolCalls"))
                .and_then(|v| v.as_array())
                .cloned();

            if !response.is_empty() {
                vec![ChatEvent::Message {
                    content: response,
                    iterations,
                    tool_calls,
                }]
            } else {
                vec![]
            }
        }
        _ => {
            // Generic step end
            vec![ChatEvent::StepEnd {
                step_id: step_id.to_string(),
                step_name,
                step_type: step_type.to_string(),
                outputs: outer_outputs.cloned(),
                duration_ms,
            }]
        }
    }
}

/// Get the SSE event type name for a chat event
pub(crate) fn chat_event_type(event: &ChatEvent) -> &'static str {
    match event {
        ChatEvent::Started { .. } => "started",
        ChatEvent::MemoryLoaded { .. } => "memory_loaded",
        ChatEvent::LlmStart { .. } => "llm_start",
        ChatEvent::LlmEnd { .. } => "llm_end",
        ChatEvent::ToolCall { .. } => "tool_call",
        ChatEvent::ToolResult { .. } => "tool_result",
        ChatEvent::WaitingForInput { .. } => "waiting_for_input",
        ChatEvent::Message { .. } => "message",
        ChatEvent::MemorySaved { .. } => "memory_saved",
        ChatEvent::StepStart { .. } => "step_start",
        ChatEvent::StepEnd { .. } => "step_end",
        ChatEvent::Done { .. } => "done",
        ChatEvent::Error { .. } => "error",
    }
}

/// Create an SSE Event from a chat event
pub(crate) fn make_event(event_type: &str, data: &ChatEvent) -> Event {
    Event::default()
        .event(event_type)
        .json_data(data)
        .unwrap_or_else(|_| Event::default().event("error").data("serialization error"))
}
