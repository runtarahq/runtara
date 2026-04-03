use axum::{
    extract::{Request, State},
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Json, Response},
};
use serde_json::json;

use crate::auth::jwt_validator;
use crate::auth::{AuthContext, AuthMethod, AuthState};

/// Authentication middleware that validates JWT tokens or API keys.
///
/// For every request:
/// 1. Extracts `Authorization: Bearer <token>` header
/// 2. If token starts with `smo_` → validates via management API key endpoint
/// 3. Otherwise → decodes JWT, verifies signature via JWKS, validates claims
/// 4. Validates org_id against configured TENANT_ID (single-tenant)
/// 5. Inserts `AuthContext` into request extensions
pub async fn authenticate(
    State(auth_state): State<AuthState>,
    mut request: Request,
    next: Next,
) -> Response {
    // If AuthContext is already in extensions, this is a trusted in-process call
    // (e.g., from MCP tools via Router::oneshot). Skip validation.
    if request.extensions().get::<AuthContext>().is_some() {
        return next.run(request).await;
    }

    let auth_header = request
        .headers()
        .get("Authorization")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    let Some(auth_header) = auth_header else {
        return unauthorized("Missing Authorization header");
    };

    let token = auth_header.strip_prefix("Bearer ").unwrap_or(&auth_header);

    if token.is_empty() {
        return unauthorized("Empty bearer token");
    }

    // Route to API key or JWT validation
    let auth_context = if token.starts_with("smo_") {
        validate_api_key(token, &auth_state).await
    } else {
        validate_jwt(token, &auth_state).await
    };

    let auth_context = match auth_context {
        Ok(ctx) => ctx,
        Err(response) => return response,
    };

    // Validate org_id against configured TENANT_ID (single-tenant enforcement)
    let configured_tenant_id = crate::config::tenant_id();
    if auth_context.org_id != configured_tenant_id {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({
                "error": "Tenant mismatch",
                "message": format!("The provided tenant '{}' is not authorized for this runtime", auth_context.org_id)
            })),
        )
            .into_response();
    }

    // Insert AuthContext into extensions — handlers use OrgId extractor to read org_id
    request.extensions_mut().insert(auth_context);

    next.run(request).await
}

/// Validate an API key token via local database
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

/// Validate a JWT token
async fn validate_jwt(token: &str, auth_state: &AuthState) -> Result<AuthContext, Response> {
    // Extract kid from token header
    let kid = jwt_validator::extract_kid(token).map_err(|e| {
        tracing::debug!(error = %e, "JWT header extraction failed");
        unauthorized("Invalid or expired token")
    })?;

    // Look up the signing key
    let decoding_key = auth_state.jwks_cache.get_key(&kid).await.ok_or_else(|| {
        tracing::warn!(kid = %kid, "Unknown signing key");
        unauthorized("Invalid or expired token")
    })?;

    // Validate the token
    let claims = jwt_validator::validate_token(token, &decoding_key, &auth_state.jwt_config)
        .map_err(|e| {
            tracing::debug!(error = %e, "JWT validation failed");
            unauthorized("Invalid or expired token")
        })?;

    Ok(AuthContext {
        org_id: claims.org_id.expect("org_id validated in validate_token"),
        user_id: claims.sub.unwrap_or_default(),
        auth_method: AuthMethod::Jwt,
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
