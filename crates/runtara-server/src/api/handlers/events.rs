//! Event API Handlers
//!
//! HTTP handlers for trigger-based workflow execution.
//! When a trigger is found for the given trigger_id, durably records an async
//! execution request. The outbox relay publishes it to the trigger stream.

use axum::{
    body::Bytes,
    extract::{FromRequest, Multipart, Path, Request, State},
    http::{HeaderMap, Method, StatusCode, Uri},
    response::Json,
};
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

use crate::api::dto::trigger_event::TriggerEvent;
use crate::api::repositories::triggers::TriggerRepository;
use crate::workers::execution_outbox::source_idempotency_key;

/// Default cap on the request body of the public, unauthenticated webhook
/// ingest endpoints when `WEBHOOK_MAX_BODY_BYTES` is unset or unparseable.
/// Without a cap a single POST can buffer until the process runs out of
/// memory and crashes.
pub const DEFAULT_WEBHOOK_MAX_BODY_BYTES: usize = 10 * 1024 * 1024;

/// Maximum accepted webhook request body in bytes. Reads
/// `WEBHOOK_MAX_BODY_BYTES`, falling back to [`DEFAULT_WEBHOOK_MAX_BODY_BYTES`];
/// a `0` or unparseable value falls back to the default rather than wedging
/// ingest by rejecting every request.
pub fn webhook_max_body_bytes() -> usize {
    parse_webhook_max_body_bytes(std::env::var("WEBHOOK_MAX_BODY_BYTES").ok().as_deref())
}

fn parse_webhook_max_body_bytes(raw: Option<&str>) -> usize {
    raw.and_then(|v| v.parse::<usize>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(DEFAULT_WEBHOOK_MAX_BODY_BYTES)
}

/// HTTP trigger execution endpoint
///
/// When a trigger is found for the given trigger_id:
/// 1. Looks up the trigger in invocation_trigger table
/// 2. Validates trigger is active
/// 3. Commits a TriggerEvent plus admission reservation to the durable outbox
/// 4. Returns instance_id for tracking
///
/// Returns 404 if trigger is not found.
///
/// Accepts ANY HTTP method (GET, POST, PUT, DELETE, PATCH, etc.)
/// Body is optional and can be any content type including multipart/form-data
#[utoipa::path(
    post,
    path = "/api/runtime/events/http/{trigger_id}/{action}",
    params(
        ("trigger_id" = String, Path, description = "Trigger ID"),
        ("action" = String, Path, description = "Action name")
    ),
    request_body(content = String, description = "Optional raw HTTP request body (accepts any content type including multipart/form-data)", content_type = "application/octet-stream"),
    responses(
        (status = 200, description = "Event captured/queued successfully"),
        (status = 404, description = "Trigger not found"),
        (status = 400, description = "Trigger is inactive"),
        (status = 500, description = "Internal server error")
    ),
    tag = "Event Capture"
)]
pub async fn capture_http_event(
    State(pool): State<PgPool>,
    State(connections): State<std::sync::Arc<runtara_connections::ConnectionsFacade>>,
    State(engine): State<std::sync::Arc<crate::workers::execution_engine::ExecutionEngine>>,
    Path((trigger_id, action)): Path<(String, String)>,
    request: Request,
) -> Result<(StatusCode, Json<Value>), (StatusCode, Json<Value>)> {
    // Events are webhook endpoints — tenant is implicit (single-tenant runtime)
    let tenant_id = crate::config::tenant_id().to_string();

    // Extract parts from request
    let (parts, body) = request.into_parts();
    let method = parts.method.clone();
    let uri = parts.uri.clone();
    let headers = parts.headers.clone();

    // Check if this is a multipart/form-data request
    let content_type = headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    let is_multipart = content_type.starts_with("multipart/form-data");

    // Save raw bytes for webhook signature verification (non-multipart only).
    let mut raw_body_bytes: Option<Bytes> = None;

    // Parse body based on content type
    let parsed_body = if is_multipart {
        // Reconstruct request for multipart parsing
        let reconstructed = Request::from_parts(parts, body);
        match parse_multipart(reconstructed).await {
            Ok(multipart_data) => multipart_data,
            Err(e) => {
                eprintln!("Failed to parse multipart: {}", e);
                let error_response = json!({
                    "success": false,
                    "message": format!("Failed to parse multipart/form-data: {}", e)
                });
                return Err((StatusCode::BAD_REQUEST, Json(error_response)));
            }
        }
    } else {
        // Read body as bytes for non-multipart requests
        let bytes = match axum::body::to_bytes(body, webhook_max_body_bytes()).await {
            Ok(b) => b,
            Err(e) => {
                eprintln!("Failed to read body: {}", e);
                let error_response = json!({
                    "success": false,
                    "message": format!("Failed to read request body: {}", e)
                });
                return Err((StatusCode::BAD_REQUEST, Json(error_response)));
            }
        };
        raw_body_bytes = Some(bytes.clone());
        parse_body_as_json(&bytes)
    };

    // Look up the trigger
    let trigger_repo = TriggerRepository::new(pool.clone());
    let trigger = match trigger_repo.get_by_id(&trigger_id, Some(&tenant_id)).await {
        Ok(Some(t)) => t,
        Ok(None) => {
            tracing::debug!("Trigger '{}' not found", trigger_id);
            let error_response = json!({
                "success": false,
                "error": "Not found",
            });
            return Err((StatusCode::NOT_FOUND, Json(error_response)));
        }
        Err(e) => {
            eprintln!("Database error looking up trigger: {}", e);
            let error_response = json!({
                "success": false,
                "error": "DATABASE_ERROR",
                "message": format!("Failed to look up trigger: {}", e)
            });
            return Err((StatusCode::INTERNAL_SERVER_ERROR, Json(error_response)));
        }
    };

    // Verify webhook signature if the trigger has a linked connection.
    if let Some(ref body_bytes) = raw_body_bytes
        && let Err(e) = crate::api::services::webhook_verification::verify_webhook(
            &connections,
            &trigger.configuration,
            &tenant_id,
            &headers,
            body_bytes,
        )
        .await
    {
        let error_response = json!({
            "success": false,
            "message": format!("Webhook verification failed: {}", e)
        });
        return Err((StatusCode::UNAUTHORIZED, Json(error_response)));
    }

    // Process the trigger
    {
        // Check if trigger is active
        if !trigger.active {
            let error_response = json!({
                "success": false,
                "message": "Trigger is inactive"
            });
            return Err((StatusCode::BAD_REQUEST, Json(error_response)));
        }

        // Build inputs from HTTP request
        let inputs = build_inputs_from_http_parsed(&method, &uri, &headers, parsed_body);

        // Extract headers for the event
        let header_pairs: Vec<(String, String)> = headers
            .iter()
            .filter_map(|(name, value)| {
                value
                    .to_str()
                    .ok()
                    .map(|v| (name.to_string(), v.to_string()))
            })
            .collect();

        // An upstream delivery ID makes repeated webhooks an idempotent
        // source operation. If a provider does not supply one we still get a
        // durable request, but cannot safely collapse two identical-looking
        // business events into one execution.
        let supplied_identity = webhook_delivery_identity(&headers);
        let idempotency_key = supplied_identity.as_ref().map_or_else(
            || source_idempotency_key("http-event", &Uuid::new_v4().to_string()),
            |identity| {
                source_idempotency_key("http-event", &format!("{trigger_id}:{action}:{identity}"))
            },
        );
        let instance_id = supplied_identity
            .as_ref()
            .map_or_else(Uuid::new_v4, |identity| {
                Uuid::new_v5(
                    &Uuid::NAMESPACE_URL,
                    format!("{tenant_id}:{trigger_id}:{action}:{identity}").as_bytes(),
                )
            });

        // Read debug flag from trigger configuration
        let debug = trigger
            .configuration
            .as_ref()
            .and_then(|c| c.get("debug"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        // Build TriggerEvent
        let event = TriggerEvent::http_event(
            instance_id.to_string(),
            tenant_id.clone(),
            trigger.workflow_id.clone(),
            None, // use current version
            inputs,
            false, // track_events - could be from trigger config
            trigger_id.clone(),
            action.clone(),
            method.to_string(),
            header_pairs,
            debug,
        );

        match engine
            .enqueue_trigger_event(&tenant_id, event, idempotency_key)
            .await
        {
            Ok(enqueued) => {
                let response = json!({
                    "status": "queued",
                    "instance_id": enqueued.instance_id,
                    "request_id": enqueued.request_id,
                    "duplicate": enqueued.duplicate,
                    "workflow_id": trigger.workflow_id,
                });
                Ok((StatusCode::OK, Json(response)))
            }
            Err(crate::workers::execution_engine::ExecutionError::EntitlementDenied(denial)) => {
                Err((StatusCode::FORBIDDEN, Json(denial.json_body())))
            }
            Err(error) => {
                let status = error.http_status();
                let error_response = json!({
                    "success": false,
                    "message": format!("Failed to queue execution: {error}")
                });
                Err((status, Json(error_response)))
            }
        }
    }
}

/// Providers use several spelling conventions for their immutable delivery
/// ID. Prefer the explicit idempotency header, then common delivery/request
/// IDs. A missing header intentionally does not hash the body: two legitimate
/// events with equal JSON must remain two executions.
fn webhook_delivery_identity(headers: &HeaderMap) -> Option<String> {
    [
        "idempotency-key",
        "x-idempotency-key",
        "x-request-id",
        "x-delivery-id",
    ]
    .into_iter()
    .find_map(|header| {
        headers
            .get(header)
            .and_then(|value| value.to_str().ok())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
    })
}

/// Parse multipart/form-data into a JSON object
///
/// Text fields become string values.
/// File fields become objects with: { filename, content_type, data (base64), size }
async fn parse_multipart(request: Request) -> Result<Value, String> {
    let mut multipart = Multipart::from_request(request, &())
        .await
        .map_err(|e| format!("Failed to create multipart extractor: {}", e))?;

    let mut fields: serde_json::Map<String, Value> = serde_json::Map::new();
    let mut files: serde_json::Map<String, Value> = serde_json::Map::new();

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| format!("Failed to read multipart field: {}", e))?
    {
        let name = field.name().unwrap_or("unnamed").to_string();
        let file_name = field.file_name().map(|s| s.to_string());
        let content_type = field.content_type().map(|s| s.to_string());

        let data = field
            .bytes()
            .await
            .map_err(|e| format!("Failed to read field data: {}", e))?;

        if let Some(filename) = file_name {
            // This is a file upload
            let base64_data =
                base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &data);
            let file_info = json!({
                "filename": filename,
                "content_type": content_type,
                "data": base64_data,
                "size": data.len()
            });

            // Handle multiple files with same field name as array
            if let Some(existing) = files.get_mut(&name) {
                if let Value::Array(arr) = existing {
                    arr.push(file_info);
                } else {
                    let prev = existing.clone();
                    *existing = json!([prev, file_info]);
                }
            } else {
                files.insert(name, file_info);
            }
        } else {
            // This is a text field
            let text_value = String::from_utf8_lossy(&data).to_string();

            // Try to parse as JSON, fall back to string
            let value = serde_json::from_str(&text_value).unwrap_or(Value::String(text_value));

            // Handle multiple values with same field name as array
            if let Some(existing) = fields.get_mut(&name) {
                if let Value::Array(arr) = existing {
                    arr.push(value);
                } else {
                    let prev = existing.clone();
                    *existing = json!([prev, value]);
                }
            } else {
                fields.insert(name, value);
            }
        }
    }

    Ok(json!({
        "fields": fields,
        "files": files
    }))
}

/// Parse raw bytes as JSON value
fn parse_body_as_json(body: &Bytes) -> Value {
    if body.is_empty() {
        return Value::Null;
    }

    // Try to parse as JSON first
    serde_json::from_slice(body).unwrap_or_else(|_| {
        // Fall back to string (base64 for binary)
        match std::str::from_utf8(body) {
            Ok(s) => Value::String(s.to_string()),
            Err(_) => Value::String(base64::Engine::encode(
                &base64::engine::general_purpose::STANDARD,
                body,
            )),
        }
    })
}

/// Build workflow inputs from HTTP request data with pre-parsed body
///
/// Returns the canonical Runtara format: `{"data": {...}, "variables": {}}`
fn build_inputs_from_http_parsed(
    method: &Method,
    uri: &Uri,
    headers: &HeaderMap,
    parsed_body: Value,
) -> Value {
    // Extract query string
    let query = uri.query().map(|q| q.to_string());

    // Extract content-type
    let content_type = headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    // Return in canonical wrapped format
    json!({
        "data": {
            "method": method.to_string(),
            "uri": uri.to_string(),
            "path": uri.path(),
            "query": query,
            "content_type": content_type,
            "body": parsed_body,
        },
        "variables": {}
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_parsing() {
        assert_eq!(
            parse_webhook_max_body_bytes(None),
            DEFAULT_WEBHOOK_MAX_BODY_BYTES
        );
        assert_eq!(
            parse_webhook_max_body_bytes(Some("0")),
            DEFAULT_WEBHOOK_MAX_BODY_BYTES
        );
        assert_eq!(
            parse_webhook_max_body_bytes(Some("not-a-number")),
            DEFAULT_WEBHOOK_MAX_BODY_BYTES
        );
        assert_eq!(parse_webhook_max_body_bytes(Some("1048576")), 1048576);
    }

    /// Boundary check for the explicit cap in `capture_http_event` (the raw
    /// `to_bytes(body, webhook_max_body_bytes())` path). A body of exactly
    /// `LIMIT` bytes must be accepted; `LIMIT + 1` must error.
    #[tokio::test]
    async fn to_bytes_cap_boundary() {
        const LIMIT: usize = 64;

        let at_limit = axum::body::Body::from(vec![b'a'; LIMIT]);
        let bytes = axum::body::to_bytes(at_limit, LIMIT)
            .await
            .expect("a body of exactly LIMIT bytes must be accepted");
        assert_eq!(bytes.len(), LIMIT);

        let over_limit = axum::body::Body::from(vec![b'a'; LIMIT + 1]);
        assert!(
            axum::body::to_bytes(over_limit, LIMIT).await.is_err(),
            "a body of LIMIT + 1 bytes must be rejected"
        );
    }

    /// Boundary check for the defense-in-depth `DefaultBodyLimit` layer on the
    /// `event_routes` router (covers the `Bytes`-extractor sync handler).
    /// `LIMIT` bytes → 200; `LIMIT + 1` → 413 Payload Too Large.
    #[tokio::test]
    async fn default_body_limit_layer_boundary() {
        use axum::Router;
        use axum::extract::DefaultBodyLimit;
        use axum::http::{Request, StatusCode};
        use axum::routing::post;
        use tower::ServiceExt;

        const LIMIT: usize = 64;

        async fn echo(body: axum::body::Bytes) -> StatusCode {
            let _ = body;
            StatusCode::OK
        }

        fn app() -> Router {
            Router::new()
                .route("/ingest", post(echo))
                .layer(DefaultBodyLimit::max(LIMIT))
        }

        let at_limit = app()
            .oneshot(
                Request::post("/ingest")
                    .body(axum::body::Body::from(vec![b'a'; LIMIT]))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            at_limit.status(),
            StatusCode::OK,
            "a body of exactly LIMIT bytes must pass the layer"
        );

        let over_limit = app()
            .oneshot(
                Request::post("/ingest")
                    .body(axum::body::Body::from(vec![b'a'; LIMIT + 1]))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            over_limit.status(),
            StatusCode::PAYLOAD_TOO_LARGE,
            "a body of LIMIT + 1 bytes must be rejected by the layer"
        );
    }
}
