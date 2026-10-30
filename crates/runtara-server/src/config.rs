pub mod isolation;
mod runtime;

pub use runtime::{RuntimeOverrides, RuntimePoolConfig, ShutdownGrace, core_instance_api_enabled};

use crate::entitlements::EntitlementSnapshot;
use runtara_environment::execution_timeout::{
    DEFAULT_EXECUTION_TIMEOUT_SECS, ExecutionTimeoutPolicy, MAX_EXECUTION_TIMEOUT_SECS,
};
use std::collections::BTreeSet;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

const DEFAULT_MCP_ALLOWED_HOSTS: [&str; 3] = ["localhost", "127.0.0.1", "::1"];
const RUNTARA_MCP_ALLOWED_HOSTS_ENV: &str = "RUNTARA_MCP_ALLOWED_HOSTS";
const RUNTARA_MCP_SESSION_STORE_ENV: &str = "RUNTARA_MCP_SESSION_STORE";
const RUNTARA_MCP_SESSION_TTL_SECONDS_ENV: &str = "RUNTARA_MCP_SESSION_TTL_SECONDS";
const RUNTARA_DIRECT_WASM_COMPONENTS_DIR_ENV: &str = "RUNTARA_DIRECT_WASM_COMPONENTS_DIR";
const DEFAULT_MCP_SESSION_TTL_SECONDS: u64 = 86_400;
const RUNTARA_DEFAULT_EXECUTION_TIMEOUT_SECS_ENV: &str = "RUNTARA_DEFAULT_EXECUTION_TIMEOUT_SECS";
const RUNTARA_MAX_EXECUTION_TIMEOUT_SECS_ENV: &str = "RUNTARA_MAX_EXECUTION_TIMEOUT_SECS";

/// Global application configuration.
///
/// Loaded once at startup via [`Config::from_env`], then stored in a `OnceLock`.
/// All env-var parsing happens eagerly, so typos and invalid values fail fast.
#[derive(Debug, Clone)]
pub struct Config {
    /// Tenant ID for all API operations (required).
    pub tenant_id: String,
    /// Every tenant this process serves: `tenant_id` first, then those
    /// `RUNTARA_SERVED_TENANTS` adds (comma-separated). One by default.
    pub served_tenants: Vec<String>,
    /// Maximum number of concurrent workflow executions.
    pub max_concurrent_executions: usize,
    /// One bounded policy shared by workflow validation, the runtime client,
    /// and the embedded Environment.
    pub execution_timeout_policy: ExecutionTimeoutPolicy,
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
    /// The platform's object model database URL. Optional: without it there
    /// is no default database, and the object model, SQL steps and database
    /// tools use the tenant's own Postgres connections.
    pub object_model_database_url: Option<String>,
    /// Whether tenant database connections must not reach private, loopback
    /// or metadata addresses (`RUNTARA_DATABASE_CONNECTION_GUARD`,
    /// `enforce`/`off`; enforced by default without a platform database).
    pub database_connection_guard: bool,
    /// The most database pools one tenant's connections may hold open at once
    /// (`OBJECT_MODEL_TENANT_POOL_MAX`).
    pub object_model_tenant_pool_max: usize,
    /// Whether the contract version 1 `member:{uid}` record still answers for
    /// the primary tenant (`RUNTARA_AUTH_LEGACY_MEMBER_KEYS`, default true).
    /// A Valkey shared by several processes must turn it off: that record
    /// names no tenant, so it would answer for every process's primary.
    pub legacy_member_keys: bool,
    /// Whether this process shares its Valkey with other tenants' processes,
    /// each confined by Valkey access rules to its own tenants' keys
    /// (`RUNTARA_VALKEY_TENANT_SCOPED`, default false). It then keeps every
    /// key it uses under a served tenant (managed session queues at
    /// `{tenant}:session:…`), and never touches the unscoped legacy keys or
    /// scans for other tenants' streams. It implies no legacy member keys.
    pub valkey_tenant_scoped: bool,
    /// Maximum pool connections for the object model database.
    pub object_model_max_connections: u32,
    /// Whether the object model uses soft delete.
    pub object_model_soft_delete: bool,
    /// Maximum number of items accepted per bulk request (create/upsert/update-by-ids).
    pub object_model_bulk_request_limit: usize,
    /// Connection-pool tuning for the runtime database the embedded runtime
    /// owns. Parsed here so a malformed value stops the process before it
    /// opens a pool, rather than only disabling workflow execution.
    pub runtime_pool: RuntimePoolConfig,
    /// How long each stage of the shutdown drain waits.
    pub shutdown_grace: ShutdownGrace,
    /// Guard rails for caller-supplied raw SQL: the workflow query-sql /
    /// execute-sql capabilities and the runtime/MCP `object-model/sql/*` routes.
    pub raw_sql_guardrails: runtara_object_store::SqlGuardrails,
    /// Connection-pool tuning for per-connection object-model PostgreSQL pools
    /// (the cross-cloud path to customer databases) and the default pool.
    pub object_model_pool: runtara_object_store::PoolConfig,
    /// Max number of distinct object-model store pools kept warm (LRU cap).
    pub object_model_pool_cache_max: u64,
    /// Idle seconds before an unused object-model store pool is evicted.
    pub object_model_pool_cache_ttl_secs: u64,
    /// Directory containing per-agent WASM components (`runtara_agent_*.wasm`).
    /// When set, `AgentTestingService` routes known agents through the
    /// embedded wasmtime path instead of the legacy dispatcher image.
    pub agent_components_dir: Option<std::path::PathBuf>,
    /// Directory containing prebuilt direct workflow stdlib/runtime components
    /// plus agent components. Defaults to `agent_components_dir`.
    pub direct_wasm_components_dir: Option<std::path::PathBuf>,
    /// Runtime approvals retained solely for previously compiled isolation artifacts.
    pub isolation_policy: Option<std::sync::Arc<isolation::IsolationPolicy>>,
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
    /// The primary tenant's entitlements.
    pub entitlement_snapshot: EntitlementSnapshot,
    /// Other served tenants' own entitlements (`RUNTARA_TENANT_ENTITLEMENTS_JSON`);
    /// a served tenant without an entry has the primary's.
    pub tenant_entitlements: std::collections::BTreeMap<String, Arc<EntitlementSnapshot>>,
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
    ///
    /// Optional:
    /// - `OBJECT_MODEL_DATABASE_URL`: the platform's object model database
    pub fn from_env() -> Result<Self, ConfigError> {
        let tenant_id =
            std::env::var("TENANT_ID").map_err(|_| ConfigError::Missing("TENANT_ID"))?;
        let served_tenants =
            served_tenants_from_raw(&tenant_id, std::env::var("RUNTARA_SERVED_TENANTS").ok());
        // Valkey keys are `{tenant}:…`; a tenant naming a separator could
        // spell another tenant's key.
        if let Some(tenant) = served_tenants.iter().find(|t| t.contains(':')) {
            return Err(ConfigError::InvalidValue(
                "TENANT_ID / RUNTARA_SERVED_TENANTS",
                format!("tenant '{tenant}' contains ':'"),
            ));
        }

        let max_concurrent_executions: usize = parse_usize_or(
            "MAX_CONCURRENT_EXECUTIONS",
            default_max_concurrent_executions(),
        )?;

        // The embedded runtime's pool and the shutdown drain are configured
        // here rather than at the point of use. Both used to be read much
        // later — the pool after the server had already booted, and the drain
        // after the environment had relaunched recovered instances — so a typo
        // in either produced a running server with no workflow engine, or a
        // crash loop through recovery, instead of a startup failure.
        let runtime_pool = RuntimePoolConfig::from_env()?;
        let shutdown_grace = ShutdownGrace::from_env()?;

        let checkpoint_ttl_hours: u64 = parse_u64_or("CHECKPOINT_TTL_HOURS", 48)?;
        let adaptive_rate_limiting_enabled: bool = parse_bool_or("ADAPTIVE_RATE_LIMITING", true)?;
        let auto_retry_on_429_enabled: bool = parse_bool_or("AUTO_RETRY_ON_429", true)?;
        let max_429_retries: u32 = parse_u32_or("MAX_429_RETRIES", 3)?;
        let max_retry_delay_ms: u64 = parse_u64_or("MAX_RETRY_DELAY_MS", 60_000)?;
        let execution_timeout_policy = execution_timeout_policy_from_env()?;

        let object_model_database_url = std::env::var("OBJECT_MODEL_DATABASE_URL")
            .ok()
            .map(|url| url.trim().to_owned())
            .filter(|url| !url.is_empty());
        let database_connection_guard = match std::env::var("RUNTARA_DATABASE_CONNECTION_GUARD")
            .ok()
            .map(|mode| mode.trim().to_ascii_lowercase())
            .as_deref()
        {
            None | Some("") => object_model_database_url.is_none(),
            Some("enforce") => true,
            Some("off") => false,
            Some(_) => {
                return Err(ConfigError::Invalid(
                    "RUNTARA_DATABASE_CONNECTION_GUARD",
                    "must be enforce or off",
                ));
            }
        };
        let object_model_tenant_pool_max: usize =
            parse_usize_or("OBJECT_MODEL_TENANT_POOL_MAX", 16)?;
        let legacy_member_keys = parse_bool_or("RUNTARA_AUTH_LEGACY_MEMBER_KEYS", true)?;
        let valkey_tenant_scoped = parse_bool_or("RUNTARA_VALKEY_TENANT_SCOPED", false)?;
        let object_model_max_connections: u32 = parse_u32_or("OBJECT_MODEL_MAX_CONNECTIONS", 10)?;
        let object_model_soft_delete: bool = parse_bool_or("OBJECT_MODEL_SOFT_DELETE", true)?;
        let object_model_bulk_request_limit: usize = parse_usize_or(
            "OBJECT_MODEL_BULK_REQUEST_LIMIT",
            runtara_object_store::DEFAULT_BULK_REQUEST_LIMIT,
        )?;

        // Per-connection pool tuning. `max_connections` defaults to the existing
        // `OBJECT_MODEL_MAX_CONNECTIONS` so the two knobs stay consistent; an idle
        // timeout / max lifetime of 0 disables that timeout.
        let object_model_pool = runtara_object_store::PoolConfig {
            max_connections: parse_u32_or(
                "OBJECT_MODEL_POOL_MAX_CONNECTIONS",
                object_model_max_connections,
            )?,
            min_connections: parse_u32_or(
                "OBJECT_MODEL_POOL_MIN_CONNECTIONS",
                runtara_object_store::DEFAULT_POOL_MIN_CONNECTIONS,
            )?,
            acquire_timeout: Duration::from_secs(parse_u64_or(
                "OBJECT_MODEL_POOL_ACQUIRE_TIMEOUT_SECS",
                runtara_object_store::DEFAULT_POOL_ACQUIRE_TIMEOUT_SECS,
            )?),
            idle_timeout: secs_to_opt_duration(parse_u64_or(
                "OBJECT_MODEL_POOL_IDLE_TIMEOUT_SECS",
                runtara_object_store::DEFAULT_POOL_IDLE_TIMEOUT_SECS,
            )?),
            max_lifetime: secs_to_opt_duration(parse_u64_or(
                "OBJECT_MODEL_POOL_MAX_LIFETIME_SECS",
                runtara_object_store::DEFAULT_POOL_MAX_LIFETIME_SECS,
            )?),
            test_before_acquire: parse_bool_or(
                "OBJECT_MODEL_POOL_TEST_BEFORE_ACQUIRE",
                runtara_object_store::DEFAULT_POOL_TEST_BEFORE_ACQUIRE,
            )?,
            statement_cache_capacity: parse_usize_or(
                "OBJECT_MODEL_POOL_STMT_CACHE",
                runtara_object_store::DEFAULT_POOL_STATEMENT_CACHE_CAPACITY,
            )?,
        };
        let object_model_pool_cache_max: u64 = parse_u64_or("OBJECT_MODEL_POOL_CACHE_MAX", 256)?;
        let object_model_pool_cache_ttl_secs: u64 =
            parse_u64_or("OBJECT_MODEL_POOL_CACHE_TTL_SECS", 900)?;

        // Workflow raw-SQL guard rails. Zero/invalid values fail boot
        // (parse_positive_u64_or) — a knob is never "unset means unbounded".
        let instance_timeout_ms =
            u64::from(execution_timeout_policy.default_timeout().as_secs()).saturating_mul(1000);
        let raw_sql_guardrails = runtara_object_store::SqlGuardrails {
            statement_timeout_ms: clamp_statement_timeout_ms(
                parse_positive_u64_or("RUNTARA_RAW_SQL_STATEMENT_TIMEOUT_MS", 60_000)?,
                instance_timeout_ms,
            ),
            max_rows: parse_positive_u64_or("RUNTARA_RAW_SQL_MAX_ROWS", 10_000)?,
            max_response_bytes: parse_positive_u64_or(
                "RUNTARA_RAW_SQL_MAX_RESPONSE_BYTES",
                64 * 1024 * 1024,
            )?,
        };

        let agent_components_dir = std::env::var("RUNTARA_AGENT_COMPONENTS_DIR")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .map(std::path::PathBuf::from);
        // Workflows compile with the direct WASM emitter only, from the
        // release this server runs (see `agent_release`) unless an explicit
        // directory overrides it; without a release, from
        // `agent_components_dir`.
        let direct_wasm_components_dir = direct_wasm_components_dir_from_raw(
            std::env::var(RUNTARA_DIRECT_WASM_COMPONENTS_DIR_ENV)
                .ok()
                .as_deref(),
            COMPONENT_RELEASE
                .get()
                .map(|release| release.dir.as_path())
                .or(agent_components_dir.as_deref()),
        );

        let isolation_policy = isolation::IsolationPolicy::from_env()?;
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

        // The entitlement allowlist tracks exactly the agents that are
        // deployable — i.e. the agents of the release this server runs (see
        // `agent_release`). The runtime only dispatches
        // components, so an agent that isn't there can't run and must not be
        // "registered". (The static `runtara_agents::registry` is no longer a
        // source here: most of it duplicates components that supersede it, and
        // serving those ids would let a workflow validate against an agent the
        // runtime can't actually execute.)
        let registered_agents: BTreeSet<String> = discover_component_agent_ids();

        let entitlement_snapshot: EntitlementSnapshot = EntitlementSnapshot::parse_entitlements(
            &tenant_id,
            std::env::var("RUNTARA_PRICING_TIER").ok().as_deref(),
            std::env::var("RUNTARA_ENTITLEMENTS_JSON").ok().as_deref(),
            std::env::var("RUNTARA_ENTITLEMENT_OVERRIDES_JSON")
                .ok()
                .as_deref(),
            &registered_agents,
        )?;
        let mut tenant_entitlements = tenant_entitlements_from_raw(
            std::env::var("RUNTARA_TENANT_ENTITLEMENTS_JSON")
                .ok()
                .as_deref(),
            &served_tenants,
            &registered_agents,
        )?;
        // The primary's own entry, which every unlisted tenant shares.
        tenant_entitlements
            .entry(tenant_id.clone())
            .or_insert_with(|| Arc::new(entitlement_snapshot.clone()));

        Ok(Self {
            tenant_id,
            served_tenants,
            max_concurrent_executions,
            execution_timeout_policy,
            checkpoint_ttl_hours,
            adaptive_rate_limiting_enabled,
            auto_retry_on_429_enabled,
            max_429_retries,
            max_retry_delay_ms,
            object_model_database_url,
            database_connection_guard,
            object_model_tenant_pool_max,
            legacy_member_keys,
            valkey_tenant_scoped,
            object_model_max_connections,
            object_model_soft_delete,
            object_model_bulk_request_limit,
            runtime_pool,
            shutdown_grace,
            raw_sql_guardrails,
            object_model_pool,
            object_model_pool_cache_max,
            object_model_pool_cache_ttl_secs,
            agent_components_dir,
            direct_wasm_components_dir,
            isolation_policy,
            mcp_allowed_hosts,
            mcp_session_store,
            mcp_session_ttl_seconds,
            dev_mode,
            entitlement_snapshot,
            tenant_entitlements,
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

    /// An environment variable has an invalid value, with dynamic detail.
    #[error("invalid value for {0}: {1}")]
    InvalidValue(&'static str, String),
}

/// Parse the process-wide active-execution timeout policy without reading
/// mutable global state. Keeping the raw parser separate makes the bounds and
/// unit labels testable without racing other configuration tests.
fn execution_timeout_policy_from_raw(
    default_raw: Option<&str>,
    maximum_raw: Option<&str>,
) -> Result<ExecutionTimeoutPolicy, ConfigError> {
    let default_seconds = match default_raw {
        Some(value) => value.parse::<u32>().map_err(|_| {
            ConfigError::Invalid(
                RUNTARA_DEFAULT_EXECUTION_TIMEOUT_SECS_ENV,
                "must be a positive whole number of seconds",
            )
        })?,
        None => DEFAULT_EXECUTION_TIMEOUT_SECS,
    };
    let maximum_seconds = match maximum_raw {
        Some(value) => value.parse::<u32>().map_err(|_| {
            ConfigError::Invalid(
                RUNTARA_MAX_EXECUTION_TIMEOUT_SECS_ENV,
                "must be a positive whole number of seconds",
            )
        })?,
        None => MAX_EXECUTION_TIMEOUT_SECS,
    };

    ExecutionTimeoutPolicy::new(default_seconds, maximum_seconds).map_err(|error| {
        let name = if default_seconds > maximum_seconds {
            RUNTARA_DEFAULT_EXECUTION_TIMEOUT_SECS_ENV
        } else if maximum_seconds > MAX_EXECUTION_TIMEOUT_SECS {
            RUNTARA_MAX_EXECUTION_TIMEOUT_SECS_ENV
        } else {
            RUNTARA_DEFAULT_EXECUTION_TIMEOUT_SECS_ENV
        };
        ConfigError::InvalidValue(name, error.to_string())
    })
}

fn execution_timeout_policy_from_env() -> Result<ExecutionTimeoutPolicy, ConfigError> {
    let default = std::env::var(RUNTARA_DEFAULT_EXECUTION_TIMEOUT_SECS_ENV).ok();
    let maximum = std::env::var(RUNTARA_MAX_EXECUTION_TIMEOUT_SECS_ENV).ok();
    execution_timeout_policy_from_raw(default.as_deref(), maximum.as_deref())
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

static RELEASE_AGENTS: OnceLock<BTreeSet<String>> = OnceLock::new();

/// The platform release workflows compile against: its name, the digest of
/// its index, and the directory it is materialized in for composition.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ComponentRelease {
    pub release: String,
    pub index: String,
    pub dir: std::path::PathBuf,
}

static COMPONENT_RELEASE: OnceLock<ComponentRelease> = OnceLock::new();

/// Record the release workflows compile against, before the config is
/// loaded. Set once, at boot.
pub fn install_component_release(release: ComponentRelease) {
    let _ = COMPONENT_RELEASE.set(release);
}

/// The release compilation reads its components from: `None` before boot,
/// and when `RUNTARA_DIRECT_WASM_COMPONENTS_DIR` overrides it.
pub fn compile_component_release() -> Option<&'static ComponentRelease> {
    let release = COMPONENT_RELEASE.get()?;
    (CONFIG.get()?.direct_wasm_components_dir.as_deref() == Some(release.dir.as_path()))
        .then_some(release)
}

/// Record the agents of the release this server runs, before the config is
/// loaded; the entitlement allowlist is exactly these. Set once, at boot.
pub fn install_release_agents(ids: impl IntoIterator<Item = String>) {
    let _ = RELEASE_AGENTS.set(ids.into_iter().collect());
}

/// The kebab-cased agent ids of the release this server runs, as installed
/// at boot by [`install_release_agents`]. A config built outside the server's
/// boot (tests, tools) has no release, and falls back to the agent ids
/// deployed at `$RUNTARA_AGENT_COMPONENTS_DIR`, derived from
/// `runtara_agent_*.wasm` filenames. Returns an empty set when the env var is unset or the directory
/// is unreadable — the static registry alone is enough for CLI / fallback
/// paths. Doesn't parse the `.meta.json` files (the `ComponentDispatcherService`
/// already does that at boot for full metadata); the filename stem is all the
/// entitlement allowlist needs, and parsing has to wait until after env-var
/// parsing has finished anyway.
fn discover_component_agent_ids() -> BTreeSet<String> {
    if let Some(ids) = RELEASE_AGENTS.get() {
        return ids.clone();
    }
    let Ok(dir) = std::env::var("RUNTARA_AGENT_COMPONENTS_DIR") else {
        return BTreeSet::new();
    };
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return BTreeSet::new();
    };
    let mut ids = BTreeSet::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("wasm") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        let Some(stem_id) = stem.strip_prefix("runtara_agent_") else {
            continue;
        };
        // Matches `ComponentDispatcherService::from_dir` — cargo-component
        // emits snake_case wasm filenames, the canonical id is kebab.
        ids.insert(stem_id.replace('_', "-"));
    }
    ids
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

/// Clamp the raw-SQL statement timeout strictly below the whole-instance
/// execution timeout: a statement that outlives the instance would keep
/// running on Postgres after the workflow is already SIGKILLed.
fn clamp_statement_timeout_ms(configured_ms: u64, instance_timeout_ms: u64) -> u64 {
    if configured_ms >= instance_timeout_ms {
        let clamped = instance_timeout_ms.saturating_sub(1).max(1);
        tracing::warn!(
            configured_ms,
            clamped_ms = clamped,
            "RUNTARA_RAW_SQL_STATEMENT_TIMEOUT_MS clamped below the instance execution timeout"
        );
        clamped
    } else {
        configured_ms
    }
}

/// Convert a seconds value into an optional `Duration`, treating `0` as "disabled".
fn secs_to_opt_duration(secs: u64) -> Option<Duration> {
    if secs == 0 {
        None
    } else {
        Some(Duration::from_secs(secs))
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

fn direct_wasm_components_dir_from_raw(
    direct_components_dir: Option<&str>,
    agent_components_dir: Option<&std::path::Path>,
) -> Option<std::path::PathBuf> {
    direct_components_dir
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(std::path::PathBuf::from)
        .or_else(|| agent_components_dir.map(std::path::Path::to_path_buf))
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

/// Legacy artifact runtime approvals. Never rereads the file.
pub fn isolation_policy() -> Option<std::sync::Arc<isolation::IsolationPolicy>> {
    try_get().and_then(|config| config.isolation_policy.clone())
}

/// New compilation always uses standard component composition. Legacy runtime
/// approvals cannot change cache identity or select another compiler backend.
pub fn workflow_lowering_tag() -> String {
    runtara_workflows::direct_lowering_tag()
}

/// Initialize the global configuration for a unit test, if nothing has yet.
///
/// The global is a `OnceLock` shared by every test in this binary, so this
/// tolerates losing the race rather than panicking the way [`init`] does.
#[cfg(test)]
pub(crate) fn init_for_test() {
    if CONFIG.get().is_some() {
        return;
    }
    // Only fill what `from_env` requires and only when absent, so a test that
    // has deliberately configured the process is never overwritten.
    for (key, value) in [
        ("TENANT_ID", "test-tenant"),
        ("OBJECT_MODEL_DATABASE_URL", "postgres://unused/unused"),
        ("VALKEY_HOST", "127.0.0.1"),
    ] {
        if std::env::var(key).is_err() {
            unsafe { std::env::set_var(key, value) };
        }
    }
    if let Ok(config) = Config::from_env() {
        let _ = CONFIG.set(config);
    }
}

/// Get the global configuration if it has been initialized.
///
/// For code that can also run outside a booted server — unit tests, and
/// anything constructed before `init` — where panicking on an uninitialized
/// `OnceLock` would be worse than falling back to a default.
pub fn try_get() -> Option<&'static Config> {
    CONFIG.get()
}

/// The bounded active-execution timeout policy for this process.
///
/// Tests and standalone helpers that run before [`init`] use the same product
/// default as a standalone Environment rather than inventing an unbounded
/// fallback.
pub fn execution_timeout_policy() -> ExecutionTimeoutPolicy {
    try_get()
        .map(|config| config.execution_timeout_policy)
        .unwrap_or_default()
}

/// Get the tenant ID.
pub fn tenant_id() -> &'static str {
    &get().tenant_id
}

/// Every tenant this process serves, the primary ([`tenant_id`]) first.
pub fn served_tenants() -> &'static [String] {
    &get().served_tenants
}

/// Whether this process serves `tenant`. Requests and work for any other
/// tenant are refused.
pub fn is_served_tenant(tenant: &str) -> bool {
    try_get().is_some_and(|config| config.served_tenants.iter().any(|served| served == tenant))
}

fn served_tenants_from_raw(primary: &str, extra: Option<String>) -> Vec<String> {
    let mut tenants = vec![primary.to_owned()];
    for tenant in extra.iter().flat_map(|raw| raw.split(',')) {
        let tenant = tenant.trim();
        if !tenant.is_empty() && !tenants.iter().any(|served| served == tenant) {
            tenants.push(tenant.to_owned());
        }
    }
    tenants
}

/// Get the tenant ID, or `None` when the global config hasn't been
/// initialised yet. Use this from code paths that may run before
/// [`init`] — for example, structured-log call sites that want to
/// degrade gracefully rather than panic in unit tests.
pub fn try_tenant_id() -> Option<&'static str> {
    CONFIG.get().map(|c| c.tenant_id.as_str())
}

/// Returns `min(infra, entitlement)`; entitlement values can only lower the cap, never
/// raise it.
pub fn max_concurrent_executions() -> usize {
    crate::middleware::entitlement::effective_limit(
        get().max_concurrent_executions,
        entitlements().limits.max_concurrent_executions,
    )
}

/// Returns the **raw infra cap** (just the env-derived value), without
/// composing against the tenant entitlement. Callers that need the
/// composed cap should use [`max_concurrent_executions`]; callers that
/// need to do the composition themselves (e.g. the in-flight gate in
/// `ExecutionEngine::queue`) read this and call `effective_limit` with the
/// entitlement value, so the source of each side of the `min` is explicit
/// at the call site.
pub fn raw_max_concurrent_executions() -> usize {
    get().max_concurrent_executions
}

/// Admission ceiling used when `MAX_CONCURRENT_EXECUTIONS` is unset.
///
/// Derived from the runner's own bound rather than from the core count
/// directly, so the two cannot drift apart. Admission is allowed the work the
/// runner can actually execute plus a queue of the same depth; beyond that a
/// higher ceiling buys queue, not throughput, because an admitted request
/// holds its reservation from intake until it terminates or parks. A ceiling
/// far above the runner's capacity therefore shows up as 403s with most of the
/// budget sitting in `queued` — observed in production as 78 of 123 held
/// reservations waiting on a runner that only executes 16 at a time.
fn default_max_concurrent_executions() -> usize {
    runtara_environment::runner::embedded::max_concurrent_runs().saturating_mul(2)
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

/// The platform's object model database URL, when the server has one.
pub fn object_model_database_url() -> Option<String> {
    get().object_model_database_url.clone()
}

/// Whether tenant database connections are refused private, loopback and
/// metadata addresses.
pub fn database_connection_guard() -> bool {
    get().database_connection_guard
}

/// Whether the version 1 `member:{uid}` record answers for the primary tenant.
/// True before config is initialised (unit tests).
pub fn legacy_member_keys() -> bool {
    try_get().is_none_or(|config| config.legacy_member_keys && !config.valkey_tenant_scoped)
}

/// Whether every Valkey key this process uses stays under a served tenant
/// (`RUNTARA_VALKEY_TENANT_SCOPED`). False before config is initialised.
pub fn valkey_tenant_scoped() -> bool {
    try_get().is_some_and(|config| config.valkey_tenant_scoped)
}

/// Whether a tenant is served only while its status record says it is
/// `ready` (`RUNTARA_AUTH_TENANT_STATUS=required`): a provisioning plane, such
/// as Runtara Cloud's customer plane, writes `runtara:tenant_status:{tenant}`.
/// Off by default, where nothing writes it.
pub fn tenant_status_required() -> bool {
    static REQUIRED: OnceLock<bool> = OnceLock::new();
    *REQUIRED.get_or_init(|| {
        std::env::var("RUNTARA_AUTH_TENANT_STATUS").is_ok_and(|value| value == "required")
    })
}

/// The most database pools one tenant's connections may hold open.
pub fn object_model_tenant_pool_max() -> usize {
    get().object_model_tenant_pool_max
}

/// Get the maximum number of connections for the object model database pool.
/// Connection-pool tuning for the embedded runtime's database.
pub fn runtime_pool_config() -> RuntimePoolConfig {
    get().runtime_pool
}

/// Grace periods for the two stages of the shutdown drain.
pub fn shutdown_grace() -> ShutdownGrace {
    get().shutdown_grace
}

pub fn object_model_max_connections() -> u32 {
    get().object_model_max_connections
}

/// Connection-pool tuning for per-connection object-model PostgreSQL pools.
pub fn object_model_pool_config() -> runtara_object_store::PoolConfig {
    get().object_model_pool.clone()
}

/// Maximum number of distinct object-model store pools kept warm (LRU cap).
pub fn object_model_pool_cache_max() -> u64 {
    get().object_model_pool_cache_max
}

/// Idle TTL after which an unused object-model store pool is evicted.
pub fn object_model_pool_cache_ttl() -> std::time::Duration {
    std::time::Duration::from_secs(get().object_model_pool_cache_ttl_secs)
}

/// Whether the object model uses soft delete. True (the default) before
/// config is initialised (unit tests).
pub fn object_model_soft_delete() -> bool {
    try_get().is_none_or(|config| config.object_model_soft_delete)
}

/// Guard rails for caller-supplied raw SQL (workflow capabilities and the
/// runtime/MCP SQL routes).
pub fn raw_sql_guardrails() -> runtara_object_store::SqlGuardrails {
    get().raw_sql_guardrails
}

/// Maximum number of items accepted per bulk request (create/upsert/update-by-ids).
/// The store's default before config is initialised (unit tests).
pub fn object_model_bulk_request_limit() -> usize {
    let Some(config) = try_get() else {
        return runtara_object_store::DEFAULT_BULK_REQUEST_LIMIT;
    };
    crate::middleware::entitlement::effective_limit(
        config.object_model_bulk_request_limit,
        entitlements().limits.object_model_bulk_request_limit,
    )
}

/// Directory containing components used by direct WASM static composition.
pub fn direct_wasm_components_dir() -> Option<std::path::PathBuf> {
    get().direct_wasm_components_dir.clone()
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

/// The primary tenant's entitlement snapshot, built once at startup by
/// `Config::from_env()`. Work done for a tenant reads that tenant's with
/// [`entitlements_for`]; this is for process-level decisions (boot logs, plan
/// change detection, the SPA's inline config).
pub fn entitlements() -> &'static EntitlementSnapshot {
    &get().entitlement_snapshot
}

/// Where each tenant's entitlements come from. The default reads them from
/// the environment at boot; a host serving many tenants supplies its own
/// (`ServerOptions::entitlements`).
pub trait EntitlementSource: Send + Sync {
    fn entitlements(&self, tenant: &str) -> Arc<EntitlementSnapshot>;
}

static ENTITLEMENT_SOURCE: OnceLock<Arc<dyn EntitlementSource>> = OnceLock::new();

/// Read every tenant's entitlements from `source`. Set once, at boot.
pub fn install_entitlement_source(source: Arc<dyn EntitlementSource>) {
    let _ = ENTITLEMENT_SOURCE.set(source);
}

/// `tenant`'s entitlements: from the installed source, else the environment
/// (its `RUNTARA_TENANT_ENTITLEMENTS_JSON` entry, else the primary's).
pub fn entitlements_for(tenant: &str) -> Arc<EntitlementSnapshot> {
    match ENTITLEMENT_SOURCE.get() {
        Some(source) => source.entitlements(tenant),
        None => environment_entitlements_for(tenant),
    }
}

/// `tenant`'s entitlements as the environment gives them: its
/// `RUNTARA_TENANT_ENTITLEMENTS_JSON` entry, else the primary's. What an
/// installed source may fall back to.
pub fn environment_entitlements_for(tenant: &str) -> Arc<EntitlementSnapshot> {
    let config = get();
    let snapshot = config
        .tenant_entitlements
        .get(tenant)
        .or_else(|| config.tenant_entitlements.get(&config.tenant_id))
        .cloned()
        .unwrap_or_else(|| Arc::new(config.entitlement_snapshot.clone()));
    // A tenant inheriting the primary's entitlements still names itself.
    if snapshot.tenant_id == tenant {
        snapshot
    } else {
        let mut own = (*snapshot).clone();
        own.tenant_id = tenant.to_owned();
        Arc::new(own)
    }
}

/// `RUNTARA_TENANT_ENTITLEMENTS_JSON`: an object of served tenant to
/// `{"pricingTier", "entitlements", "overrides"}`, each optional and read as
/// `RUNTARA_PRICING_TIER`, `RUNTARA_ENTITLEMENTS_JSON` and
/// `RUNTARA_ENTITLEMENT_OVERRIDES_JSON` are for the primary.
fn tenant_entitlements_from_raw(
    raw: Option<&str>,
    served_tenants: &[String],
    registered_agents: &BTreeSet<String>,
) -> Result<std::collections::BTreeMap<String, Arc<EntitlementSnapshot>>, ConfigError> {
    let mut snapshots = std::collections::BTreeMap::new();
    let Some(raw) = raw.map(str::trim).filter(|raw| !raw.is_empty()) else {
        return Ok(snapshots);
    };
    let entries: std::collections::BTreeMap<String, serde_json::Value> = serde_json::from_str(raw)
        .map_err(|error| {
            ConfigError::InvalidValue("RUNTARA_TENANT_ENTITLEMENTS_JSON", error.to_string())
        })?;
    for (tenant, entry) in entries {
        if !served_tenants.contains(&tenant) {
            return Err(ConfigError::InvalidValue(
                "RUNTARA_TENANT_ENTITLEMENTS_JSON",
                format!("{tenant} is not a served tenant"),
            ));
        }
        let snapshot = tenant_entry(&tenant, &entry, registered_agents)?;
        snapshots.insert(tenant, Arc::new(snapshot));
    }
    Ok(snapshots)
}

/// One tenant's `{"pricingTier", "entitlements", "overrides"}` entry.
fn tenant_entry(
    tenant: &str,
    entry: &serde_json::Value,
    registered_agents: &BTreeSet<String>,
) -> Result<EntitlementSnapshot, ConfigError> {
    let field = |name: &str| entry.get(name).filter(|value| !value.is_null());
    let tier = field("pricingTier").and_then(serde_json::Value::as_str);
    let entitlements = field("entitlements").map(serde_json::Value::to_string);
    let overrides = field("overrides").map(serde_json::Value::to_string);
    EntitlementSnapshot::parse_entitlements(
        tenant,
        tier,
        entitlements.as_deref(),
        overrides.as_deref(),
        registered_agents,
    )
}

/// `tenant`'s entitlements from an entry shaped as in
/// `RUNTARA_TENANT_ENTITLEMENTS_JSON`, against this process's agents: how an
/// [`EntitlementSource`] turns a record it was given into a snapshot.
pub fn entitlements_from_entry(
    tenant: &str,
    entry: &serde_json::Value,
) -> Result<EntitlementSnapshot, ConfigError> {
    tenant_entry(
        tenant,
        entry,
        get().entitlement_snapshot.registered_agents(),
    )
}

/// Whether source admission counting uses Valkey instead of the
/// `execution_admission_tenants` row, via `RUNTARA_VALKEY_ADMISSION`
/// (default on when a Valkey is configured).
///
/// The database keeps the durable reservation rows either way; this only
/// chooses where the bound is counted.
pub fn valkey_admission_enabled() -> bool {
    std::env::var("RUNTARA_VALKEY_ADMISSION")
        .map(|v| !matches!(v.trim(), "0" | "false" | "FALSE" | "no"))
        .unwrap_or(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn production_lowering_identity_uses_standard_composition() {
        let actual = workflow_lowering_tag();
        assert_eq!(actual, runtara_workflows::direct_lowering_tag());
        assert!(actual.contains("agent-composition=standard-v1"));
        assert!(!actual.contains("isolation=v1-"));
    }

    /// The admission ceiling exists to bound work the runner will execute, so
    /// it must stay anchored to the runner's own bound. These drifted apart
    /// once -- admission defaulted to `num_cpus * 32` while the runner ran
    /// `num_cpus * 4` -- and the 8x gap surfaced in production as 403s with
    /// most of the budget held by requests that were only queued.
    #[test]
    fn admission_default_tracks_the_runner_bound() {
        let runs = runtara_environment::runner::embedded::max_concurrent_runs();
        let admission = default_max_concurrent_executions();

        assert_eq!(
            admission,
            runs * 2,
            "admission default must stay derived from the runner bound"
        );
        assert!(
            admission >= runs,
            "admission below the runner bound would starve the runner: \
             admission={admission} runs={runs}"
        );
    }

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
    fn statement_timeout_passes_through_below_instance_timeout() {
        assert_eq!(clamp_statement_timeout_ms(60_000, 300_000), 60_000);
    }

    #[test]
    fn statement_timeout_clamps_at_or_above_instance_timeout() {
        assert_eq!(clamp_statement_timeout_ms(300_000, 300_000), 299_999);
        assert_eq!(clamp_statement_timeout_ms(900_000, 300_000), 299_999);
        // Degenerate instance timeout never clamps to zero.
        assert_eq!(clamp_statement_timeout_ms(5, 1), 1);
    }

    #[test]
    fn raw_sql_knobs_reject_zero_and_garbage() {
        // parse_positive_u64_or is the parser for all three knobs: zero and
        // non-numeric values must fail boot, never mean "unbounded".
        let name = "RUNTARA_TEST_RAW_SQL_KNOB_UNIQUE_7";
        // SAFETY: only this test references this var.
        unsafe {
            std::env::set_var(name, "0");
        }
        assert!(parse_positive_u64_or(name, 10).is_err());
        unsafe {
            std::env::set_var(name, "ten");
        }
        assert!(parse_positive_u64_or(name, 10).is_err());
        unsafe {
            std::env::remove_var(name);
        }
        assert_eq!(parse_positive_u64_or(name, 10).unwrap(), 10);
    }

    #[test]
    fn execution_timeout_policy_uses_one_bounded_default() {
        let policy = execution_timeout_policy_from_raw(None, None).unwrap();

        assert_eq!(
            policy.default_timeout().as_secs(),
            DEFAULT_EXECUTION_TIMEOUT_SECS
        );
        assert_eq!(
            policy.maximum_timeout().as_secs(),
            MAX_EXECUTION_TIMEOUT_SECS
        );
    }

    #[test]
    fn execution_timeout_policy_rejects_invalid_and_unbounded_config() {
        assert!(execution_timeout_policy_from_raw(Some("0"), None).is_err());
        assert!(execution_timeout_policy_from_raw(Some("3601"), None).is_err());
        assert!(execution_timeout_policy_from_raw(Some("120"), Some("60")).is_err());
        assert!(execution_timeout_policy_from_raw(Some("120"), Some("3601")).is_err());
        assert!(execution_timeout_policy_from_raw(Some("one-minute"), None).is_err());

        let policy = execution_timeout_policy_from_raw(Some("120"), Some("300")).unwrap();
        assert_eq!(policy.default_timeout().as_secs(), 120);
        assert_eq!(policy.maximum_timeout().as_secs(), 300);
    }

    #[test]
    fn each_served_tenant_may_have_its_own_entitlements() {
        let served = ["a".to_string(), "b".to_string()];
        let agents = BTreeSet::new();
        assert!(
            tenant_entitlements_from_raw(None, &served, &agents)
                .unwrap()
                .is_empty()
        );
        let parsed = tenant_entitlements_from_raw(
            Some(r#"{"b": {"overrides": {"limits": {"maxConcurrentExecutions": 1}}}}"#),
            &served,
            &agents,
        )
        .unwrap();
        assert_eq!(parsed["b"].limits.max_concurrent_executions, Some(1));
        assert!(
            tenant_entitlements_from_raw(Some(r#"{"c": {}}"#), &served, &agents).is_err(),
            "only served tenants"
        );
    }

    #[test]
    fn a_source_parses_a_tenants_entry_against_this_processs_agents() {
        init_for_test();
        let entry = serde_json::json!({"overrides": {"limits": {"maxWorkflows": 1}}});
        let snapshot = entitlements_from_entry("t2", &entry).unwrap();
        assert_eq!(snapshot.tenant_id, "t2");
        assert_eq!(snapshot.limits.max_workflows, Some(1));
        let invalid = [
            serde_json::json!({"pricingTier": "gold"}),
            serde_json::json!({"overrides": {"limits": {"x": 1}}}),
        ];
        for entry in invalid {
            assert!(entitlements_from_entry("t2", &entry).is_err(), "{entry}");
        }
    }

    #[test]
    fn served_tenants_start_with_the_primary_and_add_the_listed_ones_once() {
        assert_eq!(served_tenants_from_raw("a", None), ["a"]);
        assert_eq!(
            served_tenants_from_raw("a", Some(" b, a ,,c,b ".into())),
            ["a", "b", "c"]
        );
    }

    #[test]
    fn direct_wasm_components_dir_prefers_explicit_dir() {
        let agent_dir = std::path::Path::new("/opt/runtara/agents");

        assert_eq!(
            direct_wasm_components_dir_from_raw(Some("/tmp/direct-components"), Some(agent_dir)),
            Some(std::path::PathBuf::from("/tmp/direct-components"))
        );
    }

    #[test]
    fn direct_wasm_components_dir_falls_back_to_agent_dir() {
        let agent_dir = std::path::Path::new("/opt/runtara/agents");

        assert_eq!(
            direct_wasm_components_dir_from_raw(Some("  "), Some(agent_dir)),
            Some(agent_dir.to_path_buf())
        );
        assert_eq!(
            direct_wasm_components_dir_from_raw(None, Some(agent_dir)),
            Some(agent_dir.to_path_buf())
        );
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
