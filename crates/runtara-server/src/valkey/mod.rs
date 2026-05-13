pub mod cleanup;
pub mod client;
pub mod compilation_queue;
pub mod events;
pub mod stream;

use redis::RedisError;
use redis::aio::ConnectionManager;
use tokio::sync::OnceCell;

/// Process-wide shared Redis connection manager.
///
/// Built lazily on first use (or eagerly at server startup via
/// [`init_shared_manager`]) and shared across every subsystem that talks
/// to Valkey. The manager itself wraps an `Arc`, so cloning is cheap and
/// every clone reuses the same multiplexed connection — no new TCP per
/// request.
///
/// The URL is captured at first initialization. The server runs against a
/// single Valkey instance whose URL is fixed for the process lifetime, so
/// caching a single manager (rather than keying by URL) is intentional.
///
/// # Non-blocking commands only
///
/// `ConnectionManager` is a *single multiplexed connection*. A blocking
/// command issued through this manager parks that connection and
/// head-of-line blocks every other caller until it returns. Callers that
/// issue any of the following commands MUST use
/// [`dedicated_manager_for_blocking_consumer`] instead:
///
/// - `BLPOP`, `BRPOP`, `BRPOPLPUSH`, `BLMOVE`
/// - `BZPOPMIN`, `BZPOPMAX`
/// - `XREAD ... BLOCK`, `XREADGROUP ... BLOCK`
/// - `SUBSCRIBE`, `PSUBSCRIBE`
/// - `WAIT`
/// - any long-running Lua / `EVAL` script
///
/// Production incident on 2026-05-13 (commit `8c43211`): the compilation
/// worker's `BLPOP` was on this shared manager and stalled proxy
/// rate-limit checks to 3–6 s. Do not re-introduce that pattern.
static SHARED_MANAGER: OnceCell<ConnectionManager> = OnceCell::const_new();

/// Return the shared connection manager, building it on first call.
///
/// **Use only for non-blocking commands.** See the warning on
/// [`SHARED_MANAGER`]. For blocking consumers call
/// [`dedicated_manager_for_blocking_consumer`].
///
/// Subsequent calls are O(1) clones. Returns an error only if Redis is
/// unreachable on the very first call (subsequent reconnects are handled
/// transparently by `ConnectionManager`).
pub async fn get_or_create_manager(redis_url: &str) -> Result<ConnectionManager, RedisError> {
    SHARED_MANAGER
        .get_or_try_init(|| async {
            let client = redis::Client::open(redis_url)?;
            ConnectionManager::new(client).await
        })
        .await
        .cloned()
}

/// Eagerly initialize the shared manager at startup. Safe to call multiple
/// times; only the first call performs the connection.
pub async fn init_shared_manager(redis_url: &str) -> Result<ConnectionManager, RedisError> {
    get_or_create_manager(redis_url).await
}

/// Build a fresh, isolated `ConnectionManager` for a consumer that issues
/// blocking Redis commands (`BLPOP`, `XREADGROUP ... BLOCK`, `SUBSCRIBE`,
/// …).
///
/// Every call returns an independent manager backed by its own connection.
/// The point is isolation — do NOT share the returned handle with the
/// shared manager's callers, or you negate the benefit. `consumer_name`
/// is used for log/trace context only.
///
/// A grep for this function name enumerates every blocking Redis consumer
/// in the codebase, which is the second purpose of routing through it.
pub async fn dedicated_manager_for_blocking_consumer(
    redis_url: &str,
    consumer_name: &str,
) -> Result<ConnectionManager, RedisError> {
    let client = redis::Client::open(redis_url)?;
    let manager = ConnectionManager::new(client).await?;
    tracing::debug!(
        consumer = consumer_name,
        "Opened dedicated Redis ConnectionManager for blocking consumer"
    );
    Ok(manager)
}

/// Valkey configuration loaded from environment variables
#[derive(Debug, Clone)]
pub struct ValkeyConfig {
    pub host: String,
    pub port: u16,
    pub user: Option<String>,
    pub password: Option<String>,
    /// Stream name for raw event capture (legacy)
    pub stream_name: String,
    /// Consumer group for raw event capture (legacy)
    pub consumer_group: String,
    /// Stream prefix for trigger events (default: "runtara:triggers")
    pub trigger_stream_prefix: String,
    /// Consumer group for trigger workers (default: "runtara-trigger-workers")
    pub trigger_consumer_group: String,
}

impl ValkeyConfig {
    /// Load Valkey configuration from environment variables
    /// Returns None if VALKEY_HOST is not set (Valkey is optional)
    pub fn from_env() -> Option<Self> {
        let host = std::env::var("VALKEY_HOST").ok()?;

        let port = std::env::var("VALKEY_PORT")
            .ok()
            .and_then(|p| p.parse().ok())
            .unwrap_or(6379);

        let user = std::env::var("VALKEY_USER").ok();
        let password = std::env::var("VALKEY_PASSWORD").ok();

        let stream_name =
            std::env::var("VALKEY_STREAM_NAME").unwrap_or_else(|_| "runtara-events".to_string());

        let consumer_group = std::env::var("VALKEY_CONSUMER_GROUP")
            .unwrap_or_else(|_| "runtara-workers".to_string());

        let trigger_stream_prefix = std::env::var("VALKEY_TRIGGER_STREAM_PREFIX")
            .unwrap_or_else(|_| "runtara:triggers".to_string());

        let trigger_consumer_group = std::env::var("VALKEY_TRIGGER_CONSUMER_GROUP")
            .unwrap_or_else(|_| "runtara-trigger-workers".to_string());

        Some(ValkeyConfig {
            host,
            port,
            user,
            password,
            stream_name,
            consumer_group,
            trigger_stream_prefix,
            trigger_consumer_group,
        })
    }

    /// Build Redis connection URL from config
    /// Format: redis://[user:password@]host:port
    pub fn connection_url(&self) -> String {
        match (&self.user, &self.password) {
            (Some(user), Some(password)) => {
                format!("redis://{}:{}@{}:{}", user, password, self.host, self.port)
            }
            (None, Some(password)) => {
                format!("redis://:{}@{}:{}", password, self.host, self.port)
            }
            _ => {
                format!("redis://{}:{}", self.host, self.port)
            }
        }
    }

    /// Get the trigger stream key for a specific tenant
    /// Format: {trigger_stream_prefix}:{tenant_id}
    pub fn trigger_stream_key(&self, tenant_id: &str) -> String {
        format!("{}:{}", self.trigger_stream_prefix, tenant_id)
    }
}

/// Build Redis connection URL from environment variables
/// Returns None if VALKEY_HOST is not set
pub fn build_redis_url() -> Option<String> {
    ValkeyConfig::from_env().map(|config| config.connection_url())
}
