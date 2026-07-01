//! Internal presigned URL handler.
//!
//! Generates time-limited URLs for object storage providers without exposing
//! the underlying credentials to the WASM agent. The agent sends a request
//! describing the operation (connection, method, path, expiry); the proxy
//! signs an absolute URL and returns it.
//!
//! Mounted at `POST /api/internal/presign` without authentication middleware —
//! the tenant_id is passed via the `X-Org-Id` header without JWT validation
//! (same pattern as `internal_proxy`).

use axum::{extract::State, http::StatusCode, response::Json};
use runtara_connections::auth::{aws_presign, azure_sas};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::Arc;

use super::internal_proxy::ProxyState;

#[derive(Debug, Deserialize)]
pub struct PresignRequest {
    pub connection_id: String,
    /// HTTP method the resulting URL will be used with (e.g. GET, PUT, DELETE).
    pub method: String,
    /// Path under the connection's base URL — e.g. `/bucket/key` or `/container/blob`.
    pub path: String,
    /// Lifetime of the signed URL in seconds. Clamped to 7 days.
    pub expires_in_seconds: u32,
    /// Optional content-type binding for upload presigns.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct PresignResponse {
    pub url: String,
    pub expires_in_seconds: u32,
}

/// POST /api/internal/presign
pub async fn presign_handler(
    headers: axum::http::HeaderMap,
    State(state): State<Arc<ProxyState>>,
    Json(request): Json<PresignRequest>,
) -> Result<(StatusCode, Json<PresignResponse>), (StatusCode, Json<Value>)> {
    let tenant_id = extract_tenant_id(&headers)?;

    let conn = state
        .facade
        .get_with_parameters(&request.connection_id, &tenant_id)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": format!("Database error fetching connection: {}", e)})),
            )
        })?
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                Json(json!({
                    "error": format!("Connection '{}' not found", request.connection_id)
                })),
            )
        })?;

    let integration_id = conn.integration_id.as_deref().unwrap_or("");
    let params = conn
        .connection_parameters
        .as_ref()
        .cloned()
        .unwrap_or(json!({}));

    // Resolve auth so we can pull credentials + base URL. Reuse the existing
    // resolution path so OAuth tokens etc. are populated where applicable —
    // even though we only consume aws_signing / azure_signing here.
    let mut headers_sink = std::collections::HashMap::new();
    let resolved = state
        .facade
        .resolve_connection_auth(
            &request.connection_id,
            integration_id,
            &params,
            &mut headers_sink,
        )
        .await
        .map_err(|e| {
            (
                StatusCode::BAD_GATEWAY,
                Json(json!({"error": format!("Credential resolution failed: {}", e)})),
            )
        })?;

    let base_url = resolved.base_url.ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "Connection has no base URL to presign against"})),
        )
    })?;

    let absolute = build_absolute_url(&base_url, &request.path).map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": format!("Invalid URL: {}", e)})),
        )
    })?;

    let signed = if let Some(aws) = resolved.aws_signing.as_ref() {
        let url = url::Url::parse(&absolute).map_err(|e| {
            (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": format!("Invalid URL for SigV4 presign: {}", e)})),
            )
        })?;
        aws_presign::presign_url_v4(
            &request.method,
            &url,
            request.expires_in_seconds,
            &aws.access_key_id,
            &aws.secret_access_key,
            &aws.region,
            &aws.service,
            aws.session_token.as_deref(),
        )
    } else if let Some(azure) = resolved.azure_signing.as_ref() {
        let (container, blob) = split_container_blob(&request.path)
            .map_err(|e| (StatusCode::BAD_REQUEST, Json(json!({"error": e}))))?;
        let permissions = azure_sas::permissions_for(&request.method)
            .or_else(|| match request.method.to_uppercase().as_str() {
                "GET" | "HEAD" => Some("r"),
                "PUT" | "POST" => Some("cw"),
                "DELETE" => Some("d"),
                _ => None,
            })
            .ok_or_else(|| {
                (
                    StatusCode::BAD_REQUEST,
                    Json(json!({
                        "error": format!(
                            "Unsupported method/operation for Azure SAS: {}",
                            request.method
                        )
                    })),
                )
            })?;

        azure_sas::generate_blob_sas_url(
            &base_url,
            &azure.account_name,
            &azure.account_key,
            container,
            blob,
            permissions,
            request.expires_in_seconds,
            request.content_type.as_deref(),
        )
        .map_err(|e| {
            (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": format!("Azure SAS generation failed: {}", e)})),
            )
        })?
    } else {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({
                "error": format!(
                    "Integration '{}' does not support presigned URLs",
                    integration_id
                )
            })),
        ));
    };

    let clamped_expires = request
        .expires_in_seconds
        .min(aws_presign::MAX_PRESIGN_EXPIRES_SECONDS);

    Ok((
        StatusCode::OK,
        Json(PresignResponse {
            url: signed,
            expires_in_seconds: clamped_expires,
        }),
    ))
}

fn extract_tenant_id(headers: &axum::http::HeaderMap) -> Result<String, (StatusCode, Json<Value>)> {
    headers
        .get("X-Org-Id")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
        .ok_or_else(|| {
            (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": "Missing X-Org-Id header"})),
            )
        })
}

fn build_absolute_url(base_url: &str, path: &str) -> Result<String, String> {
    // Reject caller-supplied absolute URLs: a presign request must stay on the
    // connection's host. Previously an absolute `path` was returned verbatim,
    // letting a caller presign against an arbitrary host with the connection's
    // signing credentials.
    if path.starts_with("http://") || path.starts_with("https://") {
        return Err("presign path must be relative to the connection base; \
                    absolute URLs are not allowed"
            .to_string());
    }
    let cleaned_base = base_url.trim_end_matches('/');
    let absolute = if path.starts_with('/') {
        format!("{cleaned_base}{path}")
    } else {
        format!("{cleaned_base}/{path}")
    };

    // Enforce that the joined path stays under the connection base path.
    let base_parsed = url::Url::parse(base_url).map_err(|e| format!("invalid base URL: {e}"))?;
    let final_parsed =
        url::Url::parse(&absolute).map_err(|e| format!("invalid resolved URL: {e}"))?;
    if final_parsed.host_str() != base_parsed.host_str() {
        return Err(
            "presign path resolves to a different host than the connection base".to_string(),
        );
    }
    if !crate::api::handlers::proxy_url::path_within_base(final_parsed.path(), base_parsed.path()) {
        return Err("presign path is outside the connection base path".to_string());
    }
    Ok(absolute)
}

fn split_container_blob(path: &str) -> Result<(&str, &str), String> {
    let trimmed = path.trim_start_matches('/');
    let (container, blob) = trimmed
        .split_once('/')
        .ok_or_else(|| "Path must be in the form `/container/blob`".to_string())?;
    if container.is_empty() || blob.is_empty() {
        return Err("Both container and blob are required for Azure presign".to_string());
    }
    Ok((container, blob))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_absolute_joins_relative_paths() {
        assert_eq!(
            build_absolute_url("https://acct.blob.core.windows.net", "/c/b").unwrap(),
            "https://acct.blob.core.windows.net/c/b"
        );
        assert_eq!(
            build_absolute_url("https://acct.blob.core.windows.net/", "c/b").unwrap(),
            "https://acct.blob.core.windows.net/c/b"
        );
        // Path-style base with an account prefix: child stays under it.
        assert_eq!(
            build_absolute_url("http://127.0.0.1:10000/acct", "/container/blob").unwrap(),
            "http://127.0.0.1:10000/acct/container/blob"
        );
    }

    #[test]
    fn build_absolute_rejects_absolute_url_and_escapes() {
        // Absolute caller URL must not pass through (the F1/F3 presign hole).
        assert!(build_absolute_url("https://x", "https://other/foo").is_err());
        assert!(build_absolute_url("https://x", "http://other/foo").is_err());
        // Path that climbs above the connection base path is rejected. The
        // path is appended under the base, so an escape needs enough `..` to
        // leave it: base "/v2/tenantA" + "/../../tenantB/x" → "/tenantB/x".
        assert!(
            build_absolute_url("https://api.example.com/v2/tenantA", "/../../tenantB/x").is_err()
        );
        assert!(build_absolute_url("http://127.0.0.1:10000/acct", "/../other/blob").is_err());
    }

    #[test]
    fn split_container_blob_requires_both_parts() {
        assert_eq!(split_container_blob("/c/b").unwrap(), ("c", "b"));
        assert_eq!(
            split_container_blob("/c/nested/blob.txt").unwrap(),
            ("c", "nested/blob.txt")
        );
        assert!(split_container_blob("/onlyone").is_err());
        assert!(split_container_blob("/").is_err());
    }
}
