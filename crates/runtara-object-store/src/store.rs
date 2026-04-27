//! ObjectStore - Main entry point for schema-driven PostgreSQL object storage
//!
//! This module provides the main `ObjectStore` struct that manages dynamic schemas
//! and their instances in a PostgreSQL database.

use sqlx::{PgPool, Row};

use crate::config::{DEFAULT_AGGREGATE_RESULT_ROW_LIMIT, StoreConfig};
use crate::error::{ObjectStoreError, Result};
use crate::instance::{
    Condition, FilterRequest, Instance, OrderByEntry, OrderByTarget, SimpleFilter,
};
use crate::schema::{CreateSchemaRequest, Schema, UpdateSchemaRequest};
use crate::sql::aggregate::{AggregateRequest, AggregateResult, build_aggregate_query};
use crate::sql::condition::{build_condition_clause, build_order_by_clause, field_to_sql};
use crate::sql::ddl::DdlGenerator;
use crate::sql::expr::{ExprNode, render_row_expression, validate_row_expression};
use crate::sql::sanitize::quote_identifier;
use crate::types::{ColumnDefinition, ColumnType};

/// A validated row destined for bulk insert: (generated id, payload map).
type ValidatedRow = (String, serde_json::Map<String, serde_json::Value>);

/// Schema-driven dynamic PostgreSQL object store
///
/// Manages schemas and instances in a single PostgreSQL database.
/// Schema metadata is stored in a configurable metadata table (default: `__schema`).
/// Instance data is stored in dynamically created tables.
pub struct ObjectStore {
    /// Database connection pool
    pool: PgPool,
    /// Store configuration
    config: StoreConfig,
}

impl ObjectStore {
    /// Create a new ObjectStore from configuration
    ///
    /// This will:
    /// 1. Connect to the database
    /// 2. Create the metadata table if it doesn't exist
    /// 3. Try to enable required extensions (e.g. `pg_trgm`)
    pub async fn new(config: StoreConfig) -> Result<Self> {
        let pool = PgPool::connect(&config.database_url).await.map_err(|e| {
            ObjectStoreError::Connection(format!("Database connection failed: {}", e))
        })?;

        let store = Self { pool, config };
        store.ensure_metadata_table().await?;
        store.ensure_extensions().await;

        Ok(store)
    }

    /// Create a new ObjectStore from an existing pool
    ///
    /// Use this when you already have a connection pool and want to
    /// share it with the object store.
    pub async fn from_pool(pool: PgPool, config: StoreConfig) -> Result<Self> {
        let store = Self { pool, config };
        store.ensure_metadata_table().await?;
        store.ensure_extensions().await;
        Ok(store)
    }

    /// Best-effort enable of required Postgres extensions.
    ///
    /// `pg_trgm` is required for `SIMILARITY_GTE` and trigram indexes,
    /// `vector` (pgvector) is required for `Vector` columns and the four
    /// distance ExprFns, and `fuzzystrmatch` is required for `LEVENSHTEIN`.
    /// The migration runner already issues these for the metadata DB, but
    /// per-tenant DBs that bootstrap through `from_pool` may not pass through
    /// the migration path. We soft-fail with a warning so a hardened
    /// permissions setup doesn't crash startup; dependent operations will
    /// surface a clear error later. pgvector availability varies across
    /// managed Postgres providers, so the soft-fail matters more for it.
    async fn ensure_extensions(&self) {
        for (ext, hint) in [
            ("pg_trgm", "SIMILARITY_GTE / trigram indexes"),
            (
                "vector",
                "Vector columns / COSINE_DISTANCE / L2_DISTANCE / INNER_PRODUCT",
            ),
            ("fuzzystrmatch", "LEVENSHTEIN"),
        ] {
            let stmt = format!(r#"CREATE EXTENSION IF NOT EXISTS "{}""#, ext);
            if let Err(e) = sqlx::query(&stmt).execute(&self.pool).await {
                eprintln!(
                    "warning: failed to ensure {} extension ({}); {} will not work until it is installed",
                    ext, e, hint
                );
            }
        }
    }

    /// Get a reference to the connection pool
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// Get a reference to the configuration
    pub fn config(&self) -> &StoreConfig {
        &self.config
    }

    /// Ensures the metadata table exists
    async fn ensure_metadata_table(&self) -> Result<()> {
        let metadata_table = quote_identifier(&self.config.metadata_table);

        let create_sql = format!(
            r#"
            CREATE TABLE IF NOT EXISTS {} (
                id VARCHAR(255) PRIMARY KEY DEFAULT gen_random_uuid()::text,
                name VARCHAR(255) UNIQUE NOT NULL,
                description TEXT,
                table_name VARCHAR(255) UNIQUE NOT NULL,
                columns JSONB NOT NULL,
                indexes JSONB,
                created_at TIMESTAMPTZ DEFAULT NOW(),
                updated_at TIMESTAMPTZ DEFAULT NOW(),
                deleted BOOLEAN DEFAULT FALSE
            )
            "#,
            metadata_table
        );

        sqlx::query(&create_sql).execute(&self.pool).await?;

        Ok(())
    }

    // =========================================================================
    // Schema Operations
    // =========================================================================

    /// Create a new schema
    ///
    /// This will:
    /// 1. Insert the schema metadata into the metadata table
    /// 2. Create the data table with the specified columns
    /// 3. Create any specified indexes
    pub async fn create_schema(&self, request: CreateSchemaRequest) -> Result<Schema> {
        // Check if schema name already exists
        if self.get_schema(&request.name).await?.is_some() {
            return Err(ObjectStoreError::conflict(format!(
                "Schema '{}' already exists",
                request.name
            )));
        }

        // Check if table name already exists
        if self.schema_by_table(&request.table_name).await?.is_some() {
            return Err(ObjectStoreError::conflict(format!(
                "Table '{}' already exists",
                request.table_name
            )));
        }

        let schema_id = uuid::Uuid::new_v4().to_string();
        let metadata_table = quote_identifier(&self.config.metadata_table);

        // Insert metadata
        let columns_json = serde_json::to_value(&request.columns)?;
        let indexes_json = request
            .indexes
            .as_ref()
            .map(serde_json::to_value)
            .transpose()?;

        let insert_sql = format!(
            r#"
            INSERT INTO {} (id, name, description, table_name, columns, indexes)
            VALUES ($1, $2, $3, $4, $5, $6)
            RETURNING created_at, updated_at
            "#,
            metadata_table
        );

        let row = sqlx::query(&insert_sql)
            .bind(&schema_id)
            .bind(&request.name)
            .bind(&request.description)
            .bind(&request.table_name)
            .bind(&columns_json)
            .bind(&indexes_json)
            .fetch_one(&self.pool)
            .await?;

        let created_at: chrono::DateTime<chrono::Utc> = row.try_get("created_at")?;
        let updated_at: chrono::DateTime<chrono::Utc> = row.try_get("updated_at")?;

        // Create the data table
        let ddl = DdlGenerator::new(&self.config);
        let create_table_sql = ddl.generate_create_table(&request.table_name, &request.columns);
        sqlx::query(&create_table_sql).execute(&self.pool).await?;

        // Create default index
        let default_index_sql = ddl.generate_default_index(&request.table_name);
        sqlx::query(&default_index_sql).execute(&self.pool).await?;

        // Create trigram (`gin_trgm_ops`) indexes for any column that wants
        // them. Empty if no column has `text_index = trigram`.
        for trigram_sql in ddl.generate_trigram_indexes(&request.table_name, &request.columns) {
            sqlx::query(&trigram_sql).execute(&self.pool).await?;
        }

        // Create GIN indexes for any tsvector-typed columns. Tsvector
        // columns are useless without a GIN index — full-text queries fall
        // back to seq scans otherwise.
        for tsv_sql in ddl.generate_tsvector_indexes(&request.table_name, &request.columns) {
            sqlx::query(&tsv_sql).execute(&self.pool).await?;
        }

        // Create HNSW / IVFFlat indexes for any vector-typed columns whose
        // declaration opts in to an index method. Without an index, KNN
        // queries fall back to a seq scan with exact distance computation.
        for vec_sql in ddl.generate_vector_indexes(&request.table_name, &request.columns) {
            sqlx::query(&vec_sql).execute(&self.pool).await?;
        }

        // Create any specified indexes
        if let Some(indexes) = &request.indexes {
            for index in indexes {
                let index_sql = ddl.generate_create_index(&request.table_name, index);
                sqlx::query(&index_sql).execute(&self.pool).await?;
            }
        }

        Ok(Schema {
            id: schema_id,
            created_at: created_at.to_rfc3339(),
            updated_at: updated_at.to_rfc3339(),
            name: request.name,
            description: request.description,
            table_name: request.table_name,
            columns: request.columns,
            indexes: request.indexes,
        })
    }

    /// Get schema by name
    pub async fn get_schema(&self, name: &str) -> Result<Option<Schema>> {
        let metadata_table = quote_identifier(&self.config.metadata_table);

        let select_sql = format!(
            r#"
            SELECT id, created_at, updated_at, name, description, table_name, columns, indexes
            FROM {}
            WHERE name = $1 AND deleted = FALSE
            "#,
            metadata_table
        );

        let result = sqlx::query(&select_sql)
            .bind(name)
            .fetch_optional(&self.pool)
            .await?;

        match result {
            Some(row) => Ok(Some(self.row_to_schema(&row)?)),
            None => Ok(None),
        }
    }

    /// Get schema by ID
    pub async fn get_schema_by_id(&self, id: &str) -> Result<Option<Schema>> {
        let metadata_table = quote_identifier(&self.config.metadata_table);

        let select_sql = format!(
            r#"
            SELECT id, created_at, updated_at, name, description, table_name, columns, indexes
            FROM {}
            WHERE id = $1 AND deleted = FALSE
            "#,
            metadata_table
        );

        let result = sqlx::query(&select_sql)
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;

        match result {
            Some(row) => Ok(Some(self.row_to_schema(&row)?)),
            None => Ok(None),
        }
    }

    /// Get schema by table name
    async fn schema_by_table(&self, table_name: &str) -> Result<Option<Schema>> {
        let metadata_table = quote_identifier(&self.config.metadata_table);

        let select_sql = format!(
            r#"
            SELECT id, created_at, updated_at, name, description, table_name, columns, indexes
            FROM {}
            WHERE table_name = $1 AND deleted = FALSE
            "#,
            metadata_table
        );

        let result = sqlx::query(&select_sql)
            .bind(table_name)
            .fetch_optional(&self.pool)
            .await?;

        match result {
            Some(row) => Ok(Some(self.row_to_schema(&row)?)),
            None => Ok(None),
        }
    }

    /// List all schemas
    pub async fn list_schemas(&self) -> Result<Vec<Schema>> {
        let metadata_table = quote_identifier(&self.config.metadata_table);

        let select_sql = format!(
            r#"
            SELECT id, created_at, updated_at, name, description, table_name, columns, indexes
            FROM {}
            WHERE deleted = FALSE
            ORDER BY created_at DESC
            "#,
            metadata_table
        );

        let rows = sqlx::query(&select_sql).fetch_all(&self.pool).await?;

        rows.iter().map(|row| self.row_to_schema(row)).collect()
    }

    /// Update a schema
    ///
    /// This will update schema metadata and alter the table if columns changed.
    pub async fn update_schema(&self, name: &str, request: UpdateSchemaRequest) -> Result<Schema> {
        let existing = self
            .get_schema(name)
            .await?
            .ok_or_else(|| ObjectStoreError::schema_not_found(name))?;

        let metadata_table = quote_identifier(&self.config.metadata_table);

        // Build SET clauses
        let mut set_clauses = vec!["updated_at = NOW()".to_string()];
        let mut param_idx = 2; // $1 is the schema name

        if request.name.is_some() {
            set_clauses.push(format!("name = ${}", param_idx));
            param_idx += 1;
        }
        if request.description.is_some() {
            set_clauses.push(format!("description = ${}", param_idx));
            param_idx += 1;
        }
        if request.columns.is_some() {
            set_clauses.push(format!("columns = ${}", param_idx));
            param_idx += 1;
        }
        if request.indexes.is_some() {
            set_clauses.push(format!("indexes = ${}", param_idx));
        }

        let update_sql = format!(
            r#"
            UPDATE {}
            SET {}
            WHERE name = $1 AND deleted = FALSE
            RETURNING id, created_at, updated_at, name, description, table_name, columns, indexes
            "#,
            metadata_table,
            set_clauses.join(", "),
        );

        let mut query = sqlx::query(&update_sql).bind(name);

        if let Some(ref new_name) = request.name {
            query = query.bind(new_name);
        }
        if let Some(ref description) = request.description {
            query = query.bind(description);
        }
        if let Some(ref columns) = request.columns {
            let columns_json = serde_json::to_value(columns)?;
            query = query.bind(columns_json);
        }
        if let Some(ref indexes) = request.indexes {
            let indexes_json = serde_json::to_value(indexes)?;
            query = query.bind(indexes_json);
        }

        let row = query.fetch_one(&self.pool).await?;
        let schema = self.row_to_schema(&row)?;

        // Alter table if columns changed
        if let Some(new_columns) = &request.columns {
            let ddl = DdlGenerator::new(&self.config);
            let alter_statements =
                ddl.generate_alter_table(&existing.table_name, &existing.columns, new_columns);

            for statement in alter_statements {
                sqlx::query(&statement).execute(&self.pool).await?;
            }
        }

        Ok(schema)
    }

    /// Delete a schema
    ///
    /// If soft_delete is enabled, marks the schema as deleted.
    /// Otherwise, drops the table and removes the metadata.
    pub async fn delete_schema(&self, name: &str) -> Result<()> {
        let schema = self
            .get_schema(name)
            .await?
            .ok_or_else(|| ObjectStoreError::schema_not_found(name))?;

        let metadata_table = quote_identifier(&self.config.metadata_table);

        if self.config.soft_delete {
            let update_sql = format!(
                "UPDATE {} SET deleted = TRUE, updated_at = NOW() WHERE name = $1 AND deleted = FALSE",
                metadata_table
            );
            sqlx::query(&update_sql)
                .bind(name)
                .execute(&self.pool)
                .await?;
        } else {
            // Hard delete: drop table and remove metadata
            let ddl = DdlGenerator::new(&self.config);
            let drop_sql = ddl.generate_drop_table(&schema.table_name);
            sqlx::query(&drop_sql).execute(&self.pool).await?;

            let delete_sql = format!("DELETE FROM {} WHERE name = $1", metadata_table);
            sqlx::query(&delete_sql)
                .bind(name)
                .execute(&self.pool)
                .await?;
        }

        Ok(())
    }

    // =========================================================================
    // Instance Operations
    // =========================================================================

    /// Create a new instance
    pub async fn create_instance(
        &self,
        schema_name: &str,
        properties: serde_json::Value,
    ) -> Result<String> {
        let schema = self
            .get_schema(schema_name)
            .await?
            .ok_or_else(|| ObjectStoreError::schema_not_found(schema_name))?;

        let properties_obj = properties
            .as_object()
            .ok_or_else(|| ObjectStoreError::validation("Properties must be a JSON object"))?;

        let instance_id = uuid::Uuid::new_v4().to_string();

        // Build column names and placeholders
        let mut column_names = Vec::new();
        let mut placeholders = Vec::new();
        let mut param_idx = 1;

        // Add auto-managed id if enabled
        if self.config.auto_columns.id {
            column_names.push("id".to_string());
            placeholders.push(format!("${}", param_idx));
            param_idx += 1;
        }

        // Validate and collect columns
        for col in &schema.columns {
            if col.column_type.is_generated() {
                if let Some(v) = properties_obj.get(&col.name)
                    && !v.is_null()
                {
                    return Err(ObjectStoreError::validation(format!(
                        "Column '{}' is generated and cannot be set",
                        col.name
                    )));
                }
                continue;
            }
            if let Some(value) = properties_obj.get(&col.name) {
                // Validate type
                if let Err(e) = col.column_type.validate_value(value) {
                    return Err(ObjectStoreError::validation(format!(
                        "Invalid value for column '{}': {}",
                        col.name, e
                    )));
                }

                if !col.nullable && value.is_null() {
                    return Err(ObjectStoreError::validation(format!(
                        "Column '{}' does not allow NULL values",
                        col.name
                    )));
                }

                column_names.push(quote_identifier(&col.name));
                placeholders.push(format!("${}", param_idx));
                param_idx += 1;
            } else if !col.nullable && col.default_value.is_none() {
                return Err(ObjectStoreError::validation(format!(
                    "Required column '{}' is missing",
                    col.name
                )));
            }
        }

        let insert_sql = format!(
            "INSERT INTO {} ({}) VALUES ({})",
            quote_identifier(&schema.table_name),
            column_names.join(", "),
            placeholders.join(", ")
        );

        // Build query with type-aware bindings
        let mut query = sqlx::query(&insert_sql);

        if self.config.auto_columns.id {
            query = query.bind(&instance_id);
        }

        for col in &schema.columns {
            if col.column_type.is_generated() {
                continue;
            }
            if let Some(value) = properties_obj.get(&col.name) {
                query = Self::bind_value(query, &col.column_type, &col.name, value)?;
            }
        }

        query.execute(&self.pool).await?;

        Ok(instance_id)
    }

    /// Get instance by ID
    pub async fn get_instance(
        &self,
        schema_name: &str,
        instance_id: &str,
    ) -> Result<Option<Instance>> {
        let schema = self
            .get_schema(schema_name)
            .await?
            .ok_or_else(|| ObjectStoreError::schema_not_found(schema_name))?;

        // Build column list
        let mut select_columns = Vec::new();

        if self.config.auto_columns.id {
            select_columns.push("id".to_string());
        }
        if self.config.auto_columns.created_at {
            select_columns.push("created_at".to_string());
        }
        if self.config.auto_columns.updated_at {
            select_columns.push("updated_at".to_string());
        }

        for col in &schema.columns {
            if col.column_type.is_generated() {
                continue;
            }
            select_columns.push(quote_identifier(&col.name));
        }

        let select_sql = format!(
            "SELECT {} FROM {} WHERE id = $1 AND deleted = FALSE",
            select_columns.join(", "),
            quote_identifier(&schema.table_name),
        );

        let row = sqlx::query(&select_sql)
            .bind(instance_id)
            .fetch_optional(&self.pool)
            .await?;

        Ok(row.map(|row| self.row_to_instance(&row, &schema, None)))
    }

    /// Query instances using simple filters
    pub async fn query_instances(&self, filter: SimpleFilter) -> Result<(Vec<Instance>, i64)> {
        let schema = self
            .get_schema(&filter.schema_name)
            .await?
            .ok_or_else(|| ObjectStoreError::schema_not_found(&filter.schema_name))?;

        let filter_request = filter.to_filter_request();
        self.filter_instances_internal(&schema, filter_request)
            .await
    }

    /// Filter instances with condition
    pub async fn filter_instances(
        &self,
        schema_name: &str,
        filter: FilterRequest,
    ) -> Result<(Vec<Instance>, i64)> {
        let schema = self
            .get_schema(schema_name)
            .await?
            .ok_or_else(|| ObjectStoreError::schema_not_found(schema_name))?;

        self.filter_instances_internal(&schema, filter).await
    }

    /// Run an aggregate (GROUP BY) query and return a columnar result.
    ///
    /// Enforces [`DEFAULT_AGGREGATE_RESULT_ROW_LIMIT`]:
    /// - If the caller sets `limit`, it is silently clamped to the cap.
    /// - If the caller omits `limit` and the natural result exceeds the cap,
    ///   the request is rejected so the caller must add an explicit `limit`.
    pub async fn aggregate_instances(
        &self,
        schema_name: &str,
        request: AggregateRequest,
    ) -> Result<AggregateResult> {
        let schema = self
            .get_schema(schema_name)
            .await?
            .ok_or_else(|| ObjectStoreError::schema_not_found(schema_name))?;

        let sql =
            build_aggregate_query(&schema, &request).map_err(ObjectStoreError::InvalidCondition)?;

        // Effective LIMIT / OFFSET.
        let cap = DEFAULT_AGGREGATE_RESULT_ROW_LIMIT as i64;
        let (effective_limit, caller_set_limit) = match request.limit {
            Some(l) if l < 0 => (0i64, true),
            Some(l) => (l.min(cap), true),
            None => (cap + 1, false),
        };
        let effective_offset = request.offset.unwrap_or(0).max(0);

        // Bind condition params as strings (matches filter_instances_internal).
        let mut data_q = sqlx::query(&sql.data_sql);
        for param in &sql.params {
            let s = match param {
                serde_json::Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            data_q = data_q.bind(s);
        }
        data_q = data_q.bind(effective_limit).bind(effective_offset);
        let rows = data_q.fetch_all(&self.pool).await?;

        // If the caller did not supply a limit and we got more than the cap,
        // reject: the full result is too large to materialize safely.
        if !caller_set_limit && rows.len() as i64 > cap {
            return Err(ObjectStoreError::validation(format!(
                "aggregate result exceeds {} rows; add an explicit `limit`",
                cap
            )));
        }

        // Decode each row — every output column is wrapped in to_jsonb(),
        // so sqlx decodes each cell as a serde_json::Value.
        let mut out_rows: Vec<Vec<serde_json::Value>> = Vec::with_capacity(rows.len());
        for row in &rows {
            let mut cells = Vec::with_capacity(sql.columns.len());
            for col_name in &sql.columns {
                let cell: serde_json::Value = row
                    .try_get::<Option<serde_json::Value>, _>(col_name.as_str())
                    .map_err(|e| {
                        ObjectStoreError::database(format!(
                            "decoding aggregate column '{}': {}",
                            col_name, e
                        ))
                    })?
                    .unwrap_or(serde_json::Value::Null);
                cells.push(cell);
            }
            out_rows.push(cells);
        }

        // group_count: separate COUNT query when there's a GROUP BY, otherwise 1.
        let group_count: i64 = if let Some(count_sql) = &sql.count_sql {
            let mut count_q = sqlx::query_as::<_, (i64,)>(count_sql);
            for param in &sql.params {
                let s = match param {
                    serde_json::Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                count_q = count_q.bind(s);
            }
            count_q.fetch_one(&self.pool).await?.0
        } else {
            1
        };

        Ok(AggregateResult {
            columns: sql.columns,
            rows: out_rows,
            group_count,
        })
    }

    /// Check if an instance exists matching the filters
    pub async fn instance_exists(&self, filter: SimpleFilter) -> Result<Option<Instance>> {
        let mut filter = filter;
        filter.limit = 1;
        let (instances, _) = self.query_instances(filter).await?;
        Ok(instances.into_iter().next())
    }

    /// Update an instance
    pub async fn update_instance(
        &self,
        schema_name: &str,
        instance_id: &str,
        properties: serde_json::Value,
    ) -> Result<()> {
        let schema = self
            .get_schema(schema_name)
            .await?
            .ok_or_else(|| ObjectStoreError::schema_not_found(schema_name))?;

        let properties_obj = properties
            .as_object()
            .ok_or_else(|| ObjectStoreError::validation("Properties must be a JSON object"))?;

        let mut set_clauses = Vec::new();
        let mut param_idx = 2; // $1 = instance_id

        if self.config.auto_columns.updated_at {
            set_clauses.push("updated_at = NOW()".to_string());
        }

        for col in &schema.columns {
            if col.column_type.is_generated() {
                if let Some(v) = properties_obj.get(&col.name)
                    && !v.is_null()
                {
                    return Err(ObjectStoreError::validation(format!(
                        "Column '{}' is generated and cannot be set",
                        col.name
                    )));
                }
                continue;
            }
            if let Some(value) = properties_obj.get(&col.name) {
                // Validate type
                if let Err(e) = col.column_type.validate_value(value) {
                    return Err(ObjectStoreError::validation(format!(
                        "Invalid value for column '{}': {}",
                        col.name, e
                    )));
                }

                set_clauses.push(format!("{} = ${}", quote_identifier(&col.name), param_idx));
                param_idx += 1;
            }
        }

        if set_clauses.is_empty() || (set_clauses.len() == 1 && self.config.auto_columns.updated_at)
        {
            return Ok(()); // Nothing to update
        }

        let update_sql = format!(
            "UPDATE {} SET {} WHERE id = $1 AND deleted = FALSE",
            quote_identifier(&schema.table_name),
            set_clauses.join(", "),
        );

        let mut query = sqlx::query(&update_sql).bind(instance_id);

        for col in &schema.columns {
            if col.column_type.is_generated() {
                continue;
            }
            if let Some(value) = properties_obj.get(&col.name) {
                query = Self::bind_value(query, &col.column_type, &col.name, value)?;
            }
        }

        let result = query.execute(&self.pool).await?;

        if result.rows_affected() == 0 {
            return Err(ObjectStoreError::instance_not_found(instance_id));
        }

        Ok(())
    }

    /// Delete an instance
    ///
    /// If soft_delete is enabled, marks the instance as deleted.
    /// Otherwise, removes the row from the table.
    pub async fn delete_instance(&self, schema_name: &str, instance_id: &str) -> Result<()> {
        let schema = self
            .get_schema(schema_name)
            .await?
            .ok_or_else(|| ObjectStoreError::schema_not_found(schema_name))?;

        let result = if self.config.soft_delete {
            let update_set = if self.config.auto_columns.updated_at {
                "deleted = TRUE, updated_at = NOW()"
            } else {
                "deleted = TRUE"
            };

            let delete_sql = format!(
                "UPDATE {} SET {} WHERE id = $1 AND deleted = FALSE",
                quote_identifier(&schema.table_name),
                update_set
            );

            sqlx::query(&delete_sql)
                .bind(instance_id)
                .execute(&self.pool)
                .await?
        } else {
            let delete_sql = format!(
                "DELETE FROM {} WHERE id = $1",
                quote_identifier(&schema.table_name)
            );

            sqlx::query(&delete_sql)
                .bind(instance_id)
                .execute(&self.pool)
                .await?
        };

        if result.rows_affected() == 0 {
            return Err(ObjectStoreError::instance_not_found(instance_id));
        }

        Ok(())
    }

    // =========================================================================
    // Bulk Operations
    // =========================================================================

    /// Update multiple instances matching a condition
    ///
    /// All updates happen in a single transaction - if any row fails,
    /// the entire operation is rolled back.
    ///
    /// # Arguments
    /// * `schema_name` - Name of the schema
    /// * `properties` - JSON object containing fields to update
    /// * `condition` - Condition to match rows for update
    ///
    /// # Returns
    /// Number of affected rows
    pub async fn update_instances(
        &self,
        schema_name: &str,
        properties: serde_json::Value,
        condition: Condition,
    ) -> Result<i64> {
        let schema = self
            .get_schema(schema_name)
            .await?
            .ok_or_else(|| ObjectStoreError::schema_not_found(schema_name))?;

        let properties_obj = properties
            .as_object()
            .ok_or_else(|| ObjectStoreError::validation("Properties must be a JSON object"))?;

        // Build SET clause
        let mut set_clauses = Vec::new();
        let mut set_values: Vec<(&ColumnDefinition, &serde_json::Value)> = Vec::new();
        let mut param_idx = 1i32;

        if self.config.auto_columns.updated_at {
            set_clauses.push("updated_at = NOW()".to_string());
        }

        for col in &schema.columns {
            if col.column_type.is_generated() {
                continue;
            }
            if let Some(value) = properties_obj.get(&col.name) {
                // Validate type
                if let Err(e) = col.column_type.validate_value(value) {
                    return Err(ObjectStoreError::validation(format!(
                        "Invalid value for column '{}': {}",
                        col.name, e
                    )));
                }

                set_clauses.push(format!("{} = ${}", quote_identifier(&col.name), param_idx));
                set_values.push((col, value));
                param_idx += 1;
            }
        }

        if set_clauses.is_empty() || (set_clauses.len() == 1 && self.config.auto_columns.updated_at)
        {
            return Ok(0); // Nothing to update
        }

        // Build WHERE clause from condition
        let (where_clause, condition_params) =
            build_condition_clause(&condition, &mut param_idx, &schema)
                .map_err(ObjectStoreError::InvalidCondition)?;

        let base_where = format!("deleted = FALSE AND ({})", where_clause);

        let update_sql = format!(
            "UPDATE {} SET {} WHERE {}",
            quote_identifier(&schema.table_name),
            set_clauses.join(", "),
            base_where
        );

        // Start transaction
        let mut tx = self.pool.begin().await?;

        // Build and execute query
        let mut query = sqlx::query(&update_sql);

        // Bind SET values
        for (col, value) in &set_values {
            query = Self::bind_value(query, &col.column_type, &col.name, value)?;
        }

        // Bind condition params
        for param in &condition_params {
            let param_str = match param {
                serde_json::Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            query = query.bind(param_str);
        }

        let result = query.execute(&mut *tx).await?;
        tx.commit().await?;

        Ok(result.rows_affected() as i64)
    }

    /// Delete multiple instances matching a condition
    ///
    /// If soft_delete is enabled, marks instances as deleted.
    /// Otherwise, removes rows from the table.
    ///
    /// All deletes happen in a single transaction - if any row fails,
    /// the entire operation is rolled back.
    ///
    /// # Arguments
    /// * `schema_name` - Name of the schema
    /// * `condition` - Condition to match rows for deletion
    ///
    /// # Returns
    /// Number of affected rows
    pub async fn delete_instances(&self, schema_name: &str, condition: Condition) -> Result<i64> {
        let schema = self
            .get_schema(schema_name)
            .await?
            .ok_or_else(|| ObjectStoreError::schema_not_found(schema_name))?;

        // Build WHERE clause from condition
        let mut param_offset = 1i32;
        let (where_clause, condition_params) =
            build_condition_clause(&condition, &mut param_offset, &schema)
                .map_err(ObjectStoreError::InvalidCondition)?;

        let mut tx = self.pool.begin().await?;

        let result = if self.config.soft_delete {
            let update_set = if self.config.auto_columns.updated_at {
                "deleted = TRUE, updated_at = NOW()"
            } else {
                "deleted = TRUE"
            };

            let base_where = format!("deleted = FALSE AND ({})", where_clause);

            let delete_sql = format!(
                "UPDATE {} SET {} WHERE {}",
                quote_identifier(&schema.table_name),
                update_set,
                base_where
            );

            let mut query = sqlx::query(&delete_sql);
            for param in &condition_params {
                let param_str = match param {
                    serde_json::Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                query = query.bind(param_str);
            }
            query.execute(&mut *tx).await?
        } else {
            let delete_sql = format!(
                "DELETE FROM {} WHERE ({})",
                quote_identifier(&schema.table_name),
                where_clause
            );

            let mut query = sqlx::query(&delete_sql);
            for param in &condition_params {
                let param_str = match param {
                    serde_json::Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                query = query.bind(param_str);
            }
            query.execute(&mut *tx).await?
        };

        tx.commit().await?;

        Ok(result.rows_affected() as i64)
    }

    /// Create multiple instances in a single transaction
    ///
    /// All instances are validated before any are inserted.
    /// If validation fails for any instance, no instances are created.
    ///
    /// # Arguments
    /// * `schema_name` - Name of the schema
    /// * `instances` - Vector of JSON objects to insert
    ///
    /// # Returns
    /// Number of created rows
    pub async fn create_instances(
        &self,
        schema_name: &str,
        instances: Vec<serde_json::Value>,
    ) -> Result<i64> {
        if instances.is_empty() {
            return Ok(0);
        }

        if instances.len() > self.config.bulk_request_limit {
            return Err(ObjectStoreError::validation(format!(
                "bulk request size {} exceeds limit of {}",
                instances.len(),
                self.config.bulk_request_limit
            )));
        }

        let schema = self
            .get_schema(schema_name)
            .await?
            .ok_or_else(|| ObjectStoreError::schema_not_found(schema_name))?;

        // Pre-validate all instances and generate IDs
        let mut validated_instances: Vec<(String, serde_json::Map<String, serde_json::Value>)> =
            Vec::with_capacity(instances.len());

        for (idx, instance) in instances.iter().enumerate() {
            let properties_obj = instance.as_object().ok_or_else(|| {
                ObjectStoreError::validation(format!(
                    "Instance at index {} must be a JSON object",
                    idx
                ))
            })?;

            // Validate each column
            for col in &schema.columns {
                if col.column_type.is_generated() {
                    continue;
                }
                if let Some(value) = properties_obj.get(&col.name) {
                    if let Err(e) = col.column_type.validate_value(value) {
                        return Err(ObjectStoreError::validation(format!(
                            "Instance at index {}: Invalid value for column '{}': {}",
                            idx, col.name, e
                        )));
                    }

                    if !col.nullable && value.is_null() {
                        return Err(ObjectStoreError::validation(format!(
                            "Instance at index {}: Column '{}' does not allow NULL values",
                            idx, col.name
                        )));
                    }
                } else if !col.nullable && col.default_value.is_none() {
                    return Err(ObjectStoreError::validation(format!(
                        "Instance at index {}: Required column '{}' is missing",
                        idx, col.name
                    )));
                }
            }

            let instance_id = uuid::Uuid::new_v4().to_string();
            validated_instances.push((instance_id, properties_obj.clone()));
        }

        // Calculate chunk size (PostgreSQL limit ~32k params)
        let params_per_row = 1 + schema.columns.len(); // id + columns
        let chunk_size = 32000 / params_per_row.max(1);
        let chunk_size = chunk_size.max(1); // At least 1 row per chunk

        let mut tx = self.pool.begin().await?;
        let mut total_affected: i64 = 0;

        // Build column names list
        let mut column_names = Vec::new();
        if self.config.auto_columns.id {
            column_names.push("id".to_string());
        }
        for col in &schema.columns {
            if col.column_type.is_generated() {
                continue;
            }
            column_names.push(quote_identifier(&col.name));
        }

        // Process in chunks
        for chunk in validated_instances.chunks(chunk_size) {
            let mut placeholders = Vec::new();
            let mut param_idx = 1;

            for (_, properties_obj) in chunk {
                let mut row_placeholders = Vec::new();
                if self.config.auto_columns.id {
                    row_placeholders.push(format!("${}", param_idx));
                    param_idx += 1;
                }
                for col in &schema.columns {
                    if col.column_type.is_generated() {
                        continue;
                    }
                    match classify_slot(col, properties_obj) {
                        Slot::Default => row_placeholders.push("DEFAULT".to_string()),
                        Slot::TypedNull | Slot::Value(_) => {
                            row_placeholders.push(format!("${}", param_idx));
                            param_idx += 1;
                        }
                    }
                }
                placeholders.push(format!("({})", row_placeholders.join(", ")));
            }

            let insert_sql = format!(
                "INSERT INTO {} ({}) VALUES {}",
                quote_identifier(&schema.table_name),
                column_names.join(", "),
                placeholders.join(", ")
            );

            let mut query = sqlx::query(&insert_sql);

            // Bind values for each row in chunk
            for (instance_id, properties_obj) in chunk {
                if self.config.auto_columns.id {
                    query = query.bind(instance_id);
                }
                for col in &schema.columns {
                    if col.column_type.is_generated() {
                        continue;
                    }
                    query = match classify_slot(col, properties_obj) {
                        Slot::Default => query,
                        Slot::TypedNull => Self::bind_typed_null(query, &col.column_type),
                        Slot::Value(v) => Self::bind_value(query, &col.column_type, &col.name, v)?,
                    };
                }
            }

            let result = query.execute(&mut *tx).await?;
            total_affected += result.rows_affected() as i64;
        }

        tx.commit().await?;

        Ok(total_affected)
    }

    /// Insert or update multiple instances based on conflict columns
    ///
    /// Uses PostgreSQL's ON CONFLICT ... DO UPDATE syntax.
    /// All operations happen in a single transaction.
    ///
    /// # Arguments
    /// * `schema_name` - Name of the schema
    /// * `instances` - Vector of JSON objects to upsert
    /// * `conflict_columns` - Columns that define uniqueness for conflict detection
    ///
    /// # Returns
    /// Number of affected rows (inserts + updates)
    pub async fn upsert_instances(
        &self,
        schema_name: &str,
        instances: Vec<serde_json::Value>,
        conflict_columns: Vec<String>,
    ) -> Result<i64> {
        if instances.is_empty() {
            return Ok(0);
        }

        if instances.len() > self.config.bulk_request_limit {
            return Err(ObjectStoreError::validation(format!(
                "bulk request size {} exceeds limit of {}",
                instances.len(),
                self.config.bulk_request_limit
            )));
        }

        if conflict_columns.is_empty() {
            return Err(ObjectStoreError::validation(
                "At least one conflict column must be specified",
            ));
        }

        let schema = self
            .get_schema(schema_name)
            .await?
            .ok_or_else(|| ObjectStoreError::schema_not_found(schema_name))?;

        // Validate conflict columns exist
        let schema_column_names: std::collections::HashSet<_> =
            schema.columns.iter().map(|c| c.name.as_str()).collect();

        for col_name in &conflict_columns {
            if col_name != "id" && !schema_column_names.contains(col_name.as_str()) {
                return Err(ObjectStoreError::validation(format!(
                    "Conflict column '{}' does not exist in schema",
                    col_name
                )));
            }
        }

        // Pre-validate all instances and generate IDs
        let mut validated_instances: Vec<(String, serde_json::Map<String, serde_json::Value>)> =
            Vec::with_capacity(instances.len());

        for (idx, instance) in instances.iter().enumerate() {
            let properties_obj = instance.as_object().ok_or_else(|| {
                ObjectStoreError::validation(format!(
                    "Instance at index {} must be a JSON object",
                    idx
                ))
            })?;

            // Validate each column
            for col in &schema.columns {
                if col.column_type.is_generated() {
                    continue;
                }
                if let Some(value) = properties_obj.get(&col.name)
                    && let Err(e) = col.column_type.validate_value(value)
                {
                    return Err(ObjectStoreError::validation(format!(
                        "Instance at index {}: Invalid value for column '{}': {}",
                        idx, col.name, e
                    )));
                }
            }

            let instance_id = uuid::Uuid::new_v4().to_string();
            validated_instances.push((instance_id, properties_obj.clone()));
        }

        // Build column names list
        let mut column_names = Vec::new();
        if self.config.auto_columns.id {
            column_names.push("id".to_string());
        }
        for col in &schema.columns {
            if col.column_type.is_generated() {
                continue;
            }
            column_names.push(quote_identifier(&col.name));
        }

        // Build ON CONFLICT column list (same for every group).
        let conflict_cols: Vec<String> = conflict_columns
            .iter()
            .map(|c| quote_identifier(c))
            .collect();
        let conflict_set: std::collections::HashSet<&str> =
            conflict_columns.iter().map(String::as_str).collect();

        // Group rows by UPDATE signature — rows that share the same set of
        // "present non-conflict columns" can share one DO UPDATE SET clause.
        // This way a row's absent columns are never stomped on UPDATE.
        let mut groups: std::collections::HashMap<Vec<String>, Vec<ValidatedRow>> =
            std::collections::HashMap::new();
        for (id, props) in validated_instances {
            let sig = update_signature(&schema, &props, &conflict_set);
            groups.entry(sig).or_default().push((id, props));
        }

        // Calculate chunk size
        let params_per_row = 1 + schema.columns.len();
        let chunk_size = 32000 / params_per_row.max(1);
        let chunk_size = chunk_size.max(1);

        let mut tx = self.pool.begin().await?;
        let mut total_affected: i64 = 0;

        for (signature, group_rows) in groups {
            // DO UPDATE SET lists only columns present in this group's payloads.
            let mut update_sets: Vec<String> = signature
                .iter()
                .map(|name| {
                    let q = quote_identifier(name);
                    format!("{} = EXCLUDED.{}", q, q)
                })
                .collect();
            if self.config.auto_columns.updated_at {
                update_sets.push("updated_at = NOW()".to_string());
            }

            for chunk in group_rows.chunks(chunk_size) {
                let mut placeholders = Vec::new();
                let mut param_idx = 1;

                for (_, properties_obj) in chunk {
                    let mut row_placeholders = Vec::new();
                    if self.config.auto_columns.id {
                        row_placeholders.push(format!("${}", param_idx));
                        param_idx += 1;
                    }
                    for col in &schema.columns {
                        if col.column_type.is_generated() {
                            continue;
                        }
                        match classify_slot(col, properties_obj) {
                            Slot::Default => row_placeholders.push("DEFAULT".to_string()),
                            Slot::TypedNull | Slot::Value(_) => {
                                row_placeholders.push(format!("${}", param_idx));
                                param_idx += 1;
                            }
                        }
                    }
                    placeholders.push(format!("({})", row_placeholders.join(", ")));
                }

                let upsert_sql = if update_sets.is_empty() {
                    // Nothing to update for this group — skip conflicts via DO NOTHING.
                    format!(
                        "INSERT INTO {} ({}) VALUES {} ON CONFLICT ({}) DO NOTHING",
                        quote_identifier(&schema.table_name),
                        column_names.join(", "),
                        placeholders.join(", "),
                        conflict_cols.join(", ")
                    )
                } else {
                    format!(
                        "INSERT INTO {} ({}) VALUES {} ON CONFLICT ({}) DO UPDATE SET {}",
                        quote_identifier(&schema.table_name),
                        column_names.join(", "),
                        placeholders.join(", "),
                        conflict_cols.join(", "),
                        update_sets.join(", ")
                    )
                };

                let mut query = sqlx::query(&upsert_sql);

                for (instance_id, properties_obj) in chunk {
                    if self.config.auto_columns.id {
                        query = query.bind(instance_id);
                    }
                    for col in &schema.columns {
                        if col.column_type.is_generated() {
                            continue;
                        }
                        query = match classify_slot(col, properties_obj) {
                            Slot::Default => query,
                            Slot::TypedNull => Self::bind_typed_null(query, &col.column_type),
                            Slot::Value(v) => {
                                Self::bind_value(query, &col.column_type, &col.name, v)?
                            }
                        };
                    }
                }

                let result = query.execute(&mut *tx).await?;
                total_affected += result.rows_affected() as i64;
            }
        }

        tx.commit().await?;

        Ok(total_affected)
    }

    /// Bulk-create with opt-in conflict and validation handling.
    ///
    /// Unlike [`Self::create_instances`], this method accepts:
    /// - a [`ConflictMode`] choosing between error, skip-on-conflict, or upsert on
    ///   a user-supplied set of conflict columns, and
    /// - a [`ValidationMode`] choosing whether a per-row validation failure aborts
    ///   the whole batch or records the row as skipped and continues.
    ///
    /// Returns a [`BulkCreateResult`] with `created_count`, `skipped_count`, and
    /// per-row `errors` for the rows rejected in `ValidationMode::Skip`.
    pub async fn create_instances_extended(
        &self,
        schema_name: &str,
        instances: Vec<serde_json::Value>,
        opts: crate::instance::BulkCreateOptions,
    ) -> Result<crate::instance::BulkCreateResult> {
        use crate::instance::{BulkCreateResult, BulkRowError, ConflictMode, ValidationMode};

        let mut result = BulkCreateResult::default();
        if instances.is_empty() {
            return Ok(result);
        }

        if instances.len() > self.config.bulk_request_limit {
            return Err(ObjectStoreError::validation(format!(
                "bulk request size {} exceeds limit of {}",
                instances.len(),
                self.config.bulk_request_limit
            )));
        }

        // Validate conflict_columns up front — required for Skip/Upsert.
        let conflict_cols: Option<&[String]> = match &opts.conflict_mode {
            ConflictMode::Error => None,
            ConflictMode::Skip { conflict_columns } | ConflictMode::Upsert { conflict_columns } => {
                if conflict_columns.is_empty() {
                    return Err(ObjectStoreError::validation(
                        "`conflict_columns` must be non-empty when on_conflict is 'skip' or 'upsert'",
                    ));
                }
                Some(conflict_columns.as_slice())
            }
        };

        let schema = self
            .get_schema(schema_name)
            .await?
            .ok_or_else(|| ObjectStoreError::schema_not_found(schema_name))?;

        if let Some(cols) = conflict_cols {
            let known: std::collections::HashSet<&str> =
                schema.columns.iter().map(|c| c.name.as_str()).collect();
            for name in cols {
                if name != "id" && !known.contains(name.as_str()) {
                    return Err(ObjectStoreError::validation(format!(
                        "Conflict column '{}' does not exist in schema",
                        name
                    )));
                }
            }
        }

        // Per-row validation — separate into valid / invalid.
        let mut validated: Vec<(String, serde_json::Map<String, serde_json::Value>)> =
            Vec::with_capacity(instances.len());
        for (idx, instance) in instances.into_iter().enumerate() {
            match validate_instance_for_insert(&schema, &instance) {
                Ok(obj) => {
                    let instance_id = uuid::Uuid::new_v4().to_string();
                    validated.push((instance_id, obj));
                }
                Err(reason) => match opts.validation_mode {
                    ValidationMode::Stop => {
                        return Err(ObjectStoreError::validation(format!(
                            "Instance at index {}: {}",
                            idx, reason
                        )));
                    }
                    ValidationMode::Skip => {
                        result.skipped_count += 1;
                        result.errors.push(BulkRowError { index: idx, reason });
                    }
                },
            }
        }

        if validated.is_empty() {
            return Ok(result);
        }

        // Captured before `validated` is moved into conflict groups.
        let validated_len = validated.len() as i64;

        // Compute chunk size under Postgres' ~32k-param limit.
        let params_per_row = 1 + schema.columns.len();
        let chunk_size = (32000 / params_per_row.max(1)).max(1);

        // Column list (shared across chunks).
        let mut column_names = Vec::new();
        if self.config.auto_columns.id {
            column_names.push("id".to_string());
        }
        for col in &schema.columns {
            if col.column_type.is_generated() {
                continue;
            }
            column_names.push(quote_identifier(&col.name));
        }

        // Build (ON CONFLICT clause, rows) groups. Error and Skip share a
        // single clause across all rows. Upsert groups rows by their UPDATE
        // signature so each group's DO UPDATE SET only touches the columns
        // present in those rows' payloads.
        let conflict_groups: Vec<(String, Vec<ValidatedRow>)> = match &opts.conflict_mode {
            ConflictMode::Error => vec![(String::new(), validated)],
            ConflictMode::Skip { conflict_columns } => {
                let cols: Vec<String> = conflict_columns
                    .iter()
                    .map(|c| quote_identifier(c))
                    .collect();
                let clause = format!(" ON CONFLICT ({}) DO NOTHING", cols.join(", "));
                vec![(clause, validated)]
            }
            ConflictMode::Upsert { conflict_columns } => {
                let cols: Vec<String> = conflict_columns
                    .iter()
                    .map(|c| quote_identifier(c))
                    .collect();
                let conflict_set: std::collections::HashSet<&str> =
                    conflict_columns.iter().map(String::as_str).collect();
                let mut row_groups: std::collections::HashMap<Vec<String>, Vec<ValidatedRow>> =
                    std::collections::HashMap::new();
                for (id, props) in validated {
                    let sig = update_signature(&schema, &props, &conflict_set);
                    row_groups.entry(sig).or_default().push((id, props));
                }
                let updated_at_bump = self.config.auto_columns.updated_at;
                row_groups
                    .into_iter()
                    .map(|(signature, rows)| {
                        let mut update_sets: Vec<String> = signature
                            .iter()
                            .map(|name| {
                                let q = quote_identifier(name);
                                format!("{} = EXCLUDED.{}", q, q)
                            })
                            .collect();
                        if updated_at_bump {
                            update_sets.push("updated_at = NOW()".to_string());
                        }
                        let clause = if update_sets.is_empty() {
                            // Group has nothing to update — skip conflicts.
                            format!(" ON CONFLICT ({}) DO NOTHING", cols.join(", "))
                        } else {
                            format!(
                                " ON CONFLICT ({}) DO UPDATE SET {}",
                                cols.join(", "),
                                update_sets.join(", ")
                            )
                        };
                        (clause, rows)
                    })
                    .collect()
            }
        };

        let mut tx = self.pool.begin().await?;
        let mut db_affected: i64 = 0;

        for (on_conflict_clause, group_rows) in conflict_groups {
            for chunk in group_rows.chunks(chunk_size) {
                let mut placeholders = Vec::new();
                let mut param_idx = 1;
                for (_, properties_obj) in chunk {
                    let mut row_placeholders = Vec::new();
                    if self.config.auto_columns.id {
                        row_placeholders.push(format!("${}", param_idx));
                        param_idx += 1;
                    }
                    for col in &schema.columns {
                        if col.column_type.is_generated() {
                            continue;
                        }
                        match classify_slot(col, properties_obj) {
                            Slot::Default => row_placeholders.push("DEFAULT".to_string()),
                            Slot::TypedNull | Slot::Value(_) => {
                                row_placeholders.push(format!("${}", param_idx));
                                param_idx += 1;
                            }
                        }
                    }
                    placeholders.push(format!("({})", row_placeholders.join(", ")));
                }

                let insert_sql = format!(
                    "INSERT INTO {} ({}) VALUES {}{}",
                    quote_identifier(&schema.table_name),
                    column_names.join(", "),
                    placeholders.join(", "),
                    on_conflict_clause,
                );

                let mut query = sqlx::query(&insert_sql);
                for (instance_id, properties_obj) in chunk {
                    if self.config.auto_columns.id {
                        query = query.bind(instance_id);
                    }
                    for col in &schema.columns {
                        if col.column_type.is_generated() {
                            continue;
                        }
                        query = match classify_slot(col, properties_obj) {
                            Slot::Default => query,
                            Slot::TypedNull => Self::bind_typed_null(query, &col.column_type),
                            Slot::Value(v) => {
                                Self::bind_value(query, &col.column_type, &col.name, v)?
                            }
                        };
                    }
                }

                let executed = query.execute(&mut *tx).await?;
                db_affected += executed.rows_affected() as i64;
            }
        }

        tx.commit().await?;

        // Under Skip conflict, `rows_affected` is the inserted count; the rest
        // were skipped by ON CONFLICT DO NOTHING. Add that delta to skipped.
        if matches!(opts.conflict_mode, ConflictMode::Skip { .. }) && db_affected < validated_len {
            result.skipped_count += validated_len - db_affected;
        }
        result.created_count = db_affected;

        Ok(result)
    }

    /// Update multiple instances by ID, each with its own property values.
    ///
    /// Every `(id, properties)` pair is validated before any write happens;
    /// all updates run inside a single transaction so the operation is atomic.
    ///
    /// Returns the total number of rows affected.
    pub async fn update_instances_by_ids(
        &self,
        schema_name: &str,
        updates: Vec<(String, serde_json::Value)>,
    ) -> Result<i64> {
        if updates.is_empty() {
            return Ok(0);
        }

        if updates.len() > self.config.bulk_request_limit {
            return Err(ObjectStoreError::validation(format!(
                "bulk request size {} exceeds limit of {}",
                updates.len(),
                self.config.bulk_request_limit
            )));
        }

        let schema = self
            .get_schema(schema_name)
            .await?
            .ok_or_else(|| ObjectStoreError::schema_not_found(schema_name))?;

        // Pre-validate every update payload.
        let mut validated: Vec<(String, serde_json::Map<String, serde_json::Value>)> =
            Vec::with_capacity(updates.len());
        for (idx, (id, properties)) in updates.into_iter().enumerate() {
            let properties_obj = properties.as_object().ok_or_else(|| {
                ObjectStoreError::validation(format!(
                    "Update at index {}: properties must be a JSON object",
                    idx
                ))
            })?;
            for col in &schema.columns {
                if col.column_type.is_generated() {
                    continue;
                }
                if let Some(value) = properties_obj.get(&col.name)
                    && let Err(e) = col.column_type.validate_value(value)
                {
                    return Err(ObjectStoreError::validation(format!(
                        "Update at index {}: Invalid value for column '{}': {}",
                        idx, col.name, e
                    )));
                }
            }
            validated.push((id, properties_obj.clone()));
        }

        let mut tx = self.pool.begin().await?;
        let mut total_affected: i64 = 0;

        for (instance_id, properties_obj) in &validated {
            let mut set_clauses: Vec<String> = Vec::new();
            let mut param_idx = 2i32; // $1 = instance_id

            if self.config.auto_columns.updated_at {
                set_clauses.push("updated_at = NOW()".to_string());
            }

            let mut bind_cols: Vec<(&ColumnDefinition, &serde_json::Value)> = Vec::new();
            for col in &schema.columns {
                if col.column_type.is_generated() {
                    continue;
                }
                if let Some(value) = properties_obj.get(&col.name) {
                    set_clauses.push(format!("{} = ${}", quote_identifier(&col.name), param_idx));
                    bind_cols.push((col, value));
                    param_idx += 1;
                }
            }

            // Nothing to update for this row (or only the auto updated_at would change).
            if set_clauses.is_empty()
                || (set_clauses.len() == 1 && self.config.auto_columns.updated_at)
            {
                continue;
            }

            let update_sql = format!(
                "UPDATE {} SET {} WHERE id = $1 AND deleted = FALSE",
                quote_identifier(&schema.table_name),
                set_clauses.join(", "),
            );

            let mut query = sqlx::query(&update_sql).bind(instance_id);
            for (col, value) in &bind_cols {
                query = Self::bind_value(query, &col.column_type, &col.name, value)?;
            }

            let result = query.execute(&mut *tx).await?;
            total_affected += result.rows_affected() as i64;
        }

        tx.commit().await?;

        Ok(total_affected)
    }

    // =========================================================================
    // Internal Helpers
    // =========================================================================

    fn row_to_schema(&self, row: &sqlx::postgres::PgRow) -> Result<Schema> {
        let id: String = row.try_get("id")?;
        let created_at: chrono::DateTime<chrono::Utc> = row.try_get("created_at")?;
        let updated_at: chrono::DateTime<chrono::Utc> = row.try_get("updated_at")?;
        let name: String = row.try_get("name")?;
        let description: Option<String> = row.try_get("description")?;
        let table_name: String = row.try_get("table_name")?;
        let columns: serde_json::Value = row.try_get("columns")?;
        let indexes: Option<serde_json::Value> = row.try_get("indexes")?;

        Ok(Schema {
            id,
            created_at: created_at.to_rfc3339(),
            updated_at: updated_at.to_rfc3339(),
            name,
            description,
            table_name,
            columns: serde_json::from_value(columns).unwrap_or_default(),
            indexes: indexes.and_then(|v| serde_json::from_value(v).ok()),
        })
    }

    async fn filter_instances_internal(
        &self,
        schema: &Schema,
        filter: FilterRequest,
    ) -> Result<(Vec<Instance>, i64)> {
        // Build column list
        let mut select_columns = Vec::new();

        if self.config.auto_columns.id {
            select_columns.push("id".to_string());
        }
        if self.config.auto_columns.created_at {
            select_columns.push("created_at".to_string());
        }
        if self.config.auto_columns.updated_at {
            select_columns.push("updated_at".to_string());
        }

        for col in &schema.columns {
            if col.column_type.is_generated() {
                continue;
            }
            select_columns.push(quote_identifier(&col.name));
        }

        // Build WHERE clause from condition (params: $1..$N1)
        let (where_clause, where_params) = if let Some(condition) = filter.condition {
            let mut param_offset = 1;
            build_condition_clause(&condition, &mut param_offset, schema)
                .map_err(ObjectStoreError::InvalidCondition)?
        } else {
            ("TRUE".to_string(), Vec::new())
        };

        // Validate + render `score_expression` if provided. Score-expression
        // params append after WHERE params, so placeholders continue at
        // $(where_params.len() + 1).
        let mut score_params: Vec<serde_json::Value> = Vec::new();
        let mut score_alias: Option<String> = None;
        if let Some(score_expr) = filter.score_expression.as_ref() {
            validate_score_alias(&score_expr.alias).map_err(ObjectStoreError::validation)?;

            let node: ExprNode =
                serde_json::from_value(score_expr.expression.clone()).map_err(|e| {
                    ObjectStoreError::validation(format!(
                        "score_expression: invalid expression JSON: {}",
                        e
                    ))
                })?;
            validate_row_expression(&node, schema, 0).map_err(ObjectStoreError::validation)?;

            let mut score_offset = (where_params.len() as i32) + 1;
            let score_sql =
                render_row_expression(&node, schema, &mut score_params, &mut score_offset, 0)
                    .map_err(ObjectStoreError::validation)?;

            select_columns.push(format!(
                "{} AS {}",
                score_sql,
                quote_identifier(&score_expr.alias)
            ));
            score_alias = Some(score_expr.alias.clone());
        }

        // Build ORDER BY: prefer the new structured `order_by` if set,
        // otherwise fall back to the legacy `sort_by` / `sort_order`.
        let order_by_clause = if let Some(entries) = filter.order_by.as_ref() {
            render_order_by_entries(entries, schema, score_alias.as_deref())
                .map_err(ObjectStoreError::validation)?
        } else {
            build_order_by_clause(&filter.sort_by, &filter.sort_order, schema)
                .map_err(ObjectStoreError::validation)?
        };

        let base_where = format!("deleted = FALSE AND ({})", where_clause);

        // Count query: only WHERE params bind, no score params (score column
        // isn't referenced from a count(*)).
        let count_query = format!(
            "SELECT COUNT(*) FROM {} WHERE {}",
            quote_identifier(&schema.table_name),
            base_where
        );

        // Select query: WHERE params, then score params, then LIMIT and
        // OFFSET (in that bind order).
        let total_param_count = where_params.len() + score_params.len();
        let select_query = format!(
            "SELECT {} FROM {} WHERE {} ORDER BY {} LIMIT ${} OFFSET ${}",
            select_columns.join(", "),
            quote_identifier(&schema.table_name),
            base_where,
            order_by_clause,
            total_param_count + 1,
            total_param_count + 2
        );

        // Execute count query
        let mut count_query_builder = sqlx::query_as::<_, (i64,)>(&count_query);
        for param in &where_params {
            let param_str = match param {
                serde_json::Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            count_query_builder = count_query_builder.bind(param_str);
        }
        let (total_count,) = count_query_builder.fetch_one(&self.pool).await?;

        // Execute select query
        let mut select_query_builder = sqlx::query(&select_query);
        for param in &where_params {
            let param_str = match param {
                serde_json::Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            select_query_builder = select_query_builder.bind(param_str);
        }
        for param in &score_params {
            let param_str = match param {
                serde_json::Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            select_query_builder = select_query_builder.bind(param_str);
        }
        let rows = select_query_builder
            .bind(filter.limit)
            .bind(filter.offset)
            .fetch_all(&self.pool)
            .await?;

        let instances: Vec<Instance> = rows
            .iter()
            .map(|row| self.row_to_instance(row, schema, score_alias.as_deref()))
            .collect();

        Ok((instances, total_count))
    }

    fn row_to_instance(
        &self,
        row: &sqlx::postgres::PgRow,
        schema: &Schema,
        score_alias: Option<&str>,
    ) -> Instance {
        let id: String = if self.config.auto_columns.id {
            row.try_get("id").unwrap_or_default()
        } else {
            String::new()
        };

        let created_at: String = if self.config.auto_columns.created_at {
            row.try_get::<chrono::DateTime<chrono::Utc>, _>("created_at")
                .map(|dt| dt.to_rfc3339())
                .unwrap_or_default()
        } else {
            String::new()
        };

        let updated_at: String = if self.config.auto_columns.updated_at {
            row.try_get::<chrono::DateTime<chrono::Utc>, _>("updated_at")
                .map(|dt| dt.to_rfc3339())
                .unwrap_or_default()
        } else {
            String::new()
        };

        // Build properties from columns
        let mut properties = serde_json::Map::new();
        for col in &schema.columns {
            if col.column_type.is_generated() {
                continue;
            }
            if let Some(value) = Self::extract_column_value(row, col) {
                properties.insert(col.name.clone(), value);
            }
        }

        // Pull the score-expression column out of the row, if requested.
        // pg_trgm `similarity()` returns `real`; sqlx maps it to `f32`.
        let computed = score_alias.and_then(|alias| {
            row.try_get::<Option<f32>, _>(alias)
                .ok()
                .flatten()
                .and_then(|f| serde_json::Number::from_f64(f as f64))
                .map(|num| {
                    let mut map = serde_json::Map::new();
                    map.insert(alias.to_string(), serde_json::Value::Number(num));
                    map
                })
        });

        Instance {
            id,
            created_at,
            updated_at,
            schema_id: Some(schema.id.clone()),
            schema_name: Some(schema.name.clone()),
            properties: serde_json::Value::Object(properties),
            computed,
        }
    }

    fn extract_column_value(
        row: &sqlx::postgres::PgRow,
        col: &ColumnDefinition,
    ) -> Option<serde_json::Value> {
        match &col.column_type {
            ColumnType::String | ColumnType::Enum { .. } => row
                .try_get::<Option<String>, _>(col.name.as_str())
                .ok()
                .flatten()
                .map(serde_json::Value::String),
            ColumnType::Integer => row
                .try_get::<Option<i64>, _>(col.name.as_str())
                .ok()
                .flatten()
                .map(|v| serde_json::Value::Number(serde_json::Number::from(v))),
            ColumnType::Decimal { .. } => {
                use rust_decimal::prelude::ToPrimitive;
                row.try_get::<Option<rust_decimal::Decimal>, _>(col.name.as_str())
                    .ok()
                    .flatten()
                    .and_then(|d| d.to_f64())
                    .and_then(serde_json::Number::from_f64)
                    .map(serde_json::Value::Number)
            }
            ColumnType::Boolean => row
                .try_get::<Option<bool>, _>(col.name.as_str())
                .ok()
                .flatten()
                .map(serde_json::Value::Bool),
            ColumnType::Timestamp => row
                .try_get::<Option<chrono::DateTime<chrono::Utc>>, _>(col.name.as_str())
                .ok()
                .flatten()
                .map(|v| serde_json::Value::String(v.to_rfc3339())),
            ColumnType::Json => row
                .try_get::<Option<serde_json::Value>, _>(col.name.as_str())
                .ok()
                .flatten(),
            // Generated tsvector columns are not surfaced in row payloads —
            // their printed form (`'foo':1 'bar':2`) is noise for clients;
            // queryable access is via MATCH / TS_RANK.
            ColumnType::Tsvector { .. } => None,
            ColumnType::Vector { .. } => row
                .try_get::<Option<pgvector::Vector>, _>(col.name.as_str())
                .ok()
                .flatten()
                .map(|v| {
                    serde_json::Value::Array(
                        v.to_vec()
                            .into_iter()
                            .filter_map(|f| serde_json::Number::from_f64(f as f64))
                            .map(serde_json::Value::Number)
                            .collect(),
                    )
                }),
        }
    }

    fn bind_value<'q>(
        query: sqlx::query::Query<'q, sqlx::Postgres, sqlx::postgres::PgArguments>,
        column_type: &ColumnType,
        column_name: &str,
        value: &'q serde_json::Value,
    ) -> Result<sqlx::query::Query<'q, sqlx::Postgres, sqlx::postgres::PgArguments>> {
        Ok(match column_type {
            ColumnType::String | ColumnType::Enum { .. } => {
                if value.is_null() {
                    query.bind(None::<String>)
                } else {
                    query.bind(value.as_str().ok_or_else(|| {
                        ObjectStoreError::validation(format!(
                            "Column '{}' expected string",
                            column_name
                        ))
                    })?)
                }
            }
            ColumnType::Integer => {
                if value.is_null() {
                    query.bind(None::<i64>)
                } else {
                    let int_val = value
                        .as_i64()
                        .or_else(|| value.as_str().and_then(|s| s.parse::<i64>().ok()))
                        .ok_or_else(|| {
                            ObjectStoreError::validation(format!(
                                "Column '{}' expected integer",
                                column_name
                            ))
                        })?;
                    query.bind(int_val)
                }
            }
            ColumnType::Decimal { .. } => {
                if value.is_null() {
                    query.bind(None::<f64>)
                } else {
                    let dec_val = value
                        .as_f64()
                        .or_else(|| value.as_str().and_then(|s| s.parse::<f64>().ok()))
                        .ok_or_else(|| {
                            ObjectStoreError::validation(format!(
                                "Column '{}' expected decimal",
                                column_name
                            ))
                        })?;
                    query.bind(dec_val)
                }
            }
            ColumnType::Boolean => {
                if value.is_null() {
                    query.bind(None::<bool>)
                } else {
                    let bool_val = value
                        .as_bool()
                        .or_else(|| {
                            value
                                .as_str()
                                .and_then(|s| match s.to_lowercase().as_str() {
                                    "true" | "1" | "yes" => Some(true),
                                    "false" | "0" | "no" => Some(false),
                                    _ => None,
                                })
                        })
                        .ok_or_else(|| {
                            ObjectStoreError::validation(format!(
                                "Column '{}' expected boolean",
                                column_name
                            ))
                        })?;
                    query.bind(bool_val)
                }
            }
            ColumnType::Timestamp => {
                if value.is_null() {
                    query.bind(None::<chrono::DateTime<chrono::Utc>>)
                } else {
                    let timestamp_str = value.as_str().ok_or_else(|| {
                        ObjectStoreError::validation(format!(
                            "Column '{}' expected timestamp string",
                            column_name
                        ))
                    })?;
                    let timestamp = chrono::DateTime::parse_from_rfc3339(timestamp_str)
                        .map_err(|e| {
                            ObjectStoreError::validation(format!(
                                "Column '{}' has invalid timestamp: {}",
                                column_name, e
                            ))
                        })?
                        .with_timezone(&chrono::Utc);
                    query.bind(timestamp)
                }
            }
            ColumnType::Json => query.bind(value),
            // Defensive: every iteration over `schema.columns` skips
            // generated columns before reaching this path. If we ever do
            // hit it, surface a clear error rather than silently corrupting
            // an INSERT/UPDATE.
            ColumnType::Tsvector { .. } => {
                return Err(ObjectStoreError::validation(format!(
                    "internal error: attempted to bind a value to generated tsvector column '{}'",
                    column_name
                )));
            }
            ColumnType::Vector { dimension, .. } => {
                if value.is_null() {
                    query.bind(None::<pgvector::Vector>)
                } else {
                    let arr = value.as_array().ok_or_else(|| {
                        ObjectStoreError::validation(format!(
                            "Column '{}' expected JSON array of numbers for vector",
                            column_name
                        ))
                    })?;
                    if arr.len() as u32 != *dimension {
                        return Err(ObjectStoreError::validation(format!(
                            "Column '{}' vector dimension mismatch: expected {}, got {}",
                            column_name,
                            dimension,
                            arr.len()
                        )));
                    }
                    let floats: Vec<f32> = arr
                        .iter()
                        .map(|v| {
                            v.as_f64().map(|f| f as f32).ok_or_else(|| {
                                ObjectStoreError::validation(format!(
                                    "Column '{}' vector element is not a number",
                                    column_name
                                ))
                            })
                        })
                        .collect::<Result<_>>()?;
                    query.bind(pgvector::Vector::from(floats))
                }
            }
        })
    }

    /// Bind a typed SQL NULL (`None::<T>`) for the given column type.
    ///
    /// Used by the bulk-insert path when a column is absent from the payload
    /// and has no declared DB default: we still need a typed placeholder so
    /// Postgres can infer the column's type (and, for `Json`, so it writes
    /// SQL NULL rather than JSONB `null`).
    fn bind_typed_null<'q>(
        query: sqlx::query::Query<'q, sqlx::Postgres, sqlx::postgres::PgArguments>,
        column_type: &ColumnType,
    ) -> sqlx::query::Query<'q, sqlx::Postgres, sqlx::postgres::PgArguments> {
        match column_type {
            ColumnType::String | ColumnType::Enum { .. } => query.bind(None::<String>),
            ColumnType::Integer => query.bind(None::<i64>),
            ColumnType::Decimal { .. } => query.bind(None::<f64>),
            ColumnType::Boolean => query.bind(None::<bool>),
            ColumnType::Timestamp => query.bind(None::<chrono::DateTime<chrono::Utc>>),
            ColumnType::Json => query.bind(None::<serde_json::Value>),
            // Same defensive handling as `bind_value` — should never be
            // reached because callers filter generated columns out first.
            ColumnType::Tsvector { .. } => query.bind(None::<String>),
            ColumnType::Vector { .. } => query.bind(None::<pgvector::Vector>),
        }
    }
}

/// Per-column slot in a bulk-insert VALUES tuple.
///
/// Preserves the distinction between "payload omitted this key" and "payload
/// set this key to null" — which Postgres cares about for (a) firing declared
/// `DEFAULT` clauses and (b) writing SQL NULL vs JSONB `null` on JSON columns.
enum Slot<'a> {
    /// Key absent and the column declares a DB default — emit literal `DEFAULT`.
    Default,
    /// Key absent with no default — emit `$N`, bind typed `None::<T>` (SQL NULL).
    TypedNull,
    /// Key present (including explicit null) — emit `$N`, bind via [`ObjectStore::bind_value`].
    Value(&'a serde_json::Value),
}

/// Classify a (column, row-payload) pair into the correct [`Slot`] variant.
fn classify_slot<'a>(
    col: &ColumnDefinition,
    properties_obj: &'a serde_json::Map<String, serde_json::Value>,
) -> Slot<'a> {
    match properties_obj.get(&col.name) {
        None if col.default_value.is_some() => Slot::Default,
        None => Slot::TypedNull,
        Some(v) => Slot::Value(v),
    }
}

/// Compute a row's UPDATE signature: the schema column names, in schema order,
/// that are both (a) not in the conflict-column set and (b) present in the
/// payload. Used by the upsert paths to group rows so each group's
/// `ON CONFLICT ... DO UPDATE SET` only touches columns the caller actually
/// provided — absent columns keep their stored value (or fall back to
/// `DO NOTHING` if the group has nothing to update).
fn update_signature(
    schema: &Schema,
    properties_obj: &serde_json::Map<String, serde_json::Value>,
    conflict_cols: &std::collections::HashSet<&str>,
) -> Vec<String> {
    schema
        .columns
        .iter()
        .filter(|col| !conflict_cols.contains(col.name.as_str()))
        .filter(|col| properties_obj.contains_key(&col.name))
        .map(|col| col.name.clone())
        .collect()
}

/// Validate a single instance payload for an INSERT operation.
///
/// Returns the payload's object form on success, or a human-readable reason
/// string on failure. Used by `create_instances_extended` to partition rows
/// when `ValidationMode::Skip` is selected.
fn validate_instance_for_insert(
    schema: &Schema,
    instance: &serde_json::Value,
) -> std::result::Result<serde_json::Map<String, serde_json::Value>, String> {
    let properties_obj = instance
        .as_object()
        .ok_or_else(|| "properties must be a JSON object".to_string())?;

    for col in &schema.columns {
        if col.column_type.is_generated() {
            if let Some(v) = properties_obj.get(&col.name)
                && !v.is_null()
            {
                return Err(format!(
                    "Column '{}' is generated and cannot be set",
                    col.name
                ));
            }
            continue;
        }
        if let Some(value) = properties_obj.get(&col.name) {
            if let Err(e) = col.column_type.validate_value(value) {
                return Err(format!("Invalid value for column '{}': {}", col.name, e));
            }
            if !col.nullable && value.is_null() {
                return Err(format!("Column '{}' does not allow NULL values", col.name));
            }
        } else if !col.nullable && col.default_value.is_none() {
            return Err(format!("Required column '{}' is missing", col.name));
        }
    }

    Ok(properties_obj.clone())
}

/// Validate the alias on a [`ScoreExpression`]. Mirrors the rule used by
/// aggregate aliases: `[a-zA-Z_][a-zA-Z0-9_]*`.
fn validate_score_alias(alias: &str) -> std::result::Result<(), String> {
    if alias.is_empty() {
        return Err("score_expression alias cannot be empty".to_string());
    }
    let mut chars = alias.chars();
    let first = chars.next().unwrap();
    if !(first.is_ascii_alphabetic() || first == '_') {
        return Err(format!(
            "score_expression alias '{}' must start with a letter or underscore",
            alias
        ));
    }
    if !chars.all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return Err(format!(
            "score_expression alias '{}' must match [a-zA-Z_][a-zA-Z0-9_]*",
            alias
        ));
    }
    Ok(())
}

/// Render structured `order_by` entries to a SQL ORDER BY clause body. Each
/// entry's target is either a schema column (validated like the legacy
/// `sort_by`) or the alias declared on `score_expression`.
fn render_order_by_entries(
    entries: &[OrderByEntry],
    schema: &Schema,
    score_alias: Option<&str>,
) -> std::result::Result<String, String> {
    if entries.is_empty() {
        return Ok("created_at ASC".to_string());
    }

    let system_fields = ["id", "createdAt", "updatedAt", "created_at", "updated_at"];
    let mut parts = Vec::with_capacity(entries.len());

    for entry in entries {
        match &entry.expression {
            OrderByTarget::Column { name } => {
                let sql_field = field_to_sql(name);
                let is_system =
                    system_fields.contains(&name.as_str()) || system_fields.contains(&sql_field);
                let is_schema_column = schema.columns.iter().any(|c| c.name == *name);
                if !is_system && !is_schema_column {
                    return Err(format!(
                        "Invalid order_by column: '{}'. Must be a system field or schema column.",
                        name
                    ));
                }
                parts.push(format!(
                    "{} {}",
                    quote_identifier(sql_field),
                    entry.direction.as_sql()
                ));
            }
            OrderByTarget::Alias { name } => {
                if score_alias.map(|a| a == name).unwrap_or(false) {
                    parts.push(format!(
                        "{} {}",
                        quote_identifier(name),
                        entry.direction.as_sql()
                    ));
                } else {
                    return Err(format!(
                        "order_by alias '{}' does not match a declared score_expression alias",
                        name
                    ));
                }
            }
        }
    }

    Ok(parts.join(", "))
}
