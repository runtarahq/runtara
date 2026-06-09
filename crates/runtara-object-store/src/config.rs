//! Configuration for ObjectStore
//!
//! Provides a builder pattern for configuring the object store.

/// Configuration for auto-managed columns
#[derive(Debug, Clone)]
pub struct AutoColumns {
    /// Whether to include `id` column (UUID primary key)
    pub id: bool,
    /// Whether to include `created_at` column (timestamp)
    pub created_at: bool,
    /// Whether to include `updated_at` column (timestamp)
    pub updated_at: bool,
}

impl Default for AutoColumns {
    fn default() -> Self {
        Self {
            id: true,
            created_at: true,
            updated_at: true,
        }
    }
}

/// Default maximum number of connections per object-model PostgreSQL pool.
pub const DEFAULT_POOL_MAX_CONNECTIONS: u32 = 10;
/// Default minimum (warm) connections kept open per pool. Keeping at least one
/// avoids a cold cross-cloud handshake on the hot path.
pub const DEFAULT_POOL_MIN_CONNECTIONS: u32 = 1;
/// Default acquire timeout (seconds).
pub const DEFAULT_POOL_ACQUIRE_TIMEOUT_SECS: u64 = 10;
/// Default idle timeout (seconds). Recycle below typical Azure/PgBouncer cutoff.
pub const DEFAULT_POOL_IDLE_TIMEOUT_SECS: u64 = 300;
/// Default max connection lifetime (seconds).
pub const DEFAULT_POOL_MAX_LIFETIME_SECS: u64 = 1800;
/// Default for the per-acquire liveness ping. Disabled so a small query does not
/// pay an extra round-trip on a high-latency link; dead connections are bounded
/// by `idle_timeout`/`max_lifetime` staying under the server's idle cutoff.
pub const DEFAULT_POOL_TEST_BEFORE_ACQUIRE: bool = false;
/// Default prepared-statement cache capacity per connection (sqlx default).
pub const DEFAULT_POOL_STATEMENT_CACHE_CAPACITY: usize = 100;

/// Connection-pool tuning for an object-model PostgreSQL database.
///
/// These map directly onto sqlx's `PgPoolOptions`/`PgConnectOptions`. Defaults
/// are tuned for a cross-cloud link (our cloud → customer Azure): keep a warm
/// connection ([`DEFAULT_POOL_MIN_CONNECTIONS`]) and skip the per-acquire
/// liveness ping ([`DEFAULT_POOL_TEST_BEFORE_ACQUIRE`]). A `None`
/// `idle_timeout`/`max_lifetime` disables that timeout.
#[derive(Debug, Clone)]
pub struct PoolConfig {
    /// Maximum connections in the pool.
    pub max_connections: u32,
    /// Minimum (warm) connections the pool maintains.
    pub min_connections: u32,
    /// How long to wait for a free connection before erroring.
    pub acquire_timeout: std::time::Duration,
    /// Close a connection after it has been idle this long (`None` = never).
    pub idle_timeout: Option<std::time::Duration>,
    /// Close a connection after it has lived this long (`None` = never).
    pub max_lifetime: Option<std::time::Duration>,
    /// Ping a connection before handing it out. Off by default (see above).
    pub test_before_acquire: bool,
    /// Per-connection prepared-statement cache capacity.
    pub statement_cache_capacity: usize,
}

impl Default for PoolConfig {
    fn default() -> Self {
        Self {
            max_connections: DEFAULT_POOL_MAX_CONNECTIONS,
            min_connections: DEFAULT_POOL_MIN_CONNECTIONS,
            acquire_timeout: std::time::Duration::from_secs(DEFAULT_POOL_ACQUIRE_TIMEOUT_SECS),
            idle_timeout: Some(std::time::Duration::from_secs(
                DEFAULT_POOL_IDLE_TIMEOUT_SECS,
            )),
            max_lifetime: Some(std::time::Duration::from_secs(
                DEFAULT_POOL_MAX_LIFETIME_SECS,
            )),
            test_before_acquire: DEFAULT_POOL_TEST_BEFORE_ACQUIRE,
            statement_cache_capacity: DEFAULT_POOL_STATEMENT_CACHE_CAPACITY,
        }
    }
}

/// Configuration for the object store
#[derive(Debug, Clone)]
pub struct StoreConfig {
    /// PostgreSQL database URL
    pub database_url: String,
    /// Name of the metadata table (default: "__schema")
    pub metadata_table: String,
    /// Whether to use soft delete (deleted column) or hard delete
    pub soft_delete: bool,
    /// Auto-managed columns configuration
    pub auto_columns: AutoColumns,
    /// Maximum number of items accepted per bulk request that carries an
    /// explicit vector of items (create, upsert, update-by-ids). Condition-based
    /// bulk update/delete are not capped — the condition decides row count.
    pub bulk_request_limit: usize,
    /// Connection-pool tuning applied when this config builds its own pool
    /// (`ObjectStore::new`). Ignored by `ObjectStore::from_pool`.
    pub pool: PoolConfig,
}

impl StoreConfig {
    /// Create a new configuration builder
    pub fn builder(database_url: impl Into<String>) -> StoreConfigBuilder {
        StoreConfigBuilder::new(database_url)
    }
}

/// Default cap on the number of items accepted per bulk request.
pub const DEFAULT_BULK_REQUEST_LIMIT: usize = 10_000;

/// Default cap on the number of result rows returned by `aggregate_instances`.
/// If a caller omits `limit` and the natural result exceeds this cap, the
/// request is rejected so the caller must add an explicit `limit`.
pub const DEFAULT_AGGREGATE_RESULT_ROW_LIMIT: usize = 100_000;

/// Builder for StoreConfig
#[derive(Debug)]
pub struct StoreConfigBuilder {
    database_url: String,
    metadata_table: String,
    soft_delete: bool,
    auto_columns: AutoColumns,
    bulk_request_limit: usize,
    pool: PoolConfig,
}

impl StoreConfigBuilder {
    /// Create a new builder with the database URL
    pub fn new(database_url: impl Into<String>) -> Self {
        Self {
            database_url: database_url.into(),
            metadata_table: "__schema".to_string(),
            soft_delete: true,
            auto_columns: AutoColumns::default(),
            bulk_request_limit: DEFAULT_BULK_REQUEST_LIMIT,
            pool: PoolConfig::default(),
        }
    }

    /// Set the maximum number of items accepted per bulk request.
    ///
    /// Applies to methods that take an explicit Vec: `create_instances`,
    /// `upsert_instances`, `update_instances_by_ids`. A value of 0 is treated
    /// as the default ([`DEFAULT_BULK_REQUEST_LIMIT`]).
    pub fn bulk_request_limit(mut self, limit: usize) -> Self {
        self.bulk_request_limit = if limit == 0 {
            DEFAULT_BULK_REQUEST_LIMIT
        } else {
            limit
        };
        self
    }

    /// Set the connection-pool tuning used when the store builds its own pool.
    pub fn pool(mut self, pool: PoolConfig) -> Self {
        self.pool = pool;
        self
    }

    /// Set the metadata table name (default: "__schema")
    pub fn metadata_table(mut self, name: impl Into<String>) -> Self {
        self.metadata_table = name.into();
        self
    }

    /// Enable or disable soft delete (default: true)
    pub fn soft_delete(mut self, enabled: bool) -> Self {
        self.soft_delete = enabled;
        self
    }

    /// Enable or disable auto-generated `id` column (default: true)
    pub fn auto_id(mut self, enabled: bool) -> Self {
        self.auto_columns.id = enabled;
        self
    }

    /// Enable or disable auto-generated `created_at` column (default: true)
    pub fn auto_created_at(mut self, enabled: bool) -> Self {
        self.auto_columns.created_at = enabled;
        self
    }

    /// Enable or disable auto-generated `updated_at` column (default: true)
    pub fn auto_updated_at(mut self, enabled: bool) -> Self {
        self.auto_columns.updated_at = enabled;
        self
    }

    /// Disable the auto-generated `id` column
    pub fn without_id(mut self) -> Self {
        self.auto_columns.id = false;
        self
    }

    /// Disable the auto-generated `created_at` column
    pub fn without_created_at(mut self) -> Self {
        self.auto_columns.created_at = false;
        self
    }

    /// Disable the auto-generated `updated_at` column
    pub fn without_updated_at(mut self) -> Self {
        self.auto_columns.updated_at = false;
        self
    }

    /// Disable all auto-managed columns
    pub fn without_auto_columns(mut self) -> Self {
        self.auto_columns = AutoColumns {
            id: false,
            created_at: false,
            updated_at: false,
        };
        self
    }

    /// Build the configuration
    pub fn build(self) -> StoreConfig {
        StoreConfig {
            database_url: self.database_url,
            metadata_table: self.metadata_table,
            soft_delete: self.soft_delete,
            auto_columns: self.auto_columns,
            bulk_request_limit: self.bulk_request_limit,
            pool: self.pool,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // =========================================================================
    // AutoColumns Tests
    // =========================================================================

    #[test]
    fn test_auto_columns_default() {
        let ac = AutoColumns::default();
        assert!(ac.id);
        assert!(ac.created_at);
        assert!(ac.updated_at);
    }

    // =========================================================================
    // StoreConfig Default Tests
    // =========================================================================

    #[test]
    fn test_default_config() {
        let config = StoreConfig::builder("postgres://localhost/test").build();

        assert_eq!(config.database_url, "postgres://localhost/test");
        assert_eq!(config.metadata_table, "__schema");
        assert!(config.soft_delete);
        assert!(config.auto_columns.id);
        assert!(config.auto_columns.created_at);
        assert!(config.auto_columns.updated_at);
    }

    #[test]
    fn test_builder_accepts_string() {
        let config = StoreConfig::builder(String::from("postgres://localhost/db")).build();
        assert_eq!(config.database_url, "postgres://localhost/db");
    }

    #[test]
    fn test_builder_accepts_str() {
        let config = StoreConfig::builder("postgres://localhost/db").build();
        assert_eq!(config.database_url, "postgres://localhost/db");
    }

    // =========================================================================
    // Metadata Table Configuration Tests
    // =========================================================================

    #[test]
    fn test_custom_metadata_table() {
        let config = StoreConfig::builder("postgres://localhost/test")
            .metadata_table("_metadata")
            .build();

        assert_eq!(config.metadata_table, "_metadata");
    }

    #[test]
    fn test_metadata_table_accepts_string() {
        let config = StoreConfig::builder("postgres://localhost/test")
            .metadata_table(String::from("custom_schema"))
            .build();

        assert_eq!(config.metadata_table, "custom_schema");
    }

    // =========================================================================
    // Soft Delete Configuration Tests
    // =========================================================================

    #[test]
    fn test_soft_delete_enabled_by_default() {
        let config = StoreConfig::builder("postgres://localhost/test").build();
        assert!(config.soft_delete);
    }

    #[test]
    fn test_soft_delete_disabled() {
        let config = StoreConfig::builder("postgres://localhost/test")
            .soft_delete(false)
            .build();

        assert!(!config.soft_delete);
    }

    #[test]
    fn test_soft_delete_explicit_enable() {
        let config = StoreConfig::builder("postgres://localhost/test")
            .soft_delete(true)
            .build();

        assert!(config.soft_delete);
    }

    // =========================================================================
    // Auto Columns Configuration Tests
    // =========================================================================

    #[test]
    fn test_auto_id_disabled() {
        let config = StoreConfig::builder("postgres://localhost/test")
            .auto_id(false)
            .build();

        assert!(!config.auto_columns.id);
        assert!(config.auto_columns.created_at);
        assert!(config.auto_columns.updated_at);
    }

    #[test]
    fn test_auto_created_at_disabled() {
        let config = StoreConfig::builder("postgres://localhost/test")
            .auto_created_at(false)
            .build();

        assert!(config.auto_columns.id);
        assert!(!config.auto_columns.created_at);
        assert!(config.auto_columns.updated_at);
    }

    #[test]
    fn test_auto_updated_at_disabled() {
        let config = StoreConfig::builder("postgres://localhost/test")
            .auto_updated_at(false)
            .build();

        assert!(config.auto_columns.id);
        assert!(config.auto_columns.created_at);
        assert!(!config.auto_columns.updated_at);
    }

    #[test]
    fn test_without_id() {
        let config = StoreConfig::builder("postgres://localhost/test")
            .without_id()
            .build();

        assert!(!config.auto_columns.id);
    }

    #[test]
    fn test_without_created_at() {
        let config = StoreConfig::builder("postgres://localhost/test")
            .without_created_at()
            .build();

        assert!(!config.auto_columns.created_at);
    }

    #[test]
    fn test_without_updated_at() {
        let config = StoreConfig::builder("postgres://localhost/test")
            .without_updated_at()
            .build();

        assert!(!config.auto_columns.updated_at);
    }

    #[test]
    fn test_without_auto_columns() {
        let config = StoreConfig::builder("postgres://localhost/test")
            .without_auto_columns()
            .build();

        assert!(!config.auto_columns.id);
        assert!(!config.auto_columns.created_at);
        assert!(!config.auto_columns.updated_at);
    }

    // =========================================================================
    // Chained Builder Tests
    // =========================================================================

    #[test]
    fn test_full_custom_config() {
        let config = StoreConfig::builder("postgres://localhost/test")
            .metadata_table("_metadata")
            .soft_delete(false)
            .auto_id(false)
            .auto_created_at(false)
            .auto_updated_at(false)
            .build();

        assert_eq!(config.database_url, "postgres://localhost/test");
        assert_eq!(config.metadata_table, "_metadata");
        assert!(!config.soft_delete);
        assert!(!config.auto_columns.id);
        assert!(!config.auto_columns.created_at);
        assert!(!config.auto_columns.updated_at);
    }

    #[test]
    fn test_builder_order_independence() {
        // Order of builder calls should not matter
        let config1 = StoreConfig::builder("postgres://localhost/test")
            .soft_delete(false)
            .metadata_table("custom")
            .build();

        let config2 = StoreConfig::builder("postgres://localhost/test")
            .metadata_table("custom")
            .soft_delete(false)
            .build();

        assert_eq!(config1.metadata_table, config2.metadata_table);
        assert_eq!(config1.soft_delete, config2.soft_delete);
    }

    // =========================================================================
    // Debug Trait Tests
    // =========================================================================

    #[test]
    fn test_config_debug() {
        let config = StoreConfig::builder("postgres://localhost/test").build();
        let debug_str = format!("{:?}", config);
        assert!(debug_str.contains("StoreConfig"));
        assert!(debug_str.contains("database_url"));
    }

    #[test]
    fn test_builder_debug() {
        let builder = StoreConfig::builder("postgres://localhost/test");
        let debug_str = format!("{:?}", builder);
        assert!(debug_str.contains("StoreConfigBuilder"));
    }

    // =========================================================================
    // Clone Trait Tests
    // =========================================================================

    #[test]
    fn test_config_clone() {
        let config1 = StoreConfig::builder("postgres://localhost/test")
            .metadata_table("custom")
            .soft_delete(false)
            .build();

        let config2 = config1.clone();

        assert_eq!(config1.database_url, config2.database_url);
        assert_eq!(config1.metadata_table, config2.metadata_table);
        assert_eq!(config1.soft_delete, config2.soft_delete);
    }

    #[test]
    fn test_auto_columns_clone() {
        let ac1 = AutoColumns::default();
        let ac2 = ac1.clone();

        assert_eq!(ac1.id, ac2.id);
        assert_eq!(ac1.created_at, ac2.created_at);
        assert_eq!(ac1.updated_at, ac2.updated_at);
    }

    // =========================================================================
    // Pool Configuration Tests
    // =========================================================================

    #[test]
    fn test_pool_config_defaults() {
        let pool = PoolConfig::default();
        assert_eq!(pool.max_connections, DEFAULT_POOL_MAX_CONNECTIONS);
        assert_eq!(pool.min_connections, DEFAULT_POOL_MIN_CONNECTIONS);
        // Warm spare kept, per-acquire ping disabled — the cross-cloud tuning.
        assert!(pool.min_connections >= 1);
        assert!(!pool.test_before_acquire);
        assert_eq!(
            pool.idle_timeout,
            Some(std::time::Duration::from_secs(
                DEFAULT_POOL_IDLE_TIMEOUT_SECS
            ))
        );
        assert_eq!(
            pool.max_lifetime,
            Some(std::time::Duration::from_secs(
                DEFAULT_POOL_MAX_LIFETIME_SECS
            ))
        );
    }

    #[test]
    fn test_store_config_default_pool() {
        let config = StoreConfig::builder("postgres://localhost/test").build();
        assert_eq!(config.pool.min_connections, DEFAULT_POOL_MIN_CONNECTIONS);
        assert!(!config.pool.test_before_acquire);
    }

    #[test]
    fn test_store_config_custom_pool() {
        let custom = PoolConfig {
            max_connections: 42,
            min_connections: 7,
            test_before_acquire: true,
            ..PoolConfig::default()
        };
        let config = StoreConfig::builder("postgres://localhost/test")
            .pool(custom)
            .build();
        assert_eq!(config.pool.max_connections, 42);
        assert_eq!(config.pool.min_connections, 7);
        assert!(config.pool.test_before_acquire);
    }
}
