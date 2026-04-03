//! In-process API client for MCP tools.
//!
//! Replaces HTTP self-calls with Router::oneshot() calls.
//! Pre-injects AuthContext into request extensions so the auth middleware
//! recognizes these as trusted internal calls and skips JWT validation.

use axum::body::Body;
use axum::http::{Method, Request};
use http_body_util::BodyExt;
use tower::ServiceExt;

use crate::auth::{AuthContext, AuthMethod};
use crate::mcp::server::SmoMcpServer;

fn err(msg: impl Into<String>) -> rmcp::ErrorData {
    rmcp::ErrorData::internal_error(msg.into(), None)
}

/// Validate that an ID is safe to interpolate into a URL path segment.
/// Rejects values containing `/`, `..`, `?`, `#`, or whitespace to prevent
/// path traversal and query injection via MCP tool parameters.
pub fn validate_path_param(name: &str, value: &str) -> Result<(), rmcp::ErrorData> {
    if value.is_empty() {
        return Err(rmcp::ErrorData::invalid_params(
            format!("{} must not be empty", name),
            None,
        ));
    }
    if value.contains('/')
        || value.contains("..")
        || value.contains('?')
        || value.contains('#')
        || value.contains(|c: char| c.is_whitespace())
    {
        return Err(rmcp::ErrorData::invalid_params(
            format!("{} contains invalid characters", name),
            None,
        ));
    }
    Ok(())
}

/// Build a request with AuthContext pre-injected.
fn build_request(
    method: Method,
    path: &str,
    body: Option<serde_json::Value>,
    tenant_id: &str,
) -> Request<Body> {
    let body = match body {
        Some(b) => Body::from(serde_json::to_vec(&b).unwrap_or_default()),
        None => Body::empty(),
    };

    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header("Content-Type", "application/json")
        .body(body)
        .expect("valid request");

    // Inject AuthContext so auth middleware skips validation
    request.extensions_mut().insert(AuthContext {
        org_id: tenant_id.to_string(),
        user_id: "mcp-internal".to_string(),
        auth_method: AuthMethod::Jwt,
    });

    request
}

/// Make an in-process GET request via the internal router.
pub async fn api_get(
    server: &SmoMcpServer,
    path: &str,
) -> Result<serde_json::Value, rmcp::ErrorData> {
    let request = build_request(Method::GET, path, None, &server.tenant_id);

    let response = server
        .internal_router
        .clone()
        .oneshot(request)
        .await
        .map_err(|e| err(format!("Internal request failed: {}", e)))?;

    let status = response.status();
    let body_bytes = response
        .into_body()
        .collect()
        .await
        .map_err(|e| err(format!("Failed to read response: {}", e)))?
        .to_bytes();

    let body: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap_or_else(|_| {
        let text = String::from_utf8_lossy(&body_bytes);
        serde_json::json!({ "error": text.to_string() })
    });

    if !status.is_success() {
        let msg = body
            .get("message")
            .or(body.get("error"))
            .and_then(|v| v.as_str())
            .unwrap_or("Unknown error");
        return Err(err(format!("API error ({}): {}", status, msg)));
    }

    Ok(body)
}

/// Make an in-process POST request via the internal router.
pub async fn api_post(
    server: &SmoMcpServer,
    path: &str,
    body: Option<serde_json::Value>,
) -> Result<serde_json::Value, rmcp::ErrorData> {
    let request = build_request(Method::POST, path, body, &server.tenant_id);

    let response = server
        .internal_router
        .clone()
        .oneshot(request)
        .await
        .map_err(|e| err(format!("Internal request failed: {}", e)))?;

    let status = response.status();
    let body_bytes = response
        .into_body()
        .collect()
        .await
        .map_err(|e| err(format!("Failed to read response: {}", e)))?
        .to_bytes();

    let body: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap_or_else(|_| {
        let text = String::from_utf8_lossy(&body_bytes);
        serde_json::json!({ "error": text.to_string() })
    });

    if !status.is_success() {
        let msg = body
            .get("message")
            .or(body.get("error"))
            .and_then(|v| v.as_str())
            .unwrap_or("Unknown error");
        return Err(err(format!("API error ({}): {}", status, msg)));
    }

    Ok(body)
}

/// Make an in-process PUT request via the internal router.
pub async fn api_put(
    server: &SmoMcpServer,
    path: &str,
    body: Option<serde_json::Value>,
) -> Result<serde_json::Value, rmcp::ErrorData> {
    let request = build_request(Method::PUT, path, body, &server.tenant_id);

    let response = server
        .internal_router
        .clone()
        .oneshot(request)
        .await
        .map_err(|e| err(format!("Internal request failed: {}", e)))?;

    let status = response.status();
    let body_bytes = response
        .into_body()
        .collect()
        .await
        .map_err(|e| err(format!("Failed to read response: {}", e)))?
        .to_bytes();

    let body: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap_or_else(|_| {
        let text = String::from_utf8_lossy(&body_bytes);
        serde_json::json!({ "error": text.to_string() })
    });

    if !status.is_success() {
        let msg = body
            .get("message")
            .or(body.get("error"))
            .and_then(|v| v.as_str())
            .unwrap_or("Unknown error");
        return Err(err(format!("API error ({}): {}", status, msg)));
    }

    Ok(body)
}

/// Make an in-process DELETE request via the internal router.
#[allow(dead_code)]
pub async fn api_delete(
    server: &SmoMcpServer,
    path: &str,
) -> Result<serde_json::Value, rmcp::ErrorData> {
    let request = build_request(Method::DELETE, path, None, &server.tenant_id);

    let response = server
        .internal_router
        .clone()
        .oneshot(request)
        .await
        .map_err(|e| err(format!("Internal request failed: {}", e)))?;

    let status = response.status();
    let body_bytes = response
        .into_body()
        .collect()
        .await
        .map_err(|e| err(format!("Failed to read response: {}", e)))?
        .to_bytes();

    if body_bytes.is_empty() {
        return Ok(serde_json::json!({"success": true}));
    }

    let body: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap_or_else(|_| {
        let text = String::from_utf8_lossy(&body_bytes);
        serde_json::json!({ "error": text.to_string() })
    });

    if !status.is_success() {
        let msg = body
            .get("message")
            .or(body.get("error"))
            .and_then(|v| v.as_str())
            .unwrap_or("Unknown error");
        return Err(err(format!("API error ({}): {}", status, msg)));
    }

    Ok(body)
}
