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

        let jwks_cache = crate::auth::jwks::JwksCache::new(jwks_uri.clone()).await;
        crate::auth::jwks::JwksCache::spawn_refresh_task(jwks_cache.clone(), 3600);

        let api = Arc::new(OidcProvider::new(
            crate::auth::JwtConfig {
                jwks_uri: jwks_uri.clone(),
                issuer: issuer.clone(),
                audience: api_audience,
            },
            jwks_cache.clone(),
            tenant_id.clone(),
        )) as Arc<dyn AuthProvider>;

        let mcp = Arc::new(OidcProvider::new(
            crate::auth::JwtConfig {
                jwks_uri,
                issuer,
                audience: mcp_audience,
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
