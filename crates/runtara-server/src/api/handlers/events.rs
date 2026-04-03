//! Event API Handlers
//!
//! HTTP handlers for trigger-based scenario execution.
//! When a trigger is found for the given trigger_id, publishes to the trigger stream
//! for async execution. Returns 404 if trigger is not found.

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
use crate::api::repositories::trigger_stream::TriggerStreamPublisher;
use crate::api::repositories::triggers::TriggerRepository;

/// HTTP trigger execution endpoint
///
/// When a trigger is found for the given trigger_id:
/// 1. Looks up the trigger in invocation_trigger table
/// 2. Validates trigger is active
/// 3. Publishes a TriggerEvent to the trigger stream for async execution
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

    // Get Redis connection URL from environment
    let redis_url = match crate::valkey::build_redis_url() {
        Some(url) => url,
        None => {
            eprintln!("VALKEY_HOST not configured");
            let error_response = json!({
                "success": false,
                "message": "Redis/Valkey not configured"
            });
            return Err((StatusCode::INTERNAL_SERVER_ERROR, Json(error_response)));
        }
    };

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
        let bytes = match axum::body::to_bytes(body, usize::MAX).await {
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
            &pool,
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

        // Generate instance ID
        let instance_id = Uuid::new_v4();

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
            trigger.scenario_id.clone(),
            None, // use current version
            inputs,
            false, // track_events - could be from trigger config
            trigger_id.clone(),
            action.clone(),
            method.to_string(),
            header_pairs,
            debug,
        );

        // Publish to trigger stream
        let trigger_stream = TriggerStreamPublisher::new(redis_url);
        match trigger_stream.publish(&tenant_id, &event).await {
            Ok(stream_id) => {
                // Update trigger's last_run timestamp
                let _ = update_trigger_last_run(&pool, &trigger_id).await;

                let response = json!({
                    "status": "queued",
                    "instance_id": instance_id.to_string(),
                    "stream_id": stream_id,
                    "scenario_id": trigger.scenario_id,
                });
                Ok((StatusCode::OK, Json(response)))
            }
            Err(e) => {
                eprintln!("Failed to publish to trigger stream: {}", e);
                let error_response = json!({
                    "success": false,
                    "message": format!("Failed to queue execution: {}", e)
                });
                Err((StatusCode::INTERNAL_SERVER_ERROR, Json(error_response)))
            }
        }
    }
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

/// Build scenario inputs from HTTP request data with pre-parsed body
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

/// Update the last_run timestamp on a trigger
async fn update_trigger_last_run(pool: &PgPool, trigger_id: &str) -> Result<(), sqlx::Error> {
    sqlx::query!(
        "UPDATE invocation_trigger SET last_run = NOW() WHERE id = $1",
        trigger_id
    )
    .execute(pool)
    .await?;
    Ok(())
}
