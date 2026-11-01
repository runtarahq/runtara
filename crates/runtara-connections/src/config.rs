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

    /// The console's OAuth landing page (`oauth-complete.html`), where the
    /// OAuth callback sends the consent popup once it is done. The console may
    /// be on another host than this API. `None` when there is no console: the
    /// callback then answers with a plain-text outcome.
    pub oauth_complete_url: Option<url::Url>,

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

    /// Optional sink for connection lifecycle analytics events. `None` disables them. The
    /// host implements [`crate::events::ConnectionEventSink`] and injects it here; the crate
    /// stays unaware of the host's product-events pipeline. See [`crate::events`].
    #[allow(clippy::type_complexity)]
    pub connection_events: crate::events::ConnectionEvents,

    /// How OAuth authorization-code flows are started and completed.
    pub oauth: OAuthOptions,
}

/// The work a request leaves to the host, as a boxed future.
pub type HostFuture =
    std::pin::Pin<Box<dyn std::future::Future<Output = axum::response::Response> + Send + 'static>>;

/// What the host knows about the tenants this process serves, for the OAuth
/// callback that names no tenant: it learns the tenant from the flow, then
/// asks whether it is served here and runs the rest in that tenant's scope.
pub trait TenantHost: Send + Sync {
    /// Whether this process serves `tenant`.
    fn serves(&self, tenant: &str) -> bool;
    /// `work`, run in `tenant`'s scope (for example, row-level security).
    fn scoped(&self, tenant: String, work: HostFuture) -> HostFuture;
}

/// A host that serves every tenant and has no scope to enter.
pub struct EveryTenant;

impl TenantHost for EveryTenant {
    fn serves(&self, _tenant: &str) -> bool {
        true
    }

    fn scoped(&self, _tenant: String, work: HostFuture) -> HostFuture {
        work
    }
}

/// How OAuth authorization-code flows run.
#[derive(Clone)]
pub struct OAuthOptions {
    /// Serve the callback that names no tenant (`/api/oauth/callback`), and
    /// give new connections that redirect URI. Off: new connections keep the
    /// tenant's own callback (`/api/oauth/{tenant}/callback`).
    pub tenantless_callback: bool,
    /// Bind flows started in the console to the browser by a cookie. Off:
    /// they need a member's confirmation, like flows started without a
    /// browser (for a console on another origin than the callback).
    pub browser_binding: bool,
    /// The tenants this process serves, for the tenant-less callback.
    pub tenants: Arc<dyn TenantHost>,
}

impl Default for OAuthOptions {
    fn default() -> Self {
        Self {
            tenantless_callback: true,
            browser_binding: true,
            tenants: Arc::new(EveryTenant),
        }
    }
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
    pub oauth_complete_url: Option<url::Url>,
    pub http_client: reqwest::Client,
    pub cipher: Arc<dyn CredentialCipher>,
    pub compatibility: Arc<IntegrationCompatibility>,
    pub agent_catalog: Arc<runtara_dsl::agent_meta::AgentCatalog>,
    pub connection_events: crate::events::ConnectionEvents,
    pub oauth: OAuthOptions,
}

impl ConnectionsState {
    pub fn from_config(config: ConnectionsConfig) -> Self {
        Self {
            db_pool: config.db_pool,
            redis_manager: config.redis_manager,
            public_base_url: config.public_base_url,
            oauth_complete_url: config.oauth_complete_url,
            http_client: config.http_client,
            cipher: config.cipher,
            compatibility: config.compatibility,
            agent_catalog: config.agent_catalog,
            connection_events: config.connection_events,
            oauth: config.oauth,
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

impl axum::extract::FromRef<ConnectionsState> for crate::events::ConnectionEvents {
    fn from_ref(state: &ConnectionsState) -> crate::events::ConnectionEvents {
        state.connection_events.clone()
    }
}

impl axum::extract::FromRef<ConnectionsState> for OAuthOptions {
    fn from_ref(state: &ConnectionsState) -> OAuthOptions {
        state.oauth.clone()
    }
}
