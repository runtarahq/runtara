pub mod cleanup;
pub mod client;
pub mod compilation_queue;
pub mod events;
pub mod stream;

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
