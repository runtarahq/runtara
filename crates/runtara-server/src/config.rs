use std::sync::OnceLock;

const DEFAULT_MCP_ALLOWED_HOSTS: [&str; 3] = ["localhost", "127.0.0.1", "::1"];
const RUNTARA_MCP_ALLOWED_HOSTS_ENV: &str = "RUNTARA_MCP_ALLOWED_HOSTS";
const RUNTARA_MCP_SESSION_STORE_ENV: &str = "RUNTARA_MCP_SESSION_STORE";
const RUNTARA_MCP_SESSION_TTL_SECONDS_ENV: &str = "RUNTARA_MCP_SESSION_TTL_SECONDS";
const DEFAULT_MCP_SESSION_TTL_SECONDS: u64 = 86_400;

/// Global application configuration.
///
/// Loaded once at startup via [`Config::from_env`], then stored in a `OnceLock`.
/// All env-var parsing happens eagerly, so typos and invalid values fail fast.
#[derive(Debug, Clone)]
pub struct Config {
    /// Tenant ID for all API operations (required).
    pub tenant_id: String,
    /// Maximum number of concurrent workflow executions.
    pub max_concurrent_executions: usize,
    /// Checkpoint TTL in hours.
    pub checkpoint_ttl_hours: u64,
    /// Whether adaptive rate limiting is enabled.
    pub adaptive_rate_limiting_enabled: bool,
    /// Whether automatic retry on HTTP 429 responses is enabled.
    pub auto_retry_on_429_enabled: bool,
    /// Maximum retry attempts for 429 responses.
    pub max_429_retries: u32,
    /// Maximum retry delay in milliseconds.
    pub max_retry_delay_ms: u64,
    /// Object model database URL (required).
    pub object_model_database_url: String,
    /// Maximum pool connections for the object model database.
    pub object_model_max_connections: u32,
    /// Whether the object model uses soft delete.
    pub object_model_soft_delete: bool,
    /// Maximum number of items accepted per bulk request (create/upsert/update-by-ids).
    pub object_model_bulk_request_limit: usize,
    /// Internal HTTP port (used to derive default service URLs).
    pub internal_port: u16,
    /// Name of the stdlib crate compiled into workflows.
    pub stdlib_name: String,
    /// HTTP proxy URL forwarded to workflow processes for outbound HTTP.
    pub http_proxy_url: String,
    /// Object-model internal API URL forwarded to workflow processes.
    pub object_model_url: String,
    /// Agent service URL forwarded to workflow processes for native-only capabilities.
    pub agent_service_url: String,
    /// Host or host:port authorities accepted by the MCP Streamable HTTP transport.
    pub mcp_allowed_hosts: Vec<String>,
    /// Backing store for MCP Streamable HTTP session recovery.
    pub mcp_session_store: McpSessionStore,
    /// TTL for externally persisted MCP session recovery state.
    pub mcp_session_ttl_seconds: u64,
    /// Skip the graceful drain on Ctrl+C / SIGTERM. Defaults to true in debug
    /// builds so `cargo run` exits promptly; production release builds keep
    /// the full drain unless `RUNTARA_DEV_MODE=true` is set explicitly.
    pub dev_mode: bool,
}

/// Backing store for MCP Streamable HTTP session recovery.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpSessionStore {
    /// Process-local in-memory session state only.
    Local,
    /// Valkey-backed recovery state with process-local live workers.
    Valkey,
}

impl McpSessionStore {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Valkey => "valkey",
        }
    }
}

/// Global configuration instance.
static CONFIG: OnceLock<Config> = OnceLock::new();

impl Config {
    /// Load configuration from environment variables.
    ///
    /// Required:
    /// - `TENANT_ID`: organization identifier
    /// - `OBJECT_MODEL_DATABASE_URL`: Postgres URL for the object model DB
    pub fn from_env() -> Result<Self, ConfigError> {
        let tenant_id =
            std::env::var("TENANT_ID").map_err(|_| ConfigError::Missing("TENANT_ID"))?;

        let max_concurrent_executions: usize = std::env::var("MAX_CONCURRENT_EXECUTIONS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or_else(|| num_cpus::get() * 32);

        let checkpoint_ttl_hours: u64 = parse_u64_or("CHECKPOINT_TTL_HOURS", 48)?;
        let adaptive_rate_limiting_enabled: bool = parse_bool_or("ADAPTIVE_RATE_LIMITING", true)?;
        let auto_retry_on_429_enabled: bool = parse_bool_or("AUTO_RETRY_ON_429", true)?;
        let max_429_retries: u32 = parse_u32_or("MAX_429_RETRIES", 3)?;
        let max_retry_delay_ms: u64 = parse_u64_or("MAX_RETRY_DELAY_MS", 60_000)?;

        let object_model_database_url = std::env::var("OBJECT_MODEL_DATABASE_URL")
            .map_err(|_| ConfigError::Missing("OBJECT_MODEL_DATABASE_URL"))?;
        let object_model_max_connections: u32 = parse_u32_or("OBJECT_MODEL_MAX_CONNECTIONS", 10)?;
        let object_model_soft_delete: bool = parse_bool_or("OBJECT_MODEL_SOFT_DELETE", true)?;
        let object_model_bulk_request_limit: usize = parse_usize_or(
            "OBJECT_MODEL_BULK_REQUEST_LIMIT",
            runtara_object_store::DEFAULT_BULK_REQUEST_LIMIT,
        )?;

        let internal_port: u16 = std::env::var("INTERNAL_PORT")
            .unwrap_or_else(|_| "7002".to_string())
            .parse()
            .map_err(|_| ConfigError::Invalid("INTERNAL_PORT", "must be a valid port number"))?;

        let stdlib_name = std::env::var("RUNTARA_STDLIB_NAME")
            .unwrap_or_else(|_| "runtara_workflow_stdlib".to_string());

        let http_proxy_url = std::env::var("RUNTARA_HTTP_PROXY_URL")
            .unwrap_or_else(|_| format!("http://127.0.0.1:{}/api/internal/proxy", internal_port));

        let object_model_url = std::env::var("RUNTARA_OBJECT_MODEL_URL").unwrap_or_else(|_| {
            format!(
                "http://127.0.0.1:{}/api/internal/object-model",
                internal_port
            )
        });

        let agent_service_url = std::env::var("RUNTARA_AGENT_SERVICE_URL")
            .unwrap_or_else(|_| format!("http://127.0.0.1:{}/api/internal/agents", internal_port));

        let mcp_allowed_hosts = mcp_allowed_hosts_from_raw(
            std::env::var(RUNTARA_MCP_ALLOWED_HOSTS_ENV).ok().as_deref(),
        );
        let mcp_session_store = mcp_session_store_from_raw(
            std::env::var(RUNTARA_MCP_SESSION_STORE_ENV).ok().as_deref(),
        )?;
        let mcp_session_ttl_seconds = parse_positive_u64_or(
            RUNTARA_MCP_SESSION_TTL_SECONDS_ENV,
            DEFAULT_MCP_SESSION_TTL_SECONDS,
        )?;

        if mcp_session_store == McpSessionStore::Valkey
            && std::env::var("VALKEY_HOST")
                .ok()
                .is_none_or(|host| host.trim().is_empty())
        {
            return Err(ConfigError::MissingDependency {
                missing: "VALKEY_HOST",
                required_by: "RUNTARA_MCP_SESSION_STORE=valkey",
            });
        }

        let dev_mode: bool = parse_bool_or("RUNTARA_DEV_MODE", cfg!(debug_assertions))?;

        Ok(Self {
            tenant_id,
            max_concurrent_executions,
            checkpoint_ttl_hours,
            adaptive_rate_limiting_enabled,
            auto_retry_on_429_enabled,
            max_429_retries,
            max_retry_delay_ms,
            object_model_database_url,
            object_model_max_connections,
            object_model_soft_delete,
            object_model_bulk_request_limit,
            internal_port,
            stdlib_name,
            http_proxy_url,
            object_model_url,
            agent_service_url,
            mcp_allowed_hosts,
            mcp_session_store,
            mcp_session_ttl_seconds,
            dev_mode,
        })
    }
}

/// Configuration errors.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// A required environment variable is missing.
    #[error("missing required environment variable: {0}")]
    Missing(&'static str),

    /// A required dependency environment variable is missing.
    #[error("missing required environment variable: {missing} ({required_by})")]
    MissingDependency {
        missing: &'static str,
        required_by: &'static str,
    },

    /// An environment variable has an invalid value.
    #[error("invalid value for {0}: {1}")]
    Invalid(&'static str, &'static str),
}

fn parse_bool_or(name: &'static str, default: bool) -> Result<bool, ConfigError> {
    match std::env::var(name) {
        Ok(v) => parse_bool(&v).ok_or(ConfigError::Invalid(
            name,
            "must be one of true/false/1/0/yes/no/on/off",
        )),
        Err(_) => Ok(default),
    }
}

fn parse_bool(s: &str) -> Option<bool> {
    match s.trim().to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" | "on" => Some(true),
        "false" | "0" | "no" | "off" => Some(false),
        _ => None,
    }
}

fn parse_u32_or(name: &'static str, default: u32) -> Result<u32, ConfigError> {
    match std::env::var(name) {
        Ok(v) => v
            .parse()
            .map_err(|_| ConfigError::Invalid(name, "must be a non-negative integer")),
        Err(_) => Ok(default),
    }
}

fn parse_u64_or(name: &'static str, default: u64) -> Result<u64, ConfigError> {
    match std::env::var(name) {
        Ok(v) => v
            .parse()
            .map_err(|_| ConfigError::Invalid(name, "must be a non-negative integer")),
        Err(_) => Ok(default),
    }
}

fn parse_positive_u64_or(name: &'static str, default: u64) -> Result<u64, ConfigError> {
    let value = parse_u64_or(name, default)?;
    if value == 0 {
        return Err(ConfigError::Invalid(name, "must be a positive integer"));
    }
    Ok(value)
}

fn parse_usize_or(name: &'static str, default: usize) -> Result<usize, ConfigError> {
    match std::env::var(name) {
        Ok(v) => v
            .parse()
            .map_err(|_| ConfigError::Invalid(name, "must be a non-negative integer")),
        Err(_) => Ok(default),
    }
}

fn mcp_session_store_from_raw(raw: Option<&str>) -> Result<McpSessionStore, ConfigError> {
    match raw.map(str::trim) {
        None => Ok(McpSessionStore::Valkey),
        Some("local") => Ok(McpSessionStore::Local),
        Some("valkey") => Ok(McpSessionStore::Valkey),
        Some(_) => Err(ConfigError::Invalid(
            RUNTARA_MCP_SESSION_STORE_ENV,
            "must be one of local/valkey",
        )),
    }
}

fn mcp_allowed_hosts_from_raw(raw: Option<&str>) -> Vec<String> {
    let mut hosts: Vec<String> = DEFAULT_MCP_ALLOWED_HOSTS
        .iter()
        .map(|host| (*host).to_string())
        .collect();

    if let Some(raw) = raw {
        for host in raw
            .split(',')
            .map(str::trim)
            .filter(|host| !host.is_empty())
        {
            push_unique(&mut hosts, host.to_string());
        }
    }

    hosts
}

fn push_unique(values: &mut Vec<String>, value: String) {
    if !values.iter().any(|existing| existing == &value) {
        values.push(value);
    }
}

/// Initialize the global configuration. Must be called once at startup.
pub fn init(config: Config) {
    CONFIG
        .set(config)
        .expect("Config can only be initialized once");
}

/// Get the global configuration.
pub fn get() -> &'static Config {
    CONFIG.get().expect("Config must be initialized before use")
}

/// Get the tenant ID.
pub fn tenant_id() -> &'static str {
    &get().tenant_id
}

/// Get the maximum concurrent executions.
pub fn max_concurrent_executions() -> usize {
    get().max_concurrent_executions
}

/// Get checkpoint TTL in hours.
pub fn checkpoint_ttl_hours() -> u64 {
    get().checkpoint_ttl_hours
}

/// Validate that Redis is configured for checkpoint storage.
pub fn validate_checkpoint_config() -> Result<(), String> {
    let valkey_host = std::env::var("VALKEY_HOST").ok();

    if valkey_host.is_none() {
        return Err(
            "VALKEY_HOST environment variable is required for checkpoint storage. \
            Redis/Valkey is now a required dependency for workflow execution."
                .to_string(),
        );
    }

    Ok(())
}

/// Check if adaptive rate limiting is enabled.
pub fn adaptive_rate_limiting_enabled() -> bool {
    get().adaptive_rate_limiting_enabled
}

/// Check if automatic retry on 429 is enabled.
pub fn auto_retry_on_429_enabled() -> bool {
    get().auto_retry_on_429_enabled
}

/// Get maximum retry attempts for 429 responses.
pub fn max_429_retries() -> u32 {
    get().max_429_retries
}

/// Get maximum retry delay in milliseconds.
pub fn max_retry_delay_ms() -> u64 {
    get().max_retry_delay_ms
}

/// Get the object model database URL.
pub fn object_model_database_url() -> String {
    get().object_model_database_url.clone()
}

/// Get the maximum number of connections for the object model database pool.
pub fn object_model_max_connections() -> u32 {
    get().object_model_max_connections
}

/// Whether the object model uses soft delete.
pub fn object_model_soft_delete() -> bool {
    get().object_model_soft_delete
}

/// Maximum number of items accepted per bulk request (create/upsert/update-by-ids).
pub fn object_model_bulk_request_limit() -> usize {
    get().object_model_bulk_request_limit
}

/// Host or host:port authorities accepted by the MCP Streamable HTTP transport.
pub fn mcp_allowed_hosts() -> &'static [String] {
    &get().mcp_allowed_hosts
}

/// Backing store for MCP Streamable HTTP session recovery.
pub fn mcp_session_store() -> McpSessionStore {
    get().mcp_session_store
}

/// TTL for externally persisted MCP session recovery state.
pub fn mcp_session_ttl_seconds() -> u64 {
    get().mcp_session_ttl_seconds
}

/// Whether the server is running in development mode. When true, shutdown
/// skips the graceful execution/instance drain so Ctrl+C exits promptly.
pub fn dev_mode() -> bool {
    get().dev_mode
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_bool_values() {
        for v in ["true", "TRUE", "1", "yes", "YES", "on", "On"] {
            assert_eq!(parse_bool(v), Some(true), "{v:?} should be true");
        }
        for v in ["false", "0", "no", "off", "Off"] {
            assert_eq!(parse_bool(v), Some(false), "{v:?} should be false");
        }
        assert_eq!(parse_bool("nope"), None);
        assert_eq!(parse_bool(""), None);
    }

    #[test]
    fn parse_bool_or_returns_runtime_default_when_unset() {
        // Use a name we expect to be unset so we exercise the fallback
        // branch without touching shared process env (which is not safe to
        // mutate concurrently from tests).
        let name = "RUNTARA_TEST_PARSE_BOOL_OR_UNSET_UNIQUE_42";
        // SAFETY: only this test references this var; harmless if a prior
        // run leaked it.
        unsafe {
            std::env::remove_var(name);
        }
        assert_eq!(parse_bool_or(name, true).ok(), Some(true));
        assert_eq!(parse_bool_or(name, false).ok(), Some(false));
    }

    #[test]
    fn mcp_allowed_hosts_default_to_loopback() {
        assert_eq!(
            mcp_allowed_hosts_from_raw(None),
            vec!["localhost", "127.0.0.1", "::1"]
        );
    }

    #[test]
    fn mcp_allowed_hosts_extend_loopback_from_csv() {
        assert_eq!(
            mcp_allowed_hosts_from_raw(Some(
                " runtara.example.com, staging.example.com:8443 ,, 127.0.0.1 "
            )),
            vec![
                "localhost",
                "127.0.0.1",
                "::1",
                "runtara.example.com",
                "staging.example.com:8443"
            ]
        );
    }

    #[test]
    fn mcp_session_store_defaults_to_valkey() {
        assert_eq!(
            mcp_session_store_from_raw(None).unwrap(),
            McpSessionStore::Valkey
        );
    }

    #[test]
    fn mcp_session_store_parses_valid_values() {
        assert_eq!(
            mcp_session_store_from_raw(Some("local")).unwrap(),
            McpSessionStore::Local
        );
        assert_eq!(
            mcp_session_store_from_raw(Some("valkey")).unwrap(),
            McpSessionStore::Valkey
        );
    }

    #[test]
    fn mcp_session_store_rejects_invalid_values() {
        assert!(mcp_session_store_from_raw(Some("redis")).is_err());
        assert!(mcp_session_store_from_raw(Some("")).is_err());
    }
}
