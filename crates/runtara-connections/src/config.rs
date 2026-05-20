use std::sync::Arc;

use sqlx::PgPool;

use crate::crypto::CredentialCipher;
use crate::integration_compatibility::IntegrationCompatibility;

/// Configuration for the connections crate.
///
/// The host application builds this from its own environment / settings
/// and passes it to the router factory functions.
#[derive(Clone)]
pub struct ConnectionsConfig {
    /// PostgreSQL connection pool.
    pub db_pool: PgPool,

    /// Shared Redis/Valkey connection manager for rate-limit state storage.
    /// `None` disables real-time rate-limit tracking (graceful degradation).
    ///
    /// Built once by the host at startup (`redis::aio::ConnectionManager::new`)
    /// and shared across all handlers. Cheap to clone — internally an `Arc`.
    pub redis_manager: Option<redis::aio::ConnectionManager>,

    /// Public base URL used to construct OAuth2 redirect URIs.
    /// Example: `"https://api.example.com"`
    pub public_base_url: String,

    /// Shared HTTP client for outbound requests (OAuth token exchange, etc.).
    pub http_client: reqwest::Client,

    /// Cipher for at-rest encryption of `connection_parameters`.
    ///
    /// Typically constructed via [`crate::crypto::cipher_from_env`]. Use
    /// [`crate::crypto::noop::NoOpCipher`] for local development only.
    pub cipher: Arc<dyn CredentialCipher>,

    /// Maps each `default_for` bucket (agent id, or virtual platform
    /// bucket like `object_storage`) to the integration ids that satisfy
    /// it. Built by the host from the runtime [`AgentCatalog`] at boot.
    pub compatibility: Arc<IntegrationCompatibility>,

    /// Runtime agent catalog. Handlers translate `(agent id) →
    /// integration ids` here at request time using
    /// [`runtara_dsl::agent_meta::AgentCatalog::integration_ids_for`];
    /// the connection service itself stays agent-agnostic.
    pub agent_catalog: Arc<runtara_dsl::agent_meta::AgentCatalog>,
}

/// Runtime state shared across all handlers in the connections crate.
///
/// Built from [`ConnectionsConfig`] and used as Axum router state.
/// Handlers extract specific fields via [`axum::extract::FromRef`].
#[derive(Clone)]
pub struct ConnectionsState {
    pub db_pool: PgPool,
    pub redis_manager: Option<redis::aio::ConnectionManager>,
    pub public_base_url: String,
    pub http_client: reqwest::Client,
    pub cipher: Arc<dyn CredentialCipher>,
    pub compatibility: Arc<IntegrationCompatibility>,
    pub agent_catalog: Arc<runtara_dsl::agent_meta::AgentCatalog>,
}

impl ConnectionsState {
    pub fn from_config(config: ConnectionsConfig) -> Self {
        Self {
            db_pool: config.db_pool,
            redis_manager: config.redis_manager,
            public_base_url: config.public_base_url,
            http_client: config.http_client,
            cipher: config.cipher,
            compatibility: config.compatibility,
            agent_catalog: config.agent_catalog,
        }
    }
}

impl axum::extract::FromRef<ConnectionsState> for PgPool {
    fn from_ref(state: &ConnectionsState) -> PgPool {
        state.db_pool.clone()
    }
}

impl axum::extract::FromRef<ConnectionsState> for Arc<dyn CredentialCipher> {
    fn from_ref(state: &ConnectionsState) -> Arc<dyn CredentialCipher> {
        state.cipher.clone()
    }
}

impl axum::extract::FromRef<ConnectionsState> for Arc<IntegrationCompatibility> {
    fn from_ref(state: &ConnectionsState) -> Arc<IntegrationCompatibility> {
        state.compatibility.clone()
    }
}

impl axum::extract::FromRef<ConnectionsState> for Arc<runtara_dsl::agent_meta::AgentCatalog> {
    fn from_ref(state: &ConnectionsState) -> Arc<runtara_dsl::agent_meta::AgentCatalog> {
        state.agent_catalog.clone()
    }
}
