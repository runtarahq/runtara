use axum::{
    extract::{Request, State},
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Json, Response},
};
use serde_json::json;

use crate::auth::{AuthContext, AuthMethod, AuthState};

/// Authentication middleware. Defers to the configured `AuthProvider` for everything
/// except the in-process bypass and the RUNTARA-issued API-key fast path.
///
/// For every request:
/// 1. If an `AuthContext` is already in extensions, pass through (trusted in-process
///    call, e.g. from MCP tools via `Router::oneshot`).
/// 2. If `Authorization: Bearer rt_*|smo_*`, validate via the local API-key table.
///    API keys work regardless of `AUTH_PROVIDER` — they are an operator escape hatch.
/// 3. Otherwise, call `auth_state.provider.authenticate(headers)` and use its result.
///
/// The tenant-mismatch check that used to live here now sits inside `OidcProvider`;
/// `LocalProvider` and `TrustProxyProvider` set `org_id` from the configured tenant by
/// construction, so there is nothing to mismatch.
pub async fn authenticate(
    State(auth_state): State<AuthState>,
    mut request: Request,
    next: Next,
) -> Response {
    if request.extensions().get::<AuthContext>().is_some() {
        return next.run(request).await;
    }

    // Fast path: RUNTARA-issued API key. Works in every provider mode.
    if let Some(token) = api_key_token(request.headers()) {
        let auth_context = match validate_api_key(token, &auth_state).await {
            Ok(ctx) => ctx,
            Err(response) => return response,
        };
        request.extensions_mut().insert(auth_context);
        return next.run(request).await;
    }

    // Delegate everything else to the configured provider.
    let auth_context = match auth_state.provider.authenticate(request.headers()).await {
        Ok(ctx) => ctx,
        Err(e) => return e.into_http_response(),
    };

    request.extensions_mut().insert(auth_context);
    next.run(request).await
}

/// Extract a Bearer token from the `Authorization` header if and only if it looks like
/// a RUNTARA-issued API key. Returns `None` for JWTs, missing headers, or empty tokens
/// so the provider path handles them.
fn api_key_token(headers: &axum::http::HeaderMap) -> Option<&str> {
    let value = headers.get("Authorization")?.to_str().ok()?;
    let token = value.strip_prefix("Bearer ").unwrap_or(value);
    if token.is_empty() {
        return None;
    }
    if token.starts_with("rt_") || token.starts_with("smo_") {
        Some(token)
    } else {
        None
    }
}

/// Validate an API key token via the local database.
async fn validate_api_key(token: &str, auth_state: &AuthState) -> Result<AuthContext, Response> {
    use sha2::Digest;
    let key_hash = hex::encode(sha2::Sha256::digest(token.as_bytes()));

    let api_key =
        crate::api::handlers::api_keys::validate_api_key_by_hash(&auth_state.pool, &key_hash)
            .await
            .map_err(|e| {
                tracing::warn!(error = %e, "API key validation failed");
                unauthorized("Invalid or expired API key")
            })?;

    Ok(AuthContext {
        org_id: api_key.org_id,
        user_id: "api-key".to_string(),
        auth_method: AuthMethod::ApiKey,
    })
}

fn unauthorized(message: &str) -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(json!({
            "error": "Unauthorized",
            "message": message
        })),
    )
        .into_response()
}
