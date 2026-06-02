use std::sync::Arc;

use async_trait::async_trait;
use axum::http::HeaderMap;

use crate::auth::{
    AuthContext, AuthMethod, JwtConfig,
    jwks::JwksCache,
    jwt_validator,
    provider::{AuthError, AuthProvider, AuthProviderKind},
};

/// Validates incoming JWTs against a JWKS, issuer, and optional audience, then
/// enforces that the `org_id` claim matches the configured single-tenant ID.
pub struct OidcProvider {
    jwt_config: JwtConfig,
    jwks_cache: Arc<JwksCache>,
    tenant_id: String,
}

impl OidcProvider {
    pub fn new(jwt_config: JwtConfig, jwks_cache: Arc<JwksCache>, tenant_id: String) -> Self {
        Self {
            jwt_config,
            jwks_cache,
            tenant_id,
        }
    }
}

#[async_trait]
impl AuthProvider for OidcProvider {
    async fn authenticate(&self, headers: &HeaderMap) -> Result<AuthContext, AuthError> {
        let auth_header = headers
            .get("Authorization")
            .and_then(|v| v.to_str().ok())
            .ok_or(AuthError::MissingToken)?;

        let token = auth_header.strip_prefix("Bearer ").unwrap_or(auth_header);
        if token.is_empty() {
            return Err(AuthError::EmptyToken);
        }

        let kid = jwt_validator::extract_kid(token).map_err(|e| {
            tracing::debug!(error = %e, "JWT header extraction failed");
            AuthError::InvalidToken
        })?;

        let decoding_key = self.jwks_cache.get_key(&kid).await.ok_or_else(|| {
            tracing::warn!(kid = %kid, "Unknown signing key");
            AuthError::InvalidToken
        })?;

        let claims = jwt_validator::validate_token(token, &decoding_key, &self.jwt_config)
            .map_err(|e| {
                tracing::debug!(error = %e, "JWT validation failed");
                AuthError::InvalidToken
            })?;

        let org_id = claims
            .org_id
            .expect("org_id presence validated in validate_token");

        if org_id != self.tenant_id {
            return Err(AuthError::TenantMismatch(org_id));
        }

        Ok(AuthContext {
            org_id,
            user_id: claims.sub.unwrap_or_default(),
            auth_method: AuthMethod::Jwt,
            // Role is read from the tenant's Valkey, not the JWT — populated in Phase 1.7.
            role: None,
            jti: claims.jti,
            email: claims.email,
            name: claims.name,
            tenant_slug: claims.tenant_slug,
        })
    }

    fn kind(&self) -> AuthProviderKind {
        AuthProviderKind::Oidc
    }
}
