use sqlx::PgPool;

/// Configuration for the connections crate.
///
/// The host application builds this from its own environment / settings
/// and passes it to the router factory functions.
#[derive(Clone)]
pub struct ConnectionsConfig {
    /// PostgreSQL connection pool.
    pub db_pool: PgPool,

    /// Redis/Valkey URL for rate-limit state storage.
    /// `None` disables real-time rate-limit tracking (graceful degradation).
    pub redis_url: Option<String>,

    /// Public base URL used to construct OAuth2 redirect URIs.
    /// Example: `"https://api.example.com"`
    pub public_base_url: String,

    /// Shared HTTP client for outbound requests (OAuth token exchange, etc.).
    pub http_client: reqwest::Client,
}

/// Runtime state shared across all handlers in the connections crate.
///
/// Built from [`ConnectionsConfig`] and used as Axum router state.
/// Handlers extract specific fields via [`axum::extract::FromRef`].
#[derive(Clone)]
pub struct ConnectionsState {
    pub db_pool: PgPool,
    pub redis_url: Option<String>,
    pub public_base_url: String,
    pub http_client: reqwest::Client,
}

impl ConnectionsState {
    pub fn from_config(config: ConnectionsConfig) -> Self {
        Self {
            db_pool: config.db_pool,
            redis_url: config.redis_url,
            public_base_url: config.public_base_url,
            http_client: config.http_client,
        }
    }
}

impl axum::extract::FromRef<ConnectionsState> for PgPool {
    fn from_ref(state: &ConnectionsState) -> PgPool {
        state.db_pool.clone()
    }
}
