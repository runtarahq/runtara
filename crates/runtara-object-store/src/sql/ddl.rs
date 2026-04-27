//! DDL Generation for Dynamic Schema Management
//!
//! Generates PostgreSQL DDL statements for dynamically managing object model tables.

use crate::config::StoreConfig;
use crate::sql::sanitize::quote_identifier;
use crate::types::{ColumnDefinition, IndexDefinition};

/// DDL Generator for object model tables
pub struct DdlGenerator<'a> {
    config: &'a StoreConfig,
}

impl<'a> DdlGenerator<'a> {
    /// Create a new DDL generator with the given configuration
    pub fn new(config: &'a StoreConfig) -> Self {
        Self { config }
    }

    /// Generate CREATE TABLE statement with auto-managed columns
    ///
    /// Creates a table with:
    /// - User-defined columns
    /// - Auto-managed columns based on config: id, created_at, updated_at
    /// - `deleted` tombstone column (always present; the runtime soft_delete
    ///   flag decides whether deletes flip the flag or issue a hard DELETE)
    pub fn generate_create_table(&self, table_name: &str, columns: &[ColumnDefinition]) -> String {
        let quoted_table = quote_identifier(table_name);

        let mut column_defs = Vec::new();

        // Add auto-managed id column if enabled
        if self.config.auto_columns.id {
            column_defs
                .push("id VARCHAR(255) PRIMARY KEY DEFAULT gen_random_uuid()::text".to_string());
        }

        // Add user-defined columns
        for col in columns {
            column_defs.push(Self::format_column_definition(col));
        }

        // Add auto-managed timestamp columns if enabled
        // Use TIMESTAMPTZ to match Rust's chrono::DateTime<Utc>
        if self.config.auto_columns.created_at {
            column_defs.push("created_at TIMESTAMPTZ DEFAULT NOW()".to_string());
        }
        if self.config.auto_columns.updated_at {
            column_defs.push("updated_at TIMESTAMPTZ DEFAULT NOW()".to_string());
        }

        column_defs.push("deleted BOOLEAN DEFAULT FALSE".to_string());

        format!("CREATE TABLE {} ({})", quoted_table, column_defs.join(", "))
    }

    /// Generate ALTER TABLE statements to modify table structure
    pub fn generate_alter_table(
        &self,
        table_name: &str,
        old_columns: &[ColumnDefinition],
        new_columns: &[ColumnDefinition],
    ) -> Vec<String> {
        let quoted_table = quote_identifier(table_name);
        let mut statements = Vec::new();

        // Find added columns
        for new_col in new_columns {
            if !old_columns.iter().any(|c| c.name == new_col.name) {
                statements.push(format!(
                    "ALTER TABLE {} ADD COLUMN {}",
                    quoted_table,
                    Self::format_column_definition(new_col)
                ));
                // If the new column wants a trigram index, emit the partial
                // GIN/`gin_trgm_ops` index alongside the ADD COLUMN statement.
                if new_col.requires_trigram_index() {
                    statements.push(Self::trigram_index_create(table_name, &new_col.name));
                }
                // tsvector columns get a GIN index automatically.
                if matches!(
                    new_col.column_type,
                    crate::types::ColumnType::Tsvector { .. }
                ) {
                    statements.push(Self::tsvector_index_create(table_name, &new_col.name));
                }
            }
        }

        // Find dropped columns
        for old_col in old_columns {
            if !new_columns.iter().any(|c| c.name == old_col.name) {
                if old_col.requires_trigram_index() {
                    statements.push(Self::trigram_index_drop(table_name, &old_col.name));
                }
                if matches!(
                    old_col.column_type,
                    crate::types::ColumnType::Tsvector { .. }
                ) {
                    statements.push(Self::tsvector_index_drop(table_name, &old_col.name));
                }
                statements.push(format!(
                    "ALTER TABLE {} DROP COLUMN {}",
                    quoted_table,
                    quote_identifier(&old_col.name)
                ));
            }
        }

        // Find modified columns
        for new_col in new_columns {
            if let Some(old_col) = old_columns.iter().find(|c| c.name == new_col.name) {
                // Type change
                if old_col.column_type != new_col.column_type {
                    statements.push(format!(
                        "ALTER TABLE {} ALTER COLUMN {} TYPE {}",
                        quoted_table,
                        quote_identifier(&new_col.name),
                        new_col.column_type.to_sql_type(&new_col.name)
                    ));
                }

                // Nullable change
                if old_col.nullable != new_col.nullable {
                    let constraint = if new_col.nullable {
                        "DROP NOT NULL"
                    } else {
                        "SET NOT NULL"
                    };
                    statements.push(format!(
                        "ALTER TABLE {} ALTER COLUMN {} {}",
                        quoted_table,
                        quote_identifier(&new_col.name),
                        constraint
                    ));
                }

                // Default value change
                if old_col.default_value != new_col.default_value {
                    if let Some(default) = &new_col.default_value {
                        statements.push(format!(
                            "ALTER TABLE {} ALTER COLUMN {} SET DEFAULT {}",
                            quoted_table,
                            quote_identifier(&new_col.name),
                            default
                        ));
                    } else {
                        statements.push(format!(
                            "ALTER TABLE {} ALTER COLUMN {} DROP DEFAULT",
                            quoted_table,
                            quote_identifier(&new_col.name)
                        ));
                    }
                }
            }
        }

        statements
    }

    /// Generate DROP TABLE statement
    pub fn generate_drop_table(&self, table_name: &str) -> String {
        let quoted_table = quote_identifier(table_name);
        format!("DROP TABLE IF EXISTS {} CASCADE", quoted_table)
    }

    /// Generate CREATE INDEX statement
    pub fn generate_create_index(&self, table_name: &str, index: &IndexDefinition) -> String {
        let quoted_table = quote_identifier(table_name);
        let quoted_index_name = quote_identifier(&format!("{}_{}", table_name, index.name));

        let quoted_columns: Vec<String> = index
            .columns
            .iter()
            .map(|col| quote_identifier(col))
            .collect();

        let unique_clause = if index.unique { "UNIQUE " } else { "" };

        format!(
            "CREATE {}INDEX {} ON {}({})",
            unique_clause,
            quoted_index_name,
            quoted_table,
            quoted_columns.join(", ")
        )
    }

    /// Generate default index for efficient querying
    ///
    /// Partial index on created_at that excludes tombstoned rows — since reads
    /// always filter `deleted = FALSE`, this keeps the common path fast.
    pub fn generate_default_index(&self, table_name: &str) -> String {
        let quoted_table = quote_identifier(table_name);
        let index_name = format!("idx_{}_default", table_name);
        let quoted_index = quote_identifier(&index_name);

        format!(
            "CREATE INDEX {} ON {}(created_at DESC) WHERE deleted = FALSE",
            quoted_index, quoted_table
        )
    }

    /// Emit `CREATE INDEX … USING GIN … gin_trgm_ops` statements for every
    /// column annotated with `text_index = trigram`. Empty if no column wants
    /// trigram indexing.
    pub fn generate_trigram_indexes(
        &self,
        table_name: &str,
        columns: &[ColumnDefinition],
    ) -> Vec<String> {
        columns
            .iter()
            .filter(|c| c.requires_trigram_index())
            .map(|c| Self::trigram_index_create(table_name, &c.name))
            .collect()
    }

    fn trigram_index_create(table_name: &str, column: &str) -> String {
        let quoted_table = quote_identifier(table_name);
        let quoted_column = quote_identifier(column);
        let index_name = format!("idx_{}_{}_trgm", table_name, column);
        let quoted_index = quote_identifier(&index_name);
        format!(
            "CREATE INDEX IF NOT EXISTS {} ON {} USING GIN ({} gin_trgm_ops) \
             WHERE deleted = FALSE",
            quoted_index, quoted_table, quoted_column
        )
    }

    fn trigram_index_drop(table_name: &str, column: &str) -> String {
        let index_name = format!("idx_{}_{}_trgm", table_name, column);
        let quoted_index = quote_identifier(&index_name);
        format!("DROP INDEX IF EXISTS {}", quoted_index)
    }

    /// Emit a `CREATE INDEX … USING GIN (col)` for every `tsvector`-typed
    /// column. tsvector columns are useless without a GIN index — full-text
    /// queries fall back to seq scans otherwise.
    pub fn generate_tsvector_indexes(
        &self,
        table_name: &str,
        columns: &[ColumnDefinition],
    ) -> Vec<String> {
        columns
            .iter()
            .filter(|c| matches!(c.column_type, crate::types::ColumnType::Tsvector { .. }))
            .map(|c| Self::tsvector_index_create(table_name, &c.name))
            .collect()
    }

    fn tsvector_index_create(table_name: &str, column: &str) -> String {
        let quoted_table = quote_identifier(table_name);
        let quoted_column = quote_identifier(column);
        let index_name = format!("idx_{}_{}_fts", table_name, column);
        let quoted_index = quote_identifier(&index_name);
        format!(
            "CREATE INDEX IF NOT EXISTS {} ON {} USING GIN ({}) \
             WHERE deleted = FALSE",
            quoted_index, quoted_table, quoted_column
        )
    }

    fn tsvector_index_drop(table_name: &str, column: &str) -> String {
        let index_name = format!("idx_{}_{}_fts", table_name, column);
        let quoted_index = quote_identifier(&index_name);
        format!("DROP INDEX IF EXISTS {}", quoted_index)
    }

    /// Format a single column definition for CREATE TABLE or ALTER TABLE ADD COLUMN
    pub fn format_column_definition(col: &ColumnDefinition) -> String {
        let mut parts = vec![
            quote_identifier(&col.name),
            col.column_type.to_sql_type(&col.name),
        ];

        // UNIQUE constraint
        if col.unique {
            parts.push("UNIQUE".to_string());
        }

        // NOT NULL constraint
        if !col.nullable {
            parts.push("NOT NULL".to_string());
        }

        // Generated-column expression for tsvector columns. Mutually exclusive
        // with DEFAULT (Postgres rejects both on the same column).
        if let crate::types::ColumnType::Tsvector {
            source_column,
            language,
        } = &col.column_type
        {
            // Single quotes inside the language config are escaped defensively.
            let lang_lit = language.replace('\'', "''");
            parts.push(format!(
                "GENERATED ALWAYS AS (to_tsvector('{}', coalesce({}, ''))) STORED",
                lang_lit,
                quote_identifier(source_column)
            ));
        } else if let Some(default) = &col.default_value {
            // DEFAULT value
            parts.push(format!("DEFAULT {}", default));
        }

        parts.join(" ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ColumnType;

    // ==================== Test Configuration Helpers ====================

    fn default_config() -> StoreConfig {
        StoreConfig::builder("postgres://localhost/test").build()
    }

    fn config_hard_delete() -> StoreConfig {
        StoreConfig::builder("postgres://localhost/test")
            .soft_delete(false)
            .build()
    }

    fn config_no_auto_columns() -> StoreConfig {
        StoreConfig::builder("postgres://localhost/test")
            .auto_id(false)
            .auto_created_at(false)
            .auto_updated_at(false)
            .build()
    }

    fn config_only_id() -> StoreConfig {
        StoreConfig::builder("postgres://localhost/test")
            .auto_id(true)
            .auto_created_at(false)
            .auto_updated_at(false)
            .build()
    }

    fn config_only_timestamps() -> StoreConfig {
        StoreConfig::builder("postgres://localhost/test")
            .auto_id(false)
            .auto_created_at(true)
            .auto_updated_at(true)
            .build()
    }

    // ==================== CREATE TABLE Tests ====================

    #[test]
    fn test_generate_create_table_with_defaults() {
        let config = default_config();
        let generator = DdlGenerator::new(&config);

        let columns = vec![
            ColumnDefinition::new("sku", ColumnType::String)
                .unique()
                .not_null(),
            ColumnDefinition::new("price", ColumnType::decimal(10, 2)).default("0.00"),
        ];

        let ddl = generator.generate_create_table("products", &columns);

        assert!(ddl.contains("CREATE TABLE"));
        assert!(ddl.contains("\"products\""));
        assert!(ddl.contains("id VARCHAR(255) PRIMARY KEY"));
        assert!(ddl.contains("\"sku\" TEXT UNIQUE NOT NULL"));
        assert!(ddl.contains("\"price\" NUMERIC(10,2) DEFAULT 0.00"));
        assert!(ddl.contains("created_at TIMESTAMPTZ"));
        assert!(ddl.contains("updated_at TIMESTAMPTZ"));
        assert!(ddl.contains("deleted BOOLEAN"));
    }

    #[test]
    fn test_generate_create_table_always_has_deleted_column() {
        // Deleted column is always emitted — the soft_delete flag controls
        // runtime delete behavior, not schema shape.
        let config = config_hard_delete();
        let generator = DdlGenerator::new(&config);

        let columns = vec![ColumnDefinition::new("name", ColumnType::String)];

        let ddl = generator.generate_create_table("items", &columns);

        assert!(ddl.contains("id VARCHAR(255) PRIMARY KEY"));
        assert!(ddl.contains("created_at TIMESTAMPTZ"));
        assert!(ddl.contains("updated_at TIMESTAMPTZ"));
        assert!(ddl.contains("deleted BOOLEAN"));
    }

    #[test]
    fn test_generate_create_table_no_auto_columns() {
        let config = config_no_auto_columns();
        let generator = DdlGenerator::new(&config);

        let columns = vec![
            ColumnDefinition::new("id", ColumnType::String).not_null(),
            ColumnDefinition::new("name", ColumnType::String),
        ];

        let ddl = generator.generate_create_table("custom", &columns);

        // Should NOT have auto-generated id
        assert!(!ddl.contains("id VARCHAR(255) PRIMARY KEY DEFAULT gen_random_uuid()"));
        // Should have user-defined id
        assert!(ddl.contains("\"id\" TEXT NOT NULL"));
        assert!(!ddl.contains("created_at"));
        assert!(!ddl.contains("updated_at"));
        // `deleted` is always present
        assert!(ddl.contains("deleted BOOLEAN"));
    }

    #[test]
    fn test_generate_create_table_only_auto_id() {
        let config = config_only_id();
        let generator = DdlGenerator::new(&config);

        let columns = vec![ColumnDefinition::new("name", ColumnType::String)];

        let ddl = generator.generate_create_table("items", &columns);

        assert!(ddl.contains("id VARCHAR(255) PRIMARY KEY"));
        assert!(!ddl.contains("created_at"));
        assert!(!ddl.contains("updated_at"));
        assert!(ddl.contains("deleted BOOLEAN"));
    }

    #[test]
    fn test_generate_create_table_only_timestamps() {
        let config = config_only_timestamps();
        let generator = DdlGenerator::new(&config);

        let columns = vec![ColumnDefinition::new("name", ColumnType::String)];

        let ddl = generator.generate_create_table("items", &columns);

        assert!(!ddl.contains("id VARCHAR(255) PRIMARY KEY DEFAULT"));
        assert!(ddl.contains("created_at TIMESTAMPTZ"));
        assert!(ddl.contains("updated_at TIMESTAMPTZ"));
        assert!(ddl.contains("deleted BOOLEAN"));
    }

    #[test]
    fn test_generate_create_table_empty_columns() {
        let config = default_config();
        let generator = DdlGenerator::new(&config);

        let columns: Vec<ColumnDefinition> = vec![];

        let ddl = generator.generate_create_table("empty_table", &columns);

        // Should still have auto-managed columns
        assert!(ddl.contains("id VARCHAR(255) PRIMARY KEY"));
        assert!(ddl.contains("created_at TIMESTAMPTZ"));
        assert!(ddl.contains("updated_at TIMESTAMPTZ"));
        assert!(ddl.contains("deleted BOOLEAN"));
    }

    #[test]
    fn test_generate_create_table_all_column_types() {
        let config = config_no_auto_columns();
        let generator = DdlGenerator::new(&config);

        let columns = vec![
            ColumnDefinition::new("str_col", ColumnType::String),
            ColumnDefinition::new("int_col", ColumnType::Integer),
            ColumnDefinition::new("bool_col", ColumnType::Boolean),
            ColumnDefinition::new("json_col", ColumnType::Json),
            ColumnDefinition::new("dec_col", ColumnType::decimal(18, 4)),
            ColumnDefinition::new("ts_col", ColumnType::Timestamp),
        ];

        let ddl = generator.generate_create_table("all_types", &columns);

        assert!(ddl.contains("\"str_col\" TEXT"));
        assert!(ddl.contains("\"int_col\" BIGINT"));
        assert!(ddl.contains("\"bool_col\" BOOLEAN"));
        assert!(ddl.contains("\"json_col\" JSONB"));
        assert!(ddl.contains("\"dec_col\" NUMERIC(18,4)"));
        assert!(ddl.contains("\"ts_col\" TIMESTAMP"));
    }

    #[test]
    fn test_generate_create_table_with_constraints() {
        let config = config_no_auto_columns();
        let generator = DdlGenerator::new(&config);

        let columns = vec![
            ColumnDefinition::new("email", ColumnType::String)
                .unique()
                .not_null(),
            ColumnDefinition::new("status", ColumnType::String)
                .not_null()
                .default("'active'"),
            ColumnDefinition::new("notes", ColumnType::String), // Nullable by default
        ];

        let ddl = generator.generate_create_table("users", &columns);

        assert!(ddl.contains("\"email\" TEXT UNIQUE NOT NULL"));
        assert!(ddl.contains("\"status\" TEXT NOT NULL DEFAULT 'active'"));
        assert!(ddl.contains("\"notes\" TEXT")); // No NOT NULL
    }

    #[test]
    fn test_generate_create_table_special_table_name() {
        let config = config_no_auto_columns();
        let generator = DdlGenerator::new(&config);

        let columns = vec![ColumnDefinition::new("data", ColumnType::Json)];

        // Table name with reserved word
        let ddl = generator.generate_create_table("order", &columns);
        assert!(ddl.contains("CREATE TABLE \"order\""));

        // Table name needing quotes
        let ddl = generator.generate_create_table("user-data", &columns);
        assert!(ddl.contains("CREATE TABLE \"user-data\""));
    }

    // ==================== DROP TABLE Tests ====================

    #[test]
    fn test_generate_drop_table() {
        let config = default_config();
        let generator = DdlGenerator::new(&config);

        let ddl = generator.generate_drop_table("products");

        assert_eq!(ddl, "DROP TABLE IF EXISTS \"products\" CASCADE");
    }

    #[test]
    fn test_generate_drop_table_special_name() {
        let config = default_config();
        let generator = DdlGenerator::new(&config);

        let ddl = generator.generate_drop_table("user-orders");

        assert_eq!(ddl, "DROP TABLE IF EXISTS \"user-orders\" CASCADE");
    }

    // ==================== CREATE INDEX Tests ====================

    #[test]
    fn test_generate_create_index() {
        let config = default_config();
        let generator = DdlGenerator::new(&config);

        let index = IndexDefinition::new("sku_idx", vec!["sku".to_string()]).unique();

        let ddl = generator.generate_create_index("products", &index);

        assert_eq!(
            ddl,
            "CREATE UNIQUE INDEX \"products_sku_idx\" ON \"products\"(\"sku\")"
        );
    }

    #[test]
    fn test_generate_create_index_non_unique() {
        let config = default_config();
        let generator = DdlGenerator::new(&config);

        let index = IndexDefinition::new("status_idx", vec!["status".to_string()]);

        let ddl = generator.generate_create_index("orders", &index);

        assert_eq!(
            ddl,
            "CREATE INDEX \"orders_status_idx\" ON \"orders\"(\"status\")"
        );
    }

    #[test]
    fn test_generate_create_index_multi_column() {
        let config = default_config();
        let generator = DdlGenerator::new(&config);

        let index = IndexDefinition::new(
            "composite_idx",
            vec![
                "tenant".to_string(),
                "status".to_string(),
                "created_at".to_string(),
            ],
        );

        let ddl = generator.generate_create_index("tasks", &index);

        assert!(ddl.contains("CREATE INDEX"));
        assert!(ddl.contains("\"tenant\", \"status\", \"created_at\""));
    }

    #[test]
    fn test_generate_default_index_is_always_partial() {
        // Partial index always excludes tombstones, regardless of the runtime
        // soft_delete flag — reads always filter `deleted = FALSE`.
        for config in [default_config(), config_hard_delete()] {
            let generator = DdlGenerator::new(&config);
            let ddl = generator.generate_default_index("products");
            assert_eq!(
                ddl,
                "CREATE INDEX \"idx_products_default\" ON \"products\"(created_at DESC) WHERE deleted = FALSE"
            );
        }
    }

    // ==================== ALTER TABLE Tests ====================

    #[test]
    fn test_generate_alter_table_add_column() {
        let config = default_config();
        let generator = DdlGenerator::new(&config);

        let old_columns = vec![ColumnDefinition::new("name", ColumnType::String)];
        let new_columns = vec![
            ColumnDefinition::new("name", ColumnType::String),
            ColumnDefinition::new("description", ColumnType::String),
        ];

        let statements = generator.generate_alter_table("products", &old_columns, &new_columns);

        assert_eq!(statements.len(), 1);
        assert!(statements[0].contains("ADD COLUMN"));
        assert!(statements[0].contains("\"description\""));
    }

    #[test]
    fn test_generate_alter_table_add_multiple_columns() {
        let config = default_config();
        let generator = DdlGenerator::new(&config);

        let old_columns = vec![ColumnDefinition::new("name", ColumnType::String)];
        let new_columns = vec![
            ColumnDefinition::new("name", ColumnType::String),
            ColumnDefinition::new("description", ColumnType::String),
            ColumnDefinition::new("price", ColumnType::decimal(10, 2)),
            ColumnDefinition::new("active", ColumnType::Boolean),
        ];

        let statements = generator.generate_alter_table("products", &old_columns, &new_columns);

        assert_eq!(statements.len(), 3); // 3 new columns
        assert!(statements.iter().all(|s| s.contains("ADD COLUMN")));
    }

    #[test]
    fn test_generate_alter_table_drop_column() {
        let config = default_config();
        let generator = DdlGenerator::new(&config);

        let old_columns = vec![
            ColumnDefinition::new("name", ColumnType::String),
            ColumnDefinition::new("obsolete", ColumnType::String),
        ];
        let new_columns = vec![ColumnDefinition::new("name", ColumnType::String)];

        let statements = generator.generate_alter_table("products", &old_columns, &new_columns);

        assert_eq!(statements.len(), 1);
        assert!(statements[0].contains("DROP COLUMN"));
        assert!(statements[0].contains("\"obsolete\""));
    }

    #[test]
    fn test_generate_alter_table_drop_multiple_columns() {
        let config = default_config();
        let generator = DdlGenerator::new(&config);

        let old_columns = vec![
            ColumnDefinition::new("name", ColumnType::String),
            ColumnDefinition::new("old1", ColumnType::String),
            ColumnDefinition::new("old2", ColumnType::Integer),
        ];
        let new_columns = vec![ColumnDefinition::new("name", ColumnType::String)];

        let statements = generator.generate_alter_table("products", &old_columns, &new_columns);

        assert_eq!(statements.len(), 2);
        assert!(statements.iter().all(|s| s.contains("DROP COLUMN")));
    }

    #[test]
    fn test_generate_alter_table_change_type() {
        let config = default_config();
        let generator = DdlGenerator::new(&config);

        let old_columns = vec![ColumnDefinition::new("count", ColumnType::Integer)];
        let new_columns = vec![ColumnDefinition::new("count", ColumnType::decimal(10, 2))];

        let statements = generator.generate_alter_table("items", &old_columns, &new_columns);

        assert_eq!(statements.len(), 1);
        assert!(statements[0].contains("ALTER COLUMN"));
        assert!(statements[0].contains("TYPE"));
        assert!(statements[0].contains("NUMERIC(10,2)"));
    }

    #[test]
    fn test_generate_alter_table_change_nullable() {
        let config = default_config();
        let generator = DdlGenerator::new(&config);

        // Make column NOT NULL
        let old_columns = vec![ColumnDefinition::new("email", ColumnType::String)];
        let new_columns = vec![ColumnDefinition::new("email", ColumnType::String).not_null()];

        let statements = generator.generate_alter_table("users", &old_columns, &new_columns);

        assert_eq!(statements.len(), 1);
        assert!(statements[0].contains("SET NOT NULL"));
    }

    #[test]
    fn test_generate_alter_table_make_nullable() {
        let config = default_config();
        let generator = DdlGenerator::new(&config);

        // Make column nullable
        let old_columns = vec![ColumnDefinition::new("phone", ColumnType::String).not_null()];
        let new_columns = vec![ColumnDefinition::new("phone", ColumnType::String)];

        let statements = generator.generate_alter_table("users", &old_columns, &new_columns);

        assert_eq!(statements.len(), 1);
        assert!(statements[0].contains("DROP NOT NULL"));
    }

    #[test]
    fn test_generate_alter_table_add_default() {
        let config = default_config();
        let generator = DdlGenerator::new(&config);

        let old_columns = vec![ColumnDefinition::new("status", ColumnType::String)];
        let new_columns =
            vec![ColumnDefinition::new("status", ColumnType::String).default("'pending'")];

        let statements = generator.generate_alter_table("orders", &old_columns, &new_columns);

        assert_eq!(statements.len(), 1);
        assert!(statements[0].contains("SET DEFAULT"));
        assert!(statements[0].contains("'pending'"));
    }

    #[test]
    fn test_generate_alter_table_drop_default() {
        let config = default_config();
        let generator = DdlGenerator::new(&config);

        let old_columns =
            vec![ColumnDefinition::new("status", ColumnType::String).default("'active'")];
        let new_columns = vec![ColumnDefinition::new("status", ColumnType::String)];

        let statements = generator.generate_alter_table("orders", &old_columns, &new_columns);

        assert_eq!(statements.len(), 1);
        assert!(statements[0].contains("DROP DEFAULT"));
    }

    #[test]
    fn test_generate_alter_table_combined_changes() {
        let config = default_config();
        let generator = DdlGenerator::new(&config);

        let old_columns = vec![
            ColumnDefinition::new("name", ColumnType::String),
            ColumnDefinition::new("old_field", ColumnType::Integer),
            ColumnDefinition::new("price", ColumnType::Integer), // Will change type
        ];
        let new_columns = vec![
            ColumnDefinition::new("name", ColumnType::String),
            ColumnDefinition::new("price", ColumnType::decimal(10, 2)), // Type changed
            ColumnDefinition::new("new_field", ColumnType::String),     // Added
        ];

        let statements = generator.generate_alter_table("products", &old_columns, &new_columns);

        // Should have: 1 add, 1 drop, 1 type change
        assert_eq!(statements.len(), 3);

        let combined = statements.join(" | ");
        assert!(combined.contains("ADD COLUMN"));
        assert!(combined.contains("DROP COLUMN"));
        assert!(combined.contains("TYPE"));
    }

    #[test]
    fn test_generate_alter_table_no_changes() {
        let config = default_config();
        let generator = DdlGenerator::new(&config);

        let columns = vec![
            ColumnDefinition::new("name", ColumnType::String),
            ColumnDefinition::new("value", ColumnType::Integer),
        ];

        let statements = generator.generate_alter_table("items", &columns, &columns);

        assert!(statements.is_empty());
    }

    // ==================== format_column_definition Tests ====================

    #[test]
    fn test_format_column_definition_basic() {
        let col = ColumnDefinition::new("name", ColumnType::String);
        let formatted = DdlGenerator::format_column_definition(&col);

        assert_eq!(formatted, "\"name\" TEXT");
    }

    #[test]
    fn test_format_column_definition_not_null() {
        let col = ColumnDefinition::new("email", ColumnType::String).not_null();
        let formatted = DdlGenerator::format_column_definition(&col);

        assert_eq!(formatted, "\"email\" TEXT NOT NULL");
    }

    #[test]
    fn test_format_column_definition_unique() {
        let col = ColumnDefinition::new("sku", ColumnType::String).unique();
        let formatted = DdlGenerator::format_column_definition(&col);

        assert_eq!(formatted, "\"sku\" TEXT UNIQUE");
    }

    #[test]
    fn test_format_column_definition_unique_not_null() {
        let col = ColumnDefinition::new("code", ColumnType::String)
            .unique()
            .not_null();
        let formatted = DdlGenerator::format_column_definition(&col);

        assert_eq!(formatted, "\"code\" TEXT UNIQUE NOT NULL");
    }

    #[test]
    fn test_format_column_definition_with_default() {
        let col = ColumnDefinition::new("active", ColumnType::Boolean).default("TRUE");
        let formatted = DdlGenerator::format_column_definition(&col);

        assert_eq!(formatted, "\"active\" BOOLEAN DEFAULT TRUE");
    }

    #[test]
    fn test_format_column_definition_full() {
        let col = ColumnDefinition::new("status", ColumnType::String)
            .not_null()
            .default("'pending'");
        let formatted = DdlGenerator::format_column_definition(&col);

        assert_eq!(formatted, "\"status\" TEXT NOT NULL DEFAULT 'pending'");
    }

    #[test]
    fn test_format_column_definition_decimal() {
        let col = ColumnDefinition::new("amount", ColumnType::decimal(10, 2)).not_null();
        let formatted = DdlGenerator::format_column_definition(&col);

        assert_eq!(formatted, "\"amount\" NUMERIC(10,2) NOT NULL");
    }

    // ==================== Edge Cases ====================

    // ==================== Tsvector Column Tests ====================

    fn tsv_col(name: &str, source: &str) -> ColumnDefinition {
        ColumnDefinition::new(
            name,
            ColumnType::Tsvector {
                source_column: source.to_string(),
                language: "english".to_string(),
            },
        )
        .not_null()
    }

    #[test]
    fn test_format_column_definition_tsvector_emits_generated_clause() {
        let col = tsv_col("keywords_tsv", "keywords");
        let formatted = DdlGenerator::format_column_definition(&col);
        assert_eq!(
            formatted,
            "\"keywords_tsv\" TSVECTOR NOT NULL \
             GENERATED ALWAYS AS (to_tsvector('english', coalesce(\"keywords\", ''))) STORED"
        );
    }

    #[test]
    fn test_format_column_definition_tsvector_skips_default_clause() {
        // DEFAULT must not appear on a generated column — Postgres rejects both.
        let mut col = tsv_col("keywords_tsv", "keywords");
        col.default_value = Some("'ignored'".to_string());
        let formatted = DdlGenerator::format_column_definition(&col);
        assert!(!formatted.contains("DEFAULT"), "{}", formatted);
        assert!(formatted.contains("GENERATED ALWAYS AS"));
    }

    #[test]
    fn test_generate_tsvector_indexes_emits_partial_gin() {
        let config = default_config();
        let generator = DdlGenerator::new(&config);
        let columns = vec![
            ColumnDefinition::new("keywords", ColumnType::String),
            tsv_col("keywords_tsv", "keywords"),
        ];
        let statements = generator.generate_tsvector_indexes("products", &columns);
        assert_eq!(statements.len(), 1);
        assert_eq!(
            statements[0],
            "CREATE INDEX IF NOT EXISTS \"idx_products_keywords_tsv_fts\" \
             ON \"products\" USING GIN (\"keywords_tsv\") \
             WHERE deleted = FALSE"
        );
    }

    #[test]
    fn test_generate_alter_table_emits_tsvector_index_for_added_column() {
        let config = default_config();
        let generator = DdlGenerator::new(&config);
        let old_columns = vec![ColumnDefinition::new("keywords", ColumnType::String)];
        let new_columns = vec![
            ColumnDefinition::new("keywords", ColumnType::String),
            tsv_col("keywords_tsv", "keywords"),
        ];
        let statements = generator.generate_alter_table("products", &old_columns, &new_columns);
        assert_eq!(statements.len(), 2);
        assert!(statements[0].contains("ADD COLUMN"));
        assert!(statements[0].contains("GENERATED ALWAYS AS"));
        assert!(statements[1].contains("CREATE INDEX IF NOT EXISTS"));
        assert!(statements[1].contains("USING GIN"));
        assert!(statements[1].contains("idx_products_keywords_tsv_fts"));
    }

    #[test]
    fn test_generate_alter_table_drops_tsvector_index_with_column() {
        let config = default_config();
        let generator = DdlGenerator::new(&config);
        let old_columns = vec![
            ColumnDefinition::new("keywords", ColumnType::String),
            tsv_col("keywords_tsv", "keywords"),
        ];
        let new_columns = vec![ColumnDefinition::new("keywords", ColumnType::String)];
        let statements = generator.generate_alter_table("products", &old_columns, &new_columns);
        assert_eq!(statements.len(), 2);
        assert!(statements[0].contains("DROP INDEX IF EXISTS"));
        assert!(statements[0].contains("idx_products_keywords_tsv_fts"));
        assert!(statements[1].contains("DROP COLUMN"));
    }

    // ==================== Trigram Index Tests ====================

    #[test]
    fn test_generate_trigram_indexes_empty_when_no_flag() {
        let config = default_config();
        let generator = DdlGenerator::new(&config);
        let columns = vec![
            ColumnDefinition::new("name", ColumnType::String),
            ColumnDefinition::new("price", ColumnType::Integer),
        ];
        let statements = generator.generate_trigram_indexes("products", &columns);
        assert!(statements.is_empty());
    }

    #[test]
    fn test_generate_trigram_indexes_emits_partial_gin() {
        let config = default_config();
        let generator = DdlGenerator::new(&config);
        let columns = vec![
            ColumnDefinition::new("keywords", ColumnType::String).with_trigram_index(),
            ColumnDefinition::new("name", ColumnType::String),
        ];
        let statements = generator.generate_trigram_indexes("products", &columns);
        assert_eq!(statements.len(), 1);
        assert_eq!(
            statements[0],
            "CREATE INDEX IF NOT EXISTS \"idx_products_keywords_trgm\" \
             ON \"products\" USING GIN (\"keywords\" gin_trgm_ops) \
             WHERE deleted = FALSE"
        );
    }

    #[test]
    fn test_generate_alter_table_emits_trigram_index_for_added_column() {
        let config = default_config();
        let generator = DdlGenerator::new(&config);
        let old_columns = vec![ColumnDefinition::new("name", ColumnType::String)];
        let new_columns = vec![
            ColumnDefinition::new("name", ColumnType::String),
            ColumnDefinition::new("keywords", ColumnType::String).with_trigram_index(),
        ];
        let statements = generator.generate_alter_table("products", &old_columns, &new_columns);
        assert_eq!(statements.len(), 2);
        assert!(statements[0].contains("ADD COLUMN"));
        assert!(statements[1].contains("CREATE INDEX IF NOT EXISTS"));
        assert!(statements[1].contains("gin_trgm_ops"));
    }

    #[test]
    fn test_generate_alter_table_drops_trigram_index_with_column() {
        let config = default_config();
        let generator = DdlGenerator::new(&config);
        let old_columns = vec![
            ColumnDefinition::new("name", ColumnType::String),
            ColumnDefinition::new("keywords", ColumnType::String).with_trigram_index(),
        ];
        let new_columns = vec![ColumnDefinition::new("name", ColumnType::String)];
        let statements = generator.generate_alter_table("products", &old_columns, &new_columns);
        assert_eq!(statements.len(), 2);
        assert!(statements[0].contains("DROP INDEX IF EXISTS"));
        assert!(statements[1].contains("DROP COLUMN"));
    }

    #[test]
    fn test_ddl_generator_with_quoted_table_name() {
        let config = default_config();
        let generator = DdlGenerator::new(&config);

        // Table name that needs quoting
        let ddl = generator.generate_drop_table("my-table");

        assert!(ddl.contains("\"my-table\""));
    }

    #[test]
    fn test_ddl_generator_column_name_needs_quoting() {
        let config = config_no_auto_columns();
        let generator = DdlGenerator::new(&config);

        let columns = vec![
            ColumnDefinition::new("user-id", ColumnType::String),
            ColumnDefinition::new("order", ColumnType::Integer), // Reserved word
        ];

        let ddl = generator.generate_create_table("data", &columns);

        assert!(ddl.contains("\"user-id\""));
        assert!(ddl.contains("\"order\""));
    }
}
