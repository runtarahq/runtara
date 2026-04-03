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
}
