pub mod jwks;
pub mod jwt_validator;
pub mod provider;
pub mod providers;

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use sqlx::PgPool;

use crate::authz::Role;

pub use provider::{AuthError, AuthProvider, AuthProviderKind, AuthProviders};

/// JWT configuration consumed by `OidcProvider`. Parsed in the provider factory; other
/// modes ignore these fields entirely.
#[derive(Debug, Clone)]
pub struct JwtConfig {
    pub jwks_uri: String,
    pub issuer: String,
    pub audience: Option<String>,
    /// When true, a JWT without a `jti` claim is rejected. Off during rollout (Stage 0),
    /// flipped on once the Auth0 Action emits `jti` on every token (Stage 1). Driven by
    /// `RUNTARA_AUTH_REQUIRE_JTI`. See `docs/security/user-management-contracts.md`.
    pub require_jti: bool,
}

/// Authentication context inserted into request extensions after successful auth.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthContext {
    pub org_id: String,
    pub user_id: String,
    pub auth_method: AuthMethod,
    /// Caller's role in this tenant. `None` outside SaaS enforcement — non-JWT modes,
    /// trusted internal calls, and the rollout transition before the Valkey membership
    /// lookup (Phase 1.7) populates it. JWTs never carry the role; it is read from the
    /// per-tenant Valkey `member:{sub}` entry.
    pub role: Option<Role>,
    /// Token identity (`jti` claim / API-key token id). Key for the revocation denylist.
    pub jti: Option<String>,
    /// Identity claims passed through from the JWT, for logging and the `/me` response.
    pub email: Option<String>,
    pub name: Option<String>,
    /// Human-readable tenant identifier. Logging and `/me` only — `org_id` is the tenant key.
    pub tenant_slug: Option<String>,
}

impl AuthContext {
    /// Construct an `AuthContext` with no enriched identity (no role, jti, email, name, or
    /// tenant_slug). Used by non-JWT auth paths and trusted internal callers; the JWT path
    /// builds the struct directly so it can thread the token claims.
    pub fn new(org_id: String, user_id: String, auth_method: AuthMethod) -> Self {
        Self {
            org_id,
            user_id,
            auth_method,
            role: None,
            jti: None,
            email: None,
            name: None,
            tenant_slug: None,
        }
    }
}

/// How the request was authenticated.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum AuthMethod {
    Jwt,
    ApiKey,
    /// No per-request auth was performed inside RUNTARA — request was either trusted
    /// unconditionally (`local`) or trusted because it arrived through a reverse proxy
    /// that terminated auth upstream (`trust_proxy`).
    Unauthenticated,
}

/// Shared authentication state passed to the middleware. The middleware handles the
/// in-process bypass and the API-key fast path, then defers to `provider` for
/// everything else.
#[derive(Clone)]
pub struct AuthState {
    pub provider: Arc<dyn AuthProvider>,
    pub pool: PgPool,
}
