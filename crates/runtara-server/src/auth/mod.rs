pub mod jwks;
pub mod jwt_validator;
pub mod provider;
pub mod providers;

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use sqlx::PgPool;

pub use provider::{AuthError, AuthProvider, AuthProviderKind, AuthProviders};

/// JWT configuration consumed by `OidcProvider`. Parsed in the provider factory; other
/// modes ignore these fields entirely.
#[derive(Debug, Clone)]
pub struct JwtConfig {
    pub jwks_uri: String,
    pub issuer: String,
    pub audience: Option<String>,
}

/// Authentication context inserted into request extensions after successful auth.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthContext {
    pub org_id: String,
    pub user_id: String,
    pub auth_method: AuthMethod,
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
