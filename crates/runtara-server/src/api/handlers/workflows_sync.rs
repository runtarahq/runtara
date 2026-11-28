//! Synchronous Workflow Execution HTTP Handler
//!
//! Provides a low-latency synchronous execution endpoint that:
//! - Bypasses database queuing and worker polling
//! - Returns results immediately in HTTP response
//! - Runs the workflow in-process on the embedded wasmtime engine
//! - Still supports side effects (HTTP, random, timestamps)

use crate::engine::Engine;
use axum::{
    body::Bytes,
    extract::{Path, Query, State},
    http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, Uri, header::CONTENT_TYPE},
    response::{IntoResponse, Json, Response},
};
use serde_json::{Value, json};
use std::sync::Arc;
use tracing::instrument;

use crate::api::handlers::common::execution_error_response;
use crate::workers::execution_engine::{SyncExecution, SyncRequest};

// ============================================================================
// HTTP Handlers
// ============================================================================

/// Execute a workflow synchronously with minimal latency
///
/// This endpoint provides immediate execution results without creating
/// database records or checkpoints. Ideal for low-latency use cases.
///
/// Accepts ANY HTTP method (GET, POST, PUT, DELETE, PATCH, etc.)
/// Request data (method, URI, headers, body) is forwarded to the workflow as inputs.
/// Always executes the latest version of the workflow.
///
/// # Performance
/// - First execution: ~50-100ms overhead + execution time
/// - Cached executions: ~5-10ms overhead + execution time
/// - Hard timeout: 30 seconds
///
/// # Limitations
/// - No execution history in database
/// - No checkpoint/replay support
/// - Not suitable for long-running workflows
#[utoipa::path(
    post,
    path = "/api/runtime/events/http-sync/{workflow_id}",
    params(
        ("workflow_id" = String, Path, description = "Workflow identifier"),
    ),
    request_body(content = String, description = "Optional raw HTTP request body (accepts any content type or no body)", content_type = "application/octet-stream"),
    responses(
        (status = 200, description = "Execution completed (may be success or failure)", body = Value),
        (status = 400, description = "A run label was given: public endpoints do not take one", body = Value),
        (status = 404, description = "Workflow not found or not compiled", body = Value),
        (status = 500, description = "Internal server error", body = Value)
    ),
    tag = "Event Capture"
)]
// Headers and URI are the caller's (credentials, signed query strings): never span fields.
#[instrument(skip(engine, pool, body, headers, uri), fields(workflow_id = %workflow_id))]
#[allow(clippy::too_many_arguments)]
pub async fn capture_http_event_sync(
    Path(workflow_id): Path<String>,
    Query(metadata): Query<crate::api::dto::workflows::RunLabelQuery>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    State(engine): State<Arc<dyn Engine>>,
    State(pool): State<sqlx::PgPool>,
    body: Bytes,
) -> Response {
    // Anyone may send here: a run label names a record, so whoever sends
    // one could take or overwrite another's. Labels come from callers who
    // authenticate (the execute API, MCP) or from a workflow's own step.
    if metadata.run_label.is_some() {
        return crate::api::dto::workflows::RunLabelQuery::refused_publicly().into_response();
    }
    run_sync_for(workflow_id, None, method, uri, headers, engine, pool, body).await
}

/// The synchronous run for an authenticated caller (MCP's
/// `execute_workflow_sync`, on the internal routes): the same run as the
/// public endpoint, with the run label the caller may give.
#[instrument(skip(engine, pool, body, headers, uri), fields(workflow_id = %workflow_id))]
#[allow(clippy::too_many_arguments)]
pub async fn run_workflow_sync_for_caller(
    Path(workflow_id): Path<String>,
    Query(metadata): Query<crate::api::dto::workflows::RunLabelQuery>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    State(engine): State<Arc<dyn Engine>>,
    State(pool): State<sqlx::PgPool>,
    body: Bytes,
) -> Response {
    run_sync_for(
        workflow_id,
        metadata.run_label,
        method,
        uri,
        headers,
        engine,
        pool,
        body,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn run_sync_for(
    workflow_id: String,
    run_label: Option<String>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    engine: Arc<dyn Engine>,
    pool: sqlx::PgPool,
    body: Bytes,
) -> Response {
    // An unauthenticated webhook: the workflow's tenant, among those this
    // process serves. A workflow id two served tenants share names neither.
    let found: Result<Vec<String>, sqlx::Error> = crate::middleware::tenant_path::finding_the_tenant(
        sqlx::query_scalar(
            "SELECT tenant_id FROM workflows WHERE workflow_id = $1 AND tenant_id = ANY($2) LIMIT 2",
        )
        .bind(&workflow_id)
        .bind(crate::config::served_tenants())
        .fetch_all(&pool),
    )
    .await;
    let tenants = match found {
        Ok(tenants) => tenants,
        Err(error) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(
                    json!({ "success": false, "error": format!("Failed to look up workflow: {error}") }),
                ),
            )
                .into_response();
        }
    };
    let [tenant_id] = tenants.as_slice() else {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({ "success": false, "error": "Workflow not found" })),
        )
            .into_response();
    };
    let tenant_id = tenant_id.clone();

    // Build inputs from HTTP request data
    let inputs = build_inputs_from_http_request(&method, &uri, &headers, &body);

    // Execute synchronously via the shared engine
    // The run is the workflow's tenant's.
    match runtara_store_postgres::tenant_scope::scoped(
        tenant_id.clone(),
        engine.run_sync(SyncRequest {
            run_label,
            tenant_id: &tenant_id,
            workflow_id: &workflow_id,
            version: None,
            inputs,
        }),
    )
    .await
    {
        Ok(result) => answer(result),
        Err(e) => {
            tracing::debug!("Sync execution failed: {e}");
            execution_error_response(&e).into_response()
        }
    }
}

/// What a synchronous run is answered with: the workflow's own answer, when
/// it succeeded and its output says one (`httpResponse`); otherwise the
/// run's result, `success` saying how it went (200 either way).
pub fn answer(result: SyncExecution) -> Response {
    if result.success
        && let Some(own) = result.outputs.get("httpResponse")
    {
        return own_answer(own);
    }
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
    (StatusCode::OK, Json(body)).into_response()
}

/// Headers a workflow's answer may not set: what HTTP sets for the
/// connection and the body, cookies (the API's origin is not a workflow's to
/// set them on), and what the server's own CORS and transport settings say.
fn not_the_workflows(name: &str) -> bool {
    const SET_BY_THE_SERVER: &[&str] = &[
        "connection",
        "content-length",
        "keep-alive",
        "set-cookie",
        "strict-transport-security",
        "te",
        "trailer",
        "transfer-encoding",
        "upgrade",
    ];
    SET_BY_THE_SERVER.contains(&name)
        || name.starts_with("access-control-")
        || name.starts_with("proxy-")
}

/// A workflow's own answer: `status` (100 to 599, 200 if not given),
/// `headers` (each a string), and `body` (a string as it is, any other JSON
/// value as JSON, nothing for null). An answer that is not one is a 500.
fn own_answer(own: &Value) -> Response {
    let refused = |why: &str| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(
                json!({ "success": false, "error": format!("The workflow's httpResponse {why}") }),
            ),
        )
            .into_response()
    };
    let Some(own) = own.as_object() else {
        return refused("is not an object");
    };
    let status = match own.get("status") {
        None | Some(Value::Null) => StatusCode::OK,
        Some(status) => match status
            .as_u64()
            .and_then(|status| u16::try_from(status).ok())
            .filter(|status| (100..=599).contains(status))
            .and_then(|status| StatusCode::from_u16(status).ok())
        {
            Some(status) => status,
            None => return refused("has a status that is not one"),
        },
    };
    let (body, default_type) = match own.get("body") {
        None | Some(Value::Null) => (Vec::new(), None),
        Some(Value::String(text)) => (text.clone().into_bytes(), Some("text/plain; charset=utf-8")),
        Some(other) => (other.to_string().into_bytes(), Some("application/json")),
    };
    let mut response = Response::new(axum::body::Body::from(body));
    *response.status_mut() = status;
    let headers = response.headers_mut();
    if let Some(given) = own.get("headers").and_then(Value::as_object) {
        for (name, value) in given {
            let Ok(name) = HeaderName::from_bytes(name.to_ascii_lowercase().as_bytes()) else {
                continue;
            };
            let Some(Ok(value)) = value.as_str().map(HeaderValue::from_str) else {
                continue;
            };
            if !not_the_workflows(name.as_str()) {
                headers.append(name, value);
            }
        }
    }
    if let Some(default_type) = default_type
        && !headers.contains_key(CONTENT_TYPE)
    {
        headers.insert(CONTENT_TYPE, HeaderValue::from_static(default_type));
    }
    response
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Build workflow inputs from HTTP request data
///
/// Converts the raw HTTP request into a JSON object that can be passed
/// to the workflow execution engine.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workers::execution_engine::SyncExecutionMetrics;

    fn ran(success: bool, outputs: Value) -> SyncExecution {
        SyncExecution {
            success,
            outputs,
            error: None,
            stderr: None,
            metrics: SyncExecutionMetrics {
                execution_duration_seconds: 0.1,
                max_memory_mb: 1.0,
                total_duration_seconds: 0.2,
            },
        }
    }

    async fn read(response: Response) -> (StatusCode, HeaderMap, Vec<u8>) {
        let (parts, body) = response.into_parts();
        let body = axum::body::to_bytes(body, 1 << 20).await.unwrap();
        (parts.status, parts.headers, body.to_vec())
    }

    #[tokio::test]
    async fn a_workflow_answers_a_handshake_itself() {
        let (status, headers, body) = read(answer(ran(
            true,
            json!({"httpResponse": {"status": 200, "body": {"challenge": "abc"}}}),
        )))
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(headers[CONTENT_TYPE], "application/json");
        assert_eq!(body, br#"{"challenge":"abc"}"#);

        let (status, headers, body) = read(answer(ran(
            true,
            json!({"httpResponse": {"status": 202, "headers": {"Content-Type": "text/xml", "X-Kept": "1"}, "body": "<ok/>"}}),
        )))
        .await;
        assert_eq!(status, StatusCode::ACCEPTED);
        assert_eq!(headers[CONTENT_TYPE], "text/xml");
        assert_eq!(headers["x-kept"], "1");
        assert_eq!(body, b"<ok/>");

        let (status, headers, body) =
            read(answer(ran(true, json!({"httpResponse": {"status": 204}})))).await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        assert!(!headers.contains_key(CONTENT_TYPE));
        assert!(body.is_empty());
    }

    #[tokio::test]
    async fn a_workflow_without_its_own_answer_is_answered_as_before() {
        let (status, _, body) = read(answer(ran(true, json!({"x": 1})))).await;
        assert_eq!(status, StatusCode::OK);
        let body: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["success"], true);
        assert_eq!(body["outputs"], json!({"x": 1}));
        // A run that failed is answered with its result, whatever it said.
        let (status, _, body) =
            read(answer(ran(false, json!({"httpResponse": {"status": 200}})))).await;
        assert_eq!(status, StatusCode::OK);
        let body: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["success"], false);
    }

    #[tokio::test]
    async fn an_answer_sets_nothing_that_is_not_the_workflows() {
        for status in [json!(99), json!(600), json!("200"), json!(-1)] {
            let (got, _, _) = read(answer(ran(
                true,
                json!({"httpResponse": {"status": status}}),
            )))
            .await;
            assert_eq!(got, StatusCode::INTERNAL_SERVER_ERROR, "{status}");
        }
        let (got, _, _) = read(answer(ran(true, json!({"httpResponse": "ok"})))).await;
        assert_eq!(got, StatusCode::INTERNAL_SERVER_ERROR);

        let (_, headers, _) = read(answer(ran(
            true,
            json!({"httpResponse": {"headers": {
                "Set-Cookie": "s=1", "Content-Length": "99", "Transfer-Encoding": "chunked",
                "Access-Control-Allow-Origin": "*", "Connection": "close",
                "bad name": "x", "X-Number": 1, "X-Ok": "yes"
            }, "body": "hi"}}),
        )))
        .await;
        let names: Vec<_> = headers.keys().map(|name| name.as_str()).collect();
        assert_eq!(names, ["x-ok", "content-type"]);
    }
}
