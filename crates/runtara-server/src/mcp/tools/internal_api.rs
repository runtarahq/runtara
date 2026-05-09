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

/// JSON Schema for arbitrary object-shaped MCP arguments that are stored as
/// `serde_json::Value` at runtime so stringified client payloads can be recovered.
pub fn json_object_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    schemars::json_schema!({
        "type": "object",
        "additionalProperties": true
    })
}

/// JSON Schema for canonical workflow execution inputs.
pub fn workflow_inputs_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    schemars::json_schema!({
        "type": "object",
        "properties": {
            "data": {
                "description": "Workflow input payload. May be any JSON value."
            },
            "variables": {
                "type": "object",
                "additionalProperties": true,
                "description": "Workflow variables keyed by variable name."
            }
        },
        "required": ["data"],
        "additionalProperties": true
    })
}

/// JSON Schema for raw sync execution request bodies.
pub fn any_json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    schemars::json_schema!({
        "oneOf": [
            {"type": "object", "additionalProperties": true},
            {"type": "array"},
            {"type": "string"},
            {"type": "number"},
            {"type": "boolean"},
            {"type": "null"}
        ]
    })
}

/// Some MCP clients/LLMs serialize large object arguments as JSON-encoded strings
/// instead of nested objects. Accept both shapes so callers can still complete
/// deploy -> execute -> inspect loops through the same MCP server.
pub fn normalize_json_arg(
    value: serde_json::Value,
    field: &str,
) -> Result<serde_json::Value, rmcp::ErrorData> {
    match value {
        serde_json::Value::String(s) => serde_json::from_str(&s).map_err(|e| {
            rmcp::ErrorData::invalid_params(
                format!(
                    "{} was passed as a JSON-encoded string but is not valid JSON: {}. Some MCP clients stringify object arguments; pass {} as a JSON object when possible.",
                    field, e, field
                ),
                None,
            )
        }),
        other => Ok(other),
    }
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

/// Validate an identifier that is sent in a JSON body or compared in-memory,
/// not interpolated as a raw URL path segment. Runtime signal/action IDs can
/// contain path separators because they are deterministic checkpoint IDs.
pub fn validate_identifier_param(name: &str, value: &str) -> Result<(), rmcp::ErrorData> {
    if value.trim().is_empty() {
        return Err(rmcp::ErrorData::invalid_params(
            format!("{} must not be empty", name),
            None,
        ));
    }
    if value.contains(|c: char| c.is_control()) {
        return Err(rmcp::ErrorData::invalid_params(
            format!("{} contains invalid control characters", name),
            None,
        ));
    }
    Ok(())
}

/// Percent-encode a single URL path segment.
pub fn encode_path_param(value: &str) -> String {
    urlencoding::encode(value).into_owned()
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

/// Make an in-process PATCH request via the internal router.
pub async fn api_patch(
    server: &SmoMcpServer,
    path: &str,
    body: Option<serde_json::Value>,
) -> Result<serde_json::Value, rmcp::ErrorData> {
    let request = build_request(Method::PATCH, path, body, &server.tenant_id);

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

/// Make an in-process DELETE request with a JSON body via the internal router.
pub async fn api_delete_with_body(
    server: &SmoMcpServer,
    path: &str,
    body: Option<serde_json::Value>,
) -> Result<serde_json::Value, rmcp::ErrorData> {
    let request = build_request(Method::DELETE, path, body, &server.tenant_id);

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

#[cfg(test)]
mod tests {
    use super::{
        encode_path_param, normalize_json_arg, validate_identifier_param, validate_path_param,
    };

    #[test]
    fn identifier_param_allows_canonical_runtime_signal_ids() {
        let id = "00000000-0000-0000-0000-000000000001/00000000-0000-0000-0000-000000000002::00000000-0000-0000-0000-000000000003/review_step";

        assert!(validate_identifier_param("signal_id", id).is_ok());
    }

    #[test]
    fn path_param_still_rejects_unencoded_slashes() {
        let id = "00000000-0000-0000-0000-000000000001/root/review_step";

        assert!(validate_path_param("signal_id", id).is_err());
    }

    #[test]
    fn identifier_param_rejects_empty_or_control_values() {
        assert!(validate_identifier_param("signal_id", "").is_err());
        assert!(validate_identifier_param("signal_id", " \t ").is_err());
        assert!(validate_identifier_param("signal_id", "review\nstep").is_err());
    }

    #[test]
    fn encode_path_param_escapes_canonical_action_ids() {
        assert_eq!(
            encode_path_param("instance/workflow::version/review_step"),
            "instance%2Fworkflow%3A%3Aversion%2Freview_step"
        );
    }

    #[test]
    fn normalize_json_arg_passes_object_through() {
        let v = serde_json::json!({"name": "foo", "steps": {}});
        let normalized = normalize_json_arg(v.clone(), "execution_graph").unwrap();
        assert_eq!(normalized, v);
    }

    #[test]
    fn normalize_json_arg_parses_stringified_object() {
        let original = serde_json::json!({"name": "foo", "n": 42});
        let stringified = serde_json::Value::String(original.to_string());
        let normalized = normalize_json_arg(stringified, "execution_graph").unwrap();
        assert_eq!(normalized, original);
    }

    #[test]
    fn normalize_json_arg_rejects_invalid_json_string_with_hint() {
        let bad = serde_json::Value::String("{not valid json".to_string());
        let err = normalize_json_arg(bad, "inputs").unwrap_err();
        let msg = err.message.to_string();
        assert!(
            msg.contains("inputs")
                && msg.contains("not valid JSON")
                && msg.contains("Some MCP clients stringify object arguments"),
            "got: {msg}"
        );
    }
}
