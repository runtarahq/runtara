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
}

impl StoreConfig {
    /// Create a new configuration builder
    pub fn builder(database_url: impl Into<String>) -> StoreConfigBuilder {
        StoreConfigBuilder::new(database_url)
    }
}

/// Builder for StoreConfig
#[derive(Debug)]
pub struct StoreConfigBuilder {
    database_url: String,
    metadata_table: String,
    soft_delete: bool,
    auto_columns: AutoColumns,
}

impl StoreConfigBuilder {
    /// Create a new builder with the database URL
    pub fn new(database_url: impl Into<String>) -> Self {
        Self {
            database_url: database_url.into(),
            metadata_table: "__schema".to_string(),
            soft_delete: true,
            auto_columns: AutoColumns::default(),
        }
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
}
