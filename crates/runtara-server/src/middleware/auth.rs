use axum::{
    extract::{Request, State},
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Json, Response},
};
use serde_json::json;
use tracing::Instrument;

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
    if let Some(ctx) = request.extensions().get::<AuthContext>().cloned() {
        return next.run(request).instrument(auth_span(&ctx)).await;
    }

    // Fast path: RUNTARA-issued API key. Works in every provider mode.
    if let Some(token) = api_key_token(request.headers()) {
        let auth_context = match validate_api_key(token, &auth_state).await {
            Ok(ctx) => ctx,
            Err(response) => return response,
        };
        let span = auth_span(&auth_context);
        request.extensions_mut().insert(auth_context);
        return next.run(request).instrument(span).await;
    }

    // Delegate everything else to the configured provider.
    let auth_context = match auth_state.provider.authenticate(request.headers()).await {
        Ok(ctx) => ctx,
        Err(e) => return e.into_http_response(),
    };

    let span = auth_span(&auth_context);
    request.extensions_mut().insert(auth_context);
    next.run(request).instrument(span).await
}

/// Span carrying the resolved auth identity, used to wrap the rest of the
/// request future so any downstream `tracing` event inherits `user_id` and
/// `auth_method` fields. Subscribers that flatten parent-span fields onto
/// each emitted event (JSON formatter, OTLP exporter) surface these
/// alongside entitlement-denial warns, satisfying the Phase 6 audit
/// requirement that denial logs identify the caller without per-line
/// plumbing through every gate.
fn auth_span(ctx: &AuthContext) -> tracing::Span {
    tracing::info_span!(
        "request_auth",
        user_id = %ctx.user_id,
        auth_method = ?ctx.auth_method,
    )
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

    Ok(AuthContext::new(
        api_key.org_id,
        "api-key".to_string(),
        AuthMethod::ApiKey,
    ))
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
