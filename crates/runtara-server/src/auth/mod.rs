pub mod jwks;
pub mod jwt_validator;

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use sqlx::PgPool;

use crate::auth::jwks::JwksCache;

/// JWT configuration parsed from environment variables
#[derive(Debug, Clone)]
pub struct JwtConfig {
    pub jwks_uri: String,
    pub issuer: String,
    pub audience: Option<String>,
}

impl JwtConfig {
    /// Parse base JWT config from environment variables.
    /// Returns (api_config, mcp_config) with separate audiences.
    /// - `OAUTH2_AUDIENCE` → API routes
    /// - `OAUTH2_MCP_AUDIENCE` → MCP routes
    pub fn from_env() -> (Self, Self) {
        let jwks_uri = std::env::var("OAUTH2_JWKS_URI").expect("OAUTH2_JWKS_URI must be set");
        let issuer = std::env::var("OAUTH2_ISSUER").expect("OAUTH2_ISSUER must be set");
        let api_audience = std::env::var("OAUTH2_AUDIENCE").ok();
        let mcp_audience = std::env::var("OAUTH2_MCP_AUDIENCE").ok();

        let api_config = Self {
            jwks_uri: jwks_uri.clone(),
            issuer: issuer.clone(),
            audience: api_audience,
        };

        let mcp_config = Self {
            jwks_uri,
            issuer,
            audience: mcp_audience,
        };

        (api_config, mcp_config)
    }
}

/// Authentication context inserted into request extensions after successful auth
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthContext {
    pub org_id: String,
    pub user_id: String,
    pub auth_method: AuthMethod,
}

/// How the request was authenticated
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum AuthMethod {
    Jwt,
    ApiKey,
}

/// Shared authentication state passed to middleware
#[derive(Clone)]
pub struct AuthState {
    pub jwks_cache: Arc<JwksCache>,
    pub jwt_config: JwtConfig,
    pub pool: PgPool,
}
