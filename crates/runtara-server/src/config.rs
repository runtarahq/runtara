use std::sync::OnceLock;

/// Global application configuration
#[derive(Debug)]
pub struct Config {
    /// Tenant ID for all API operations
    pub tenant_id: String,
    /// Maximum number of concurrent scenario executions
    pub max_concurrent_executions: usize,
}

/// Global configuration instance
static CONFIG: OnceLock<Config> = OnceLock::new();

/// Initialize the global configuration
pub fn init(tenant_id: String, max_concurrent_executions: usize) {
    CONFIG
        .set(Config {
            tenant_id,
            max_concurrent_executions,
        })
        .expect("Config can only be initialized once");
}

/// Get the global configuration
pub fn get() -> &'static Config {
    CONFIG.get().expect("Config must be initialized before use")
}

/// Get the tenant ID
pub fn tenant_id() -> &'static str {
    &get().tenant_id
}

/// Get the maximum concurrent executions
pub fn max_concurrent_executions() -> usize {
    get().max_concurrent_executions
}

/// Get checkpoint TTL in hours (default: 48 hours)
pub fn checkpoint_ttl_hours() -> u64 {
    std::env::var("CHECKPOINT_TTL_HOURS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(48)
}

/// Validate that Redis is configured for checkpoint storage
pub fn validate_checkpoint_config() -> Result<(), String> {
    let valkey_host = std::env::var("VALKEY_HOST").ok();

    if valkey_host.is_none() {
        return Err(
            "VALKEY_HOST environment variable is required for checkpoint storage. \
            Redis/Valkey is now a required dependency for scenario execution."
                .to_string(),
        );
    }

    Ok(())
}

/// Check if adaptive rate limiting is enabled (default: true)
pub fn adaptive_rate_limiting_enabled() -> bool {
    std::env::var("ADAPTIVE_RATE_LIMITING")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(true)
}

/// Check if automatic retry on 429 is enabled (default: true)
pub fn auto_retry_on_429_enabled() -> bool {
    std::env::var("AUTO_RETRY_ON_429")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(true)
}

/// Get maximum retry attempts for 429 responses (default: 3)
pub fn max_429_retries() -> u32 {
    std::env::var("MAX_429_RETRIES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(3)
}

/// Get maximum retry delay in milliseconds (default: 60000 = 1 minute)
pub fn max_retry_delay_ms() -> u64 {
    std::env::var("MAX_RETRY_DELAY_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(60000)
}

/// Get the object model database URL (required)
pub fn object_model_database_url() -> String {
    std::env::var("OBJECT_MODEL_DATABASE_URL")
        .expect("OBJECT_MODEL_DATABASE_URL environment variable is required")
}

/// Get the maximum number of connections for the object model database pool (default: 5)
pub fn object_model_max_connections() -> u32 {
    std::env::var("OBJECT_MODEL_MAX_CONNECTIONS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(5)
}
