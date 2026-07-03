use std::sync::Arc;

use async_trait::async_trait;
use axum::{
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Json, Response},
};
use serde_json::json;

use crate::auth::AuthContext;

/// Which auth provider is in use. Also governs the bind-address safety check at startup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthProviderKind {
    /// Validates JWTs via OIDC discovery / JWKS. Default.
    Oidc,
    /// No authentication inside RUNTARA — single-user airgapped deploy. Requires loopback bind.
    Local,
    /// Trusts a reverse proxy that has already terminated auth. Requires loopback bind.
    TrustProxy,
}

impl AuthProviderKind {
    /// Providers that disable in-process auth and therefore require a loopback listener.
    pub fn requires_loopback(self) -> bool {
        matches!(self, AuthProviderKind::Local | AuthProviderKind::TrustProxy)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            AuthProviderKind::Oidc => "oidc",
            AuthProviderKind::Local => "local",
            AuthProviderKind::TrustProxy => "trust_proxy",
        }
    }
}

/// Errors an `AuthProvider` may return. Converts to a JSON response for the middleware.
#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("Missing Authorization header")]
    MissingToken,

    #[error("Empty bearer token")]
    EmptyToken,

    #[error("Invalid or expired token")]
    InvalidToken,

    #[error("Tenant '{0}' is not authorized for this runtime")]
    TenantMismatch(String),
}

impl AuthError {
    pub fn into_http_response(self) -> Response {
        let (status, error, message) = match &self {
            AuthError::MissingToken => (
                StatusCode::UNAUTHORIZED,
                "Unauthorized",
                "Missing Authorization header".to_string(),
            ),
            AuthError::EmptyToken => (
                StatusCode::UNAUTHORIZED,
                "Unauthorized",
                "Empty bearer token".to_string(),
            ),
            AuthError::InvalidToken => (
                StatusCode::UNAUTHORIZED,
                "Unauthorized",
                "Invalid or expired token".to_string(),
            ),
            AuthError::TenantMismatch(id) => (
                StatusCode::FORBIDDEN,
                "Tenant mismatch",
                format!("The provided tenant '{id}' is not authorized for this runtime"),
            ),
        };
        (status, Json(json!({ "error": error, "message": message }))).into_response()
    }
}

/// A pluggable auth backend. Implementations inspect the incoming request headers and
/// either return a validated `AuthContext` or an error; the surrounding middleware is
/// responsible for the in-process bypass and the API-key fast path.
#[async_trait]
pub trait AuthProvider: Send + Sync {
    async fn authenticate(&self, headers: &HeaderMap) -> Result<AuthContext, AuthError>;

    fn kind(&self) -> AuthProviderKind;
}

/// A pair of providers built from the environment: one for the public API and one for MCP.
///
/// In `oidc` mode the two providers may differ (separate audience validation for API vs MCP);
/// in `local` and `trust_proxy` modes both handles refer to the same `Arc`.
#[derive(Clone)]
pub struct AuthProviders {
    pub api: Arc<dyn AuthProvider>,
    pub mcp: Arc<dyn AuthProvider>,
    pub kind: AuthProviderKind,
}

impl AuthProviders {
    /// Read `AUTH_PROVIDER` and build the matching providers. Panics on unknown values or
    /// missing required env vars (OIDC mode); this mirrors the fail-fast behavior the
    /// server uses for other required configuration.
    pub async fn from_env(tenant_id: String) -> Self {
        let provider_name = std::env::var("AUTH_PROVIDER").unwrap_or_else(|_| "oidc".to_string());
        match provider_name.as_str() {
            "oidc" => Self::oidc_from_env(tenant_id).await,
            "local" => Self::local(tenant_id),
            "trust_proxy" | "trust-proxy" => Self::trust_proxy_from_env(tenant_id),
            other => panic!(
                "Unknown AUTH_PROVIDER value: '{other}'. \
                 Must be one of: oidc, local, trust_proxy"
            ),
        }
    }

    async fn oidc_from_env(tenant_id: String) -> Self {
        use crate::auth::providers::OidcProvider;

        let jwks_uri = std::env::var("OAUTH2_JWKS_URI").expect("OAUTH2_JWKS_URI must be set");
        let issuer = std::env::var("OAUTH2_ISSUER").expect("OAUTH2_ISSUER must be set");
        let api_audience = std::env::var("OAUTH2_AUDIENCE").ok();
        let mcp_audience = std::env::var("OAUTH2_MCP_AUDIENCE").ok();
        // Off during rollout before the Auth0 Action emits `jti`; flipped on once every
        // token carries it. See docs/security/user-management-contracts.md.
        let require_jti = std::env::var("RUNTARA_AUTH_REQUIRE_JTI")
            .map(|v| v == "true" || v == "1")
            .unwrap_or(false);
        // Opt-in strict MCP audience enforcement (SYN-523). Default OFF so existing
        // deployments keep today's lax behavior: audience validation is simply skipped
        // when OAUTH2_MCP_AUDIENCE is unset. When the flag is set, an unset audience is
        // a fatal misconfiguration — fail fast like the JWKS/issuer expect()s above.
        let require_mcp_audience = std::env::var("RUNTARA_MCP_REQUIRE_AUDIENCE")
            .map(|v| v == "true" || v == "1")
            .unwrap_or(false);
        if mcp_audience.is_none() {
            if require_mcp_audience {
                panic!(
                    "RUNTARA_MCP_REQUIRE_AUDIENCE is enabled but OAUTH2_MCP_AUDIENCE is not set. \
                     Set OAUTH2_MCP_AUDIENCE to the MCP resource audience (the token `aud` MCP \
                     clients request), or unset RUNTARA_MCP_REQUIRE_AUDIENCE to keep lax mode."
                );
            }
            tracing::warn!(
                "OAUTH2_MCP_AUDIENCE unset — MCP JWT audience validation disabled; \
                 any valid token from this issuer is accepted on /mcp"
            );
        }
        if api_audience.is_none() {
            tracing::warn!(
                "OAUTH2_AUDIENCE unset — API JWT audience validation disabled; \
                 any valid token from this issuer is accepted on the API"
            );
        }

        let jwks_cache = crate::auth::jwks::JwksCache::new(jwks_uri.clone()).await;
        crate::auth::jwks::JwksCache::spawn_refresh_task(jwks_cache.clone(), 3600);

        let api = Arc::new(OidcProvider::new(
            crate::auth::JwtConfig {
                jwks_uri: jwks_uri.clone(),
                issuer: issuer.clone(),
                audience: api_audience,
                require_jti,
                // The API path never requires `aud` to be present; SYN-522 strict mode
                // is MCP-only. Keep this byte-identical to pre-SYN-522 behavior.
                require_audience_present: false,
            },
            jwks_cache.clone(),
            tenant_id.clone(),
        )) as Arc<dyn AuthProvider>;

        let mcp = Arc::new(OidcProvider::new(
            crate::auth::JwtConfig {
                jwks_uri,
                issuer,
                audience: mcp_audience,
                require_jti,
                // Strict MCP posture (SYN-522): when RUNTARA_MCP_REQUIRE_AUDIENCE is on
                // (which already forced OAUTH2_MCP_AUDIENCE to be Some above), reject a
                // token that omits `aud`. Default off keeps today's lax pass-through.
                require_audience_present: require_mcp_audience,
            },
            jwks_cache,
            tenant_id,
        )) as Arc<dyn AuthProvider>;

        Self {
            api,
            mcp,
            kind: AuthProviderKind::Oidc,
        }
    }

    fn local(tenant_id: String) -> Self {
        use crate::auth::providers::LocalProvider;

        let provider = Arc::new(LocalProvider::new(tenant_id)) as Arc<dyn AuthProvider>;
        Self {
            api: provider.clone(),
            mcp: provider,
            kind: AuthProviderKind::Local,
        }
    }

    fn trust_proxy_from_env(tenant_id: String) -> Self {
        use crate::auth::providers::TrustProxyProvider;

        let header_name = std::env::var("TRUST_PROXY_USER_HEADER")
            .unwrap_or_else(|_| "x-forwarded-user".to_string());
        let provider =
            Arc::new(TrustProxyProvider::new(tenant_id, header_name)) as Arc<dyn AuthProvider>;
        Self {
            api: provider.clone(),
            mcp: provider,
            kind: AuthProviderKind::TrustProxy,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_env::{ENV_MUTEX, EnvGuard};

    #[tokio::test]
    async fn oidc_from_env_panics_when_strict_audience_flag_set_without_audience() {
        // SYN-523 strict mode: RUNTARA_MCP_REQUIRE_AUDIENCE=1 with no
        // OAUTH2_MCP_AUDIENCE must fail fast at startup. The panic fires
        // before the JWKS fetch, so dummy URIs never see the network.
        let _lock = ENV_MUTEX.lock().await;
        let mut guard = EnvGuard::new();
        guard.set("OAUTH2_JWKS_URI", "http://127.0.0.1:1/jwks.json");
        guard.set("OAUTH2_ISSUER", "http://127.0.0.1:1/");
        guard.set("RUNTARA_MCP_REQUIRE_AUDIENCE", "1");
        guard.remove("OAUTH2_MCP_AUDIENCE");

        let err = match tokio::spawn(AuthProviders::oidc_from_env("org_test".to_string())).await {
            Err(join_err) => join_err,
            Ok(_) => panic!("oidc_from_env must panic in strict mode"),
        };
        assert!(err.is_panic(), "expected a panic, got {err:?}");
        let payload = err.into_panic();
        let msg = payload
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| payload.downcast_ref::<String>().cloned())
            .expect("panic payload is a string");
        assert!(
            msg.contains("OAUTH2_MCP_AUDIENCE"),
            "panic message must name the missing var, got: {msg}"
        );
    }
}
