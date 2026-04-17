//! Synchronous Scenario Execution HTTP Handler
//!
//! Provides a low-latency synchronous execution endpoint that:
//! - Bypasses database queuing and worker polling
//! - Returns results immediately in HTTP response
//! - Uses native binary execution via crun launcher
//! - Still supports side effects (HTTP, random, timestamps)

use axum::{
    body::Bytes,
    extract::{Path, State},
    http::{HeaderMap, Method, StatusCode, Uri},
    response::Json,
};
use serde_json::{Value, json};
use std::sync::Arc;
use tracing::instrument;

use crate::api::handlers::common::execution_error_response;
use crate::workers::execution_engine::{ExecutionEngine, SyncRequest};

// ============================================================================
// HTTP Handlers
// ============================================================================

/// Execute a scenario synchronously with minimal latency
///
/// This endpoint provides immediate execution results without creating
/// database records or checkpoints. Ideal for low-latency use cases.
///
/// Accepts ANY HTTP method (GET, POST, PUT, DELETE, PATCH, etc.)
/// Request data (method, URI, headers, body) is forwarded to the scenario as inputs.
/// Always executes the latest version of the scenario.
///
/// # Performance
/// - First execution: ~50-100ms overhead + execution time
/// - Cached executions: ~5-10ms overhead + execution time
/// - Hard timeout: 30 seconds
///
/// # Limitations
/// - No execution history in database
/// - No checkpoint/replay support
/// - Not suitable for long-running scenarios
#[utoipa::path(
    post,
    path = "/api/runtime/events/http-sync/{scenario_id}",
    params(
        ("scenario_id" = String, Path, description = "Scenario identifier")
    ),
    request_body(content = String, description = "Optional raw HTTP request body (accepts any content type or no body)", content_type = "application/octet-stream"),
    responses(
        (status = 200, description = "Execution completed (may be success or failure)", body = Value),
        (status = 404, description = "Scenario not found or not compiled", body = Value),
        (status = 500, description = "Internal server error", body = Value)
    ),
    tag = "Event Capture"
)]
#[instrument(skip(engine, body), fields(scenario_id = %scenario_id))]
#[allow(clippy::too_many_arguments)]
pub async fn capture_http_event_sync(
    Path(scenario_id): Path<String>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    State(engine): State<Arc<ExecutionEngine>>,
    body: Bytes,
) -> (StatusCode, Json<Value>) {
    // Events are webhook endpoints — tenant is implicit (single-tenant runtime)
    let tenant_id = crate::config::tenant_id().to_string();

    // Build inputs from HTTP request data
    let inputs = build_inputs_from_http_request(&method, &uri, &headers, &body);

    // Execute synchronously via the shared engine
    match engine
        .run_sync(SyncRequest {
            tenant_id: &tenant_id,
            scenario_id: &scenario_id,
            version: None,
            inputs,
        })
        .await
    {
        Ok(result) => {
            // Return the result directly (success or failure both use 200 OK)
            // The client should check the "success" field in the response
            let metrics = json!({
                "executionDurationSeconds": result.metrics.execution_duration_seconds,
                "maxMemoryMb": result.metrics.max_memory_mb,
                "totalDurationSeconds": result.metrics.total_duration_seconds,
            });
            let mut body = json!({
                "success": result.success,
                "outputs": result.outputs,
                "metrics": metrics,
            });
            if let Some(ref err) = result.error {
                body["error"] = json!(err);
            }
            if let Some(ref stderr) = result.stderr {
                body["stderr"] = json!(stderr);
            }
            (StatusCode::OK, Json(body))
        }
        Err(e) => {
            tracing::debug!("Sync execution failed: {e}");
            execution_error_response(&e)
        }
    }
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Build scenario inputs from HTTP request data
///
/// Converts the raw HTTP request into a JSON object that can be passed
/// to the scenario execution engine.
fn build_inputs_from_http_request(
    method: &Method,
    uri: &Uri,
    headers: &HeaderMap,
    body: &Bytes,
) -> Value {
    // Convert headers to a map
    let mut headers_map = serde_json::Map::new();
    for (key, value) in headers.iter() {
        if let Ok(value_str) = value.to_str() {
            headers_map.insert(key.as_str().to_string(), json!(value_str));
        }
    }

    // Try to parse body as JSON, fall back to string or base64
    let body_value = if body.is_empty() {
        Value::Null
    } else if let Ok(json_body) = serde_json::from_slice::<Value>(body) {
        json_body
    } else if let Ok(text_body) = std::str::from_utf8(body) {
        json!(text_body)
    } else {
        // Binary data - encode as base64
        json!(base64::Engine::encode(
            &base64::engine::general_purpose::STANDARD,
            body
        ))
    };

    // Build the inputs object in canonical Runtara format
    json!({
        "data": {
            "method": method.as_str(),
            "uri": uri.to_string(),
            "path": uri.path(),
            "query": uri.query().unwrap_or(""),
            "headers": headers_map,
            "body": body_value,
        },
        "variables": {}
    })
}
