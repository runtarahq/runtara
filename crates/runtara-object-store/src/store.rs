//! ObjectStore - Main entry point for schema-driven PostgreSQL object storage
//!
//! This module provides the main `ObjectStore` struct that manages dynamic schemas
//! and their instances in a PostgreSQL database.

use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use sqlx::{PgPool, Row};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use crate::config::{DEFAULT_AGGREGATE_RESULT_ROW_LIMIT, StoreConfig};
use crate::error::{ObjectStoreError, Result};
use crate::instance::{Condition, FilterRequest, Instance, SimpleFilter};
use crate::schema::{CreateSchemaRequest, Schema, UpdateSchemaRequest};
use crate::sql::aggregate::{
    AggregateRequest, AggregateResult, build_aggregate_query_with_subqueries,
};
use crate::sql::condition::collect_condition_subquery_schema_names;
use crate::sql::ddl::DdlGenerator;
use crate::sql::sanitize::quote_identifier;
use crate::types::{ColumnDefinition, ColumnType, IndexDefinition};
use runtara_object_model_core::bulk::{
    Slot, classify_slot, live_conflict_target, update_signature,
};

/// A validated row destined for bulk insert: (generated id, payload map).
type ValidatedRow = (String, serde_json::Map<String, serde_json::Value>);

/// Schema-driven dynamic PostgreSQL object store
///
/// Manages schemas and instances in a single PostgreSQL database.
/// Schema metadata is stored in a configurable metadata table (default: `__schema`).
/// Instance data is stored in dynamically created tables.
/// How long a schema definition stays cached before the next read re-fetches
/// it. Schema reads sit on the hot path of every query (filter and aggregate
/// each do a `get_schema` round trip before the real query); caching collapses
/// that to one fetch per schema per window. The short TTL bounds staleness, and
/// local mutations invalidate eagerly so a same-process schema edit is visible
/// immediately.
const SCHEMA_CACHE_TTL: Duration = Duration::from_secs(30);

struct CachedSchema {
    schema: Arc<Schema>,
    expires_at: Instant,
}

pub struct ObjectStore {
    /// Database connection pool
    pool: PgPool,
    /// Store configuration
    config: StoreConfig,
    /// Short-TTL cache of schema metadata keyed by name.
    schema_cache_by_name: RwLock<HashMap<String, CachedSchema>>,
    /// Short-TTL cache of schema metadata keyed by id.
    schema_cache_by_id: RwLock<HashMap<String, CachedSchema>>,
    metadata_ready: tokio::sync::OnceCell<()>,
}

impl ObjectStore {
    /// Create a new ObjectStore from configuration
    ///
    /// This will:
    /// 1. Connect to the database
    /// 2. Create the metadata table if it doesn't exist
    ///
    /// Required Postgres extensions (`pg_trgm`, `vector`, `fuzzystrmatch`) are
    /// **not** created here. Provisioning the database with those extensions is
    /// the operator's responsibility — managed Postgres typically withholds the
    /// superuser/`CREATE EXTENSION` privilege from application roles, so doing
    /// it at runtime fails even when a DBA has already installed them.
    pub async fn new(config: StoreConfig) -> Result<Self> {
        let store = Self::connect(config).await?;
        store.initialize_object_model().await?;
        Ok(store)
    }

    /// Open a native SQL pool without creating Object Model tables or running DDL.
    /// Object Model clients explicitly initialize their own metadata.
    pub async fn connect(config: StoreConfig) -> Result<Self> {
        let connect_options = config
            .database_url
            .parse::<PgConnectOptions>()
            .map_err(|e| ObjectStoreError::Connection(format!("Invalid database_url: {}", e)))?
            .application_name("runtara-object-model")
            .statement_cache_capacity(config.pool.statement_cache_capacity);

        let pool = PgPoolOptions::new()
            .max_connections(config.pool.max_connections)
            .min_connections(config.pool.min_connections)
            .acquire_timeout(config.pool.acquire_timeout)
            .idle_timeout(config.pool.idle_timeout)
            .max_lifetime(config.pool.max_lifetime)
            .test_before_acquire(config.pool.test_before_acquire)
            .connect_with(connect_options)
            .await
            .map_err(|e| {
                ObjectStoreError::Connection(format!("Database connection failed: {}", e))
            })?;

        let store = Self {
            pool,
            config,
            schema_cache_by_name: RwLock::new(HashMap::new()),
            schema_cache_by_id: RwLock::new(HashMap::new()),
            metadata_ready: tokio::sync::OnceCell::new(),
        };
        Ok(store)
    }

    /// Create a new ObjectStore from an existing pool
    ///
    /// Use this when you already have a connection pool and want to
    /// share it with the object store.
    pub async fn from_pool(pool: PgPool, config: StoreConfig) -> Result<Self> {
        let store = Self {
            pool,
            config,
            schema_cache_by_name: RwLock::new(HashMap::new()),
            schema_cache_by_id: RwLock::new(HashMap::new()),
            metadata_ready: tokio::sync::OnceCell::new(),
        };
        store.initialize_object_model().await?;
        Ok(store)
    }

    /// Get a reference to the connection pool
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// Get a reference to the configuration
    pub fn config(&self) -> &StoreConfig {
        &self.config
    }

    /// Initialize metadata only for native public Object Model clients.
    /// The generic SQL host uses `connect` and never calls this method.
    pub async fn initialize_object_model(&self) -> Result<()> {
        self.metadata_ready
            .get_or_try_init(|| async {
                let mut transaction = self.pool.begin().await?;
                sqlx::query(DdlGenerator::METADATA_LOCK_SQL)
                    .bind(&self.config.metadata_table)
                    .execute(&mut *transaction)
                    .await?;
                sqlx::query(&DdlGenerator::new(&self.config).generate_metadata_table())
                    .execute(&mut *transaction)
                    .await?;
                transaction.commit().await?;
                Ok::<(), ObjectStoreError>(())
            })
            .await?;
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
        let metadata_table = quote_identifier(&self.config.metadata_table);
        let mut tx = self.pool.begin().await?;

        let active_schema_sql = format!(
            "SELECT 1 FROM {} WHERE name = $1 AND deleted = FALSE",
            metadata_table
        );
        if sqlx::query(&active_schema_sql)
            .bind(&request.name)
            .fetch_optional(&mut *tx)
            .await?
            .is_some()
        {
            return Err(ObjectStoreError::conflict(format!(
                "Schema '{}' already exists",
                request.name
            )));
        }

        let active_table_sql = format!(
            "SELECT 1 FROM {} WHERE table_name = $1 AND deleted = FALSE",
            metadata_table
        );
        if sqlx::query(&active_table_sql)
            .bind(&request.table_name)
            .fetch_optional(&mut *tx)
            .await?
            .is_some()
        {
            return Err(ObjectStoreError::conflict(format!(
                "Table '{}' already exists",
                request.table_name
            )));
        }

        self.tombstone_deleted_schema_rows(&mut tx, &request.name, &request.table_name)
            .await?;

        let schema_id = uuid::Uuid::new_v4().to_string();

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
            .fetch_one(&mut *tx)
            .await?;

        let created_at: chrono::DateTime<chrono::Utc> = row.try_get("created_at")?;
        let updated_at: chrono::DateTime<chrono::Utc> = row.try_get("updated_at")?;

        // Create the data table
        let ddl = DdlGenerator::new(&self.config);
        let create_table_sql = ddl.generate_create_table(&request.table_name, &request.columns);
        sqlx::query(&create_table_sql).execute(&mut *tx).await?;

        // Create default index
        let default_index_sql = ddl.generate_default_index(&request.table_name);
        sqlx::query(&default_index_sql).execute(&mut *tx).await?;

        // Create partial unique indexes for column-level uniqueness. Inline
        // UNIQUE constraints would include tombstoned rows.
        for unique_sql in ddl.generate_unique_column_indexes(&request.table_name, &request.columns)
        {
            sqlx::query(&unique_sql).execute(&mut *tx).await?;
        }

        // Create trigram (`gin_trgm_ops`) indexes for any column that wants
        // them. Empty if no column has `text_index = trigram`.
        for trigram_sql in ddl.generate_trigram_indexes(&request.table_name, &request.columns) {
            sqlx::query(&trigram_sql).execute(&mut *tx).await?;
        }

        // Create GIN indexes for any tsvector-typed columns. Tsvector
        // columns are useless without a GIN index — full-text queries fall
        // back to seq scans otherwise.
        for tsv_sql in ddl.generate_tsvector_indexes(&request.table_name, &request.columns) {
            sqlx::query(&tsv_sql).execute(&mut *tx).await?;
        }

        // Create HNSW / IVFFlat indexes for any vector-typed columns whose
        // declaration opts in to an index method. Without an index, KNN
        // queries fall back to a seq scan with exact distance computation.
        for vec_sql in ddl.generate_vector_indexes(&request.table_name, &request.columns) {
            sqlx::query(&vec_sql).execute(&mut *tx).await?;
        }

        // Create any specified indexes
        if let Some(indexes) = &request.indexes {
            for index in indexes {
                let index_sql = ddl.generate_create_index(&request.table_name, index);
                sqlx::query(&index_sql).execute(&mut *tx).await?;
            }
        }

        tx.commit().await?;

        let schema = Schema {
            id: schema_id,
            created_at: created_at.to_rfc3339(),
            updated_at: updated_at.to_rfc3339(),
            name: request.name,
            description: request.description,
            table_name: request.table_name,
            columns: request.columns,
            indexes: request.indexes,
        };
        self.cache_schema(&schema);
        Ok(schema)
    }

    /// Get schema by name (cached; see [`SCHEMA_CACHE_TTL`]).
    pub async fn get_schema(&self, name: &str) -> Result<Option<Schema>> {
        if let Some(schema) = Self::schema_from_cache(&self.schema_cache_by_name, name) {
            return Ok(Some((*schema).clone()));
        }
        let fetched = self.get_schema_uncached(name).await?;
        if let Some(schema) = &fetched {
            self.cache_schema(schema);
        }
        Ok(fetched)
    }

    /// Get schema by name, bypassing the cache. Used by mutations that must diff
    /// against the current persisted state.
    async fn get_schema_uncached(&self, name: &str) -> Result<Option<Schema>> {
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

    /// Get schema by ID (cached; see [`SCHEMA_CACHE_TTL`]).
    pub async fn get_schema_by_id(&self, id: &str) -> Result<Option<Schema>> {
        if let Some(schema) = Self::schema_from_cache(&self.schema_cache_by_id, id) {
            return Ok(Some((*schema).clone()));
        }
        let fetched = self.get_schema_by_id_uncached(id).await?;
        if let Some(schema) = &fetched {
            self.cache_schema(schema);
        }
        Ok(fetched)
    }

    /// Get schema by ID, bypassing the cache.
    async fn get_schema_by_id_uncached(&self, id: &str) -> Result<Option<Schema>> {
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

    /// Look up a live (unexpired) schema from one of the caches.
    fn schema_from_cache(
        cache: &RwLock<HashMap<String, CachedSchema>>,
        key: &str,
    ) -> Option<Arc<Schema>> {
        let guard = cache.read().unwrap();
        let entry = guard.get(key)?;
        if entry.expires_at > Instant::now() {
            Some(entry.schema.clone())
        } else {
            None
        }
    }

    /// Insert (or refresh) a schema in both caches.
    fn cache_schema(&self, schema: &Schema) {
        let arc = Arc::new(schema.clone());
        let expires_at = Instant::now() + SCHEMA_CACHE_TTL;
        self.schema_cache_by_name.write().unwrap().insert(
            schema.name.clone(),
            CachedSchema {
                schema: arc.clone(),
                expires_at,
            },
        );
        self.schema_cache_by_id.write().unwrap().insert(
            schema.id.clone(),
            CachedSchema {
                schema: arc,
                expires_at,
            },
        );
    }

    /// Drop a schema from both caches (after a mutation).
    fn invalidate_schema(&self, name: &str, id: &str) {
        self.schema_cache_by_name.write().unwrap().remove(name);
        self.schema_cache_by_id.write().unwrap().remove(id);
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
            .get_schema_uncached(name)
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

        let mut tx = self.pool.begin().await?;

        let ddl = DdlGenerator::new(&self.config);

        // Alter table if columns changed
        if let Some(new_columns) = &request.columns {
            let renames = &request.column_renames;

            // Validate declared renames against the actual columns: the source
            // must exist now and the target must be present in the new list, so
            // a typo can't silently turn into a drop + add.
            for r in renames {
                if r.from == r.to {
                    continue;
                }
                if !existing.columns.iter().any(|c| c.name == r.from) {
                    return Err(ObjectStoreError::validation(format!(
                        "column rename source '{}' does not exist on schema '{}'",
                        r.from, existing.name
                    )));
                }
                if !new_columns.iter().any(|c| c.name == r.to) {
                    return Err(ObjectStoreError::validation(format!(
                        "column rename target '{}' is not present in the new column list",
                        r.to
                    )));
                }
                if existing.columns.iter().any(|c| c.name == r.to) {
                    return Err(ObjectStoreError::validation(format!(
                        "column rename target '{}' already exists on schema '{}'",
                        r.to, existing.name
                    )));
                }
            }
            // A source/target may each appear in only one rename.
            for (i, r) in renames.iter().enumerate() {
                if renames[..i].iter().any(|p| p.from == r.from) {
                    return Err(ObjectStoreError::validation(format!(
                        "duplicate column rename source '{}'",
                        r.from
                    )));
                }
                if renames[..i].iter().any(|p| p.to == r.to) {
                    return Err(ObjectStoreError::validation(format!(
                        "duplicate column rename target '{}'",
                        r.to
                    )));
                }
            }

            // Destructive-change guard: a column present now but absent from the
            // new list (and not declared as a rename source) would be dropped,
            // losing its data. Reject unless the caller acknowledges it — this is
            // the safety net that stops an undeclared rename from silently
            // destroying a column.
            if !request.allow_destructive {
                let dropped: Vec<&str> = existing
                    .columns
                    .iter()
                    .filter(|c| {
                        !renames.iter().any(|r| r.from == c.name)
                            && !new_columns.iter().any(|n| n.name == c.name)
                    })
                    .map(|c| c.name.as_str())
                    .collect();
                if !dropped.is_empty() {
                    return Err(ObjectStoreError::validation(format!(
                        "update would drop column(s) [{}] on schema '{}', losing their data; \
                         declare a rename via `column_renames` or set `allow_destructive = true` \
                         to proceed",
                        dropped.join(", "),
                        existing.name
                    )));
                }
            }

            let alter_statements = ddl.generate_alter_table_with_renames(
                &existing.table_name,
                &existing.columns,
                new_columns,
                renames,
            );

            for statement in alter_statements {
                sqlx::query(&statement).execute(&mut *tx).await?;
            }
        }

        // Reconcile explicit schema indexes when the update provides a full
        // replacement list. Omitted indexes mean "leave unchanged".
        if let Some(new_indexes) = &request.indexes {
            let old_indexes: &[IndexDefinition] = existing.indexes.as_deref().unwrap_or(&[]);
            for statement in
                ddl.generate_alter_indexes(&existing.table_name, old_indexes, new_indexes)
            {
                sqlx::query(&statement).execute(&mut *tx).await?;
            }
        }

        let row = query.fetch_one(&mut *tx).await?;
        let schema = self.row_to_schema(&row)?;

        if request.columns.is_some() {
            self.verify_table_has_expected_columns(&mut tx, &schema.table_name, &schema.columns)
                .await?;
        }

        tx.commit().await?;

        // Drop the old entry (the name may have changed) and cache the new one.
        self.invalidate_schema(name, &existing.id);
        self.cache_schema(&schema);

        Ok(schema)
    }

    /// Delete a schema
    ///
    /// If soft_delete is enabled, marks the schema as deleted.
    /// Otherwise, drops the table and removes the metadata.
    pub async fn delete_schema(&self, name: &str) -> Result<()> {
        let schema = self
            .get_schema_uncached(name)
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

        self.invalidate_schema(name, &schema.id);

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

        let plan = runtara_object_model_core::planning::plan_insert(
            &self.config,
            &schema,
            &properties,
            &instance_id,
        )
        .map_err(ObjectStoreError::validation)?;
        let insert_sql = plan.sql;

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

        let subquery_schemas = self
            .resolve_condition_subquery_schemas(request.condition.as_ref())
            .await?;
        let sql = build_aggregate_query_with_subqueries(&schema, &request, &subquery_schemas)
            .map_err(ObjectStoreError::InvalidCondition)?;

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

        let Some(plan) = runtara_object_model_core::planning::plan_update(
            &self.config,
            &schema,
            &properties,
            instance_id,
        )
        .map_err(ObjectStoreError::validation)?
        else {
            return Ok(());
        };
        let update_sql = plan.sql;

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

        let plan =
            runtara_object_model_core::planning::plan_delete(&self.config, &schema, instance_id);
        let result = sqlx::query(&plan.sql)
            .bind(instance_id)
            .execute(&self.pool)
            .await?;

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

        let subqueries = self
            .resolve_condition_subquery_schemas(Some(&condition))
            .await?;
        let Some(plan) = runtara_object_model_core::planning::plan_update_where(
            &self.config,
            &schema,
            &properties,
            &condition,
            &subqueries,
        )
        .map_err(ObjectStoreError::validation)?
        else {
            return Ok(0);
        };
        let mut tx = self.pool.begin().await?;
        let query = crate::database::bind(sqlx::query(&plan.sql), &plan.params)
            .map_err(|error| ObjectStoreError::validation(error.to_string()))?;
        let affected = query.execute(&mut *tx).await?.rows_affected() as i64;
        tx.commit().await?;
        Ok(affected)
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

        let subqueries = self
            .resolve_condition_subquery_schemas(Some(&condition))
            .await?;
        let plan = runtara_object_model_core::planning::plan_delete_where(
            &self.config,
            &schema,
            &condition,
            &subqueries,
        )
        .map_err(ObjectStoreError::InvalidCondition)?;
        let mut tx = self.pool.begin().await?;
        let query = crate::database::bind(sqlx::query(&plan.sql), &plan.params)
            .map_err(|error| ObjectStoreError::validation(error.to_string()))?;
        let affected = query.execute(&mut *tx).await?.rows_affected() as i64;
        tx.commit().await?;
        Ok(affected)
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
        let conflict_target = live_conflict_target(&conflict_cols);
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
                        "INSERT INTO {} ({}) VALUES {} ON CONFLICT {} DO NOTHING",
                        quote_identifier(&schema.table_name),
                        column_names.join(", "),
                        placeholders.join(", "),
                        conflict_target
                    )
                } else {
                    format!(
                        "INSERT INTO {} ({}) VALUES {} ON CONFLICT {} DO UPDATE SET {}",
                        quote_identifier(&schema.table_name),
                        column_names.join(", "),
                        placeholders.join(", "),
                        conflict_target,
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
        if instances.is_empty() {
            return Ok(crate::instance::BulkCreateResult::default());
        }
        let schema = self
            .get_schema(schema_name)
            .await?
            .ok_or_else(|| ObjectStoreError::schema_not_found(schema_name))?;
        let plan = runtara_object_model_core::bulk::plan_bulk_create(
            &self.config,
            &schema,
            instances,
            opts,
            || uuid::Uuid::new_v4().to_string(),
        )
        .map_err(|error| ObjectStoreError::validation(error.to_string()))?;
        if plan.statements.is_empty() {
            return Ok(plan.finish(0));
        }
        let mut tx = self.pool.begin().await?;
        let mut affected = 0i64;
        for statement in &plan.statements {
            let query = crate::database::bind(sqlx::query(&statement.sql), &statement.params)
                .map_err(|error| ObjectStoreError::validation(error.to_string()))?;
            affected += query.execute(&mut *tx).await?.rows_affected() as i64;
        }
        tx.commit().await?;
        Ok(plan.finish(affected))
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

        let statements =
            runtara_object_model_core::planning::plan_update_by_ids(&self.config, &schema, updates)
                .map_err(ObjectStoreError::validation)?;
        let mut tx = self.pool.begin().await?;
        let mut affected = 0i64;
        for plan in &statements {
            let query = crate::database::bind(sqlx::query(&plan.sql), &plan.params)
                .map_err(|error| ObjectStoreError::validation(error.to_string()))?;
            affected += query.execute(&mut *tx).await?.rows_affected() as i64;
        }
        tx.commit().await?;
        Ok(affected)
    }

    async fn tombstone_deleted_schema_rows(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        name: &str,
        table_name: &str,
    ) -> Result<()> {
        let metadata_table = quote_identifier(&self.config.metadata_table);
        let select_sql = format!(
            r#"
            SELECT id, table_name
            FROM {}
            WHERE deleted = TRUE AND (name = $1 OR table_name = $2)
            FOR UPDATE
            "#,
            metadata_table
        );

        let rows = sqlx::query(&select_sql)
            .bind(name)
            .bind(table_name)
            .fetch_all(&mut **tx)
            .await?;

        for row in rows {
            let id: String = row.try_get("id")?;
            let old_table_name: String = row.try_get("table_name")?;
            let tombstone_name = Self::tombstone_name("schema");
            let tombstone_table_name = Self::tombstone_name("table");

            if Self::table_exists(tx, &old_table_name).await? {
                let indexes: Vec<String> = sqlx::query_scalar(
                    "SELECT indexname FROM pg_indexes WHERE schemaname=current_schema() AND tablename=$1"
                ).bind(&old_table_name).fetch_all(&mut **tx).await?;
                for sql in DdlGenerator::tombstone_table(
                    &old_table_name,
                    &tombstone_table_name,
                    &indexes,
                    || Self::tombstone_name("index"),
                ) {
                    sqlx::query(&sql).execute(&mut **tx).await?;
                }
            }

            let update_sql = format!(
                r#"
                UPDATE {}
                SET name = $2,
                    table_name = $3,
                    updated_at = NOW()
                WHERE id = $1
                "#,
                metadata_table
            );

            sqlx::query(&update_sql)
                .bind(id)
                .bind(tombstone_name)
                .bind(tombstone_table_name)
                .execute(&mut **tx)
                .await?;
        }

        Ok(())
    }

    fn tombstone_name(kind: &str) -> String {
        format!("__deleted_{}_{}", kind, uuid::Uuid::new_v4().simple())
    }

    async fn table_exists(
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        table_name: &str,
    ) -> Result<bool> {
        let exists: bool = sqlx::query_scalar(
            r#"
            SELECT EXISTS (
                SELECT 1
                FROM information_schema.tables
                WHERE table_schema = current_schema()
                  AND table_name = $1
                  AND table_type = 'BASE TABLE'
            )
            "#,
        )
        .bind(table_name)
        .fetch_one(&mut **tx)
        .await?;

        Ok(exists)
    }

    async fn verify_table_has_expected_columns(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        table_name: &str,
        columns: &[ColumnDefinition],
    ) -> Result<()> {
        let expected: std::collections::BTreeSet<String> = columns
            .iter()
            .filter(|col| !col.column_type.is_generated())
            .map(|col| col.name.clone())
            .collect();

        if expected.is_empty() {
            return Ok(());
        }

        let rows = sqlx::query(
            r#"
            SELECT column_name
            FROM information_schema.columns
            WHERE table_schema = current_schema()
              AND table_name = $1
              AND is_generated = 'NEVER'
            "#,
        )
        .bind(table_name)
        .fetch_all(&mut **tx)
        .await?;

        let actual: std::collections::BTreeSet<String> = rows
            .iter()
            .map(|row| row.try_get::<String, _>("column_name"))
            .collect::<std::result::Result<_, _>>()?;

        let missing: Vec<String> = expected.difference(&actual).cloned().collect();
        if !missing.is_empty() {
            return Err(ObjectStoreError::database(format!(
                "schema update verification failed for table '{}': missing expected columns: {}",
                table_name,
                missing.join(", ")
            )));
        }

        Ok(())
    }

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

    async fn resolve_condition_subquery_schemas(
        &self,
        condition: Option<&Condition>,
    ) -> Result<HashMap<String, Schema>> {
        let Some(condition) = condition else {
            return Ok(HashMap::new());
        };

        let names = collect_condition_subquery_schema_names(condition)
            .map_err(ObjectStoreError::InvalidCondition)?;
        let mut schemas = HashMap::with_capacity(names.len());
        for name in names {
            let schema = self
                .get_schema(&name)
                .await?
                .ok_or_else(|| ObjectStoreError::schema_not_found(&name))?;
            schemas.insert(name, schema);
        }

        Ok(schemas)
    }

    async fn filter_instances_internal(
        &self,
        schema: &Schema,
        filter: FilterRequest,
    ) -> Result<(Vec<Instance>, i64)> {
        let subquery_schemas = self
            .resolve_condition_subquery_schemas(filter.condition.as_ref())
            .await?;

        let runtara_object_model_core::planning::FilterPlan {
            count_query,
            select_query,
            where_params,
            score_params,
            score_alias,
            effective_limit,
            effective_offset,
        } = runtara_object_model_core::planning::plan_filter(
            &self.config,
            schema,
            filter,
            &subquery_schemas,
        )
        .map_err(ObjectStoreError::InvalidCondition)?;

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
            .bind(effective_limit)
            .bind(effective_offset)
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
        // pg_trgm `similarity()` / `ts_rank()` return `real` (`f32`), while
        // pgvector distance operators return `double precision` (`f64`).
        let computed = score_alias.and_then(|alias| {
            row.try_get::<Option<f32>, _>(alias)
                .ok()
                .flatten()
                .and_then(|f| serde_json::Number::from_f64(f as f64))
                .or_else(|| {
                    row.try_get::<Option<f64>, _>(alias)
                        .ok()
                        .flatten()
                        .and_then(serde_json::Number::from_f64)
                })
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
