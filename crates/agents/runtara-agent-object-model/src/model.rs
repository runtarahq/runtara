//! Object Model orchestration runs in the agent over three SQL host operations.
use crate::sql_client::SqlClient;
use runtara_database_contract::*;
use runtara_object_model_core::mapping::{legacy_json, row_objects};
use runtara_object_model_core::planning::{plan_delete, plan_filter, plan_insert, plan_update};
use runtara_object_model_core::sql::condition::collect_condition_subquery_schema_names;
use runtara_object_model_core::sql::quote_identifier;
use runtara_object_model_core::{
    Condition, CreateSchemaRequest, FilterRequest, Schema, StoreConfig,
};
use serde_json::{Map, Value, json};
use std::collections::HashMap;

pub struct ObjectModel<'a, C> {
    pub client: &'a C,
    pub connection: &'a str,
    pub config: StoreConfig,
}

fn invalid(error: impl std::fmt::Display) -> DatabaseError {
    DatabaseError::invalid(error.to_string())
}
fn text(value: impl Into<String>) -> SqlValue {
    SqlValue::Text(value.into())
}
fn statement(sql: impl Into<String>, params: Vec<SqlValue>) -> Statement {
    Statement {
        sql: sql.into(),
        params,
        returning: None,
    }
}
pub fn string_params(values: &[Value]) -> Vec<SqlValue> {
    values
        .iter()
        .map(|value| {
            text(
                value
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| value.to_string()),
            )
        })
        .collect()
}

impl<'a, C: SqlClient> ObjectModel<'a, C> {
    pub async fn new(client: &'a C, connection: &'a str) -> Result<Self, DatabaseError> {
        let layout = client.layout(connection).await?;
        Ok(Self {
            client,
            connection,
            config: layout.store_config().map_err(invalid)?,
        })
    }

    pub async fn rows(
        &self,
        sql: String,
        params: Vec<SqlValue>,
    ) -> Result<Vec<Map<String, Value>>, DatabaseError> {
        let rows = self
            .client
            .query(
                self.connection,
                QueryRequest {
                    sql,
                    params,
                    result_schema: ResultSpec::Raw,
                },
            )
            .await?;
        row_objects(rows).map_err(invalid)
    }

    pub async fn schema(&self, name: &str) -> Result<Option<Schema>, DatabaseError> {
        let sql = format!(
            "SELECT id, created_at AS \"createdAt\", updated_at AS \"updatedAt\", name, description, table_name AS \"tableName\", columns, indexes FROM {} WHERE name=$1 AND deleted=FALSE",
            quote_identifier(&self.config.metadata_table)
        );
        let rows = match self.rows(sql, vec![text(name)]).await {
            Ok(rows) => rows,
            Err(error) if error.sqlstate.as_deref() == Some("42P01") => return Ok(None),
            Err(error) => return Err(error),
        };
        rows.into_iter()
            .next()
            .map(|row| serde_json::from_value(Value::Object(row)).map_err(invalid))
            .transpose()
    }

    pub async fn require_schema(&self, name: &str) -> Result<Schema, DatabaseError> {
        self.schema(name)
            .await?
            .ok_or_else(|| invalid(format!("Schema not found: {name}")))
    }

    pub async fn subquery_schemas(
        &self,
        condition: Option<&Condition>,
    ) -> Result<HashMap<String, Schema>, DatabaseError> {
        let mut schemas = HashMap::new();
        if let Some(condition) = condition {
            for name in collect_condition_subquery_schema_names(condition).map_err(invalid)? {
                schemas.insert(name.clone(), self.require_schema(&name).await?);
            }
        }
        Ok(schemas)
    }

    pub async fn create(
        &self,
        schema_name: &str,
        properties: Value,
    ) -> Result<Value, DatabaseError> {
        let schema = self.require_schema(schema_name).await?;
        let id = uuid::Uuid::new_v4().to_string();
        let plan = plan_insert(&self.config, &schema, &properties, &id).map_err(invalid)?;
        self.client.execute(self.connection, plan).await?;
        Ok(json!({"success":true,"instance_id":id}))
    }

    pub async fn query(
        &self,
        schema_name: &str,
        filter: FilterRequest,
    ) -> Result<Value, DatabaseError> {
        let schema = self.require_schema(schema_name).await?;
        let subqueries = self.subquery_schemas(filter.condition.as_ref()).await?;
        let plan = plan_filter(&self.config, &schema, filter, &subqueries).map_err(invalid)?;
        let count = self
            .rows(plan.count_query, string_params(&plan.where_params))
            .await?;
        let total_count = count
            .first()
            .and_then(|row| row.values().next())
            .and_then(Value::as_i64)
            .unwrap_or(0);
        let mut params = string_params(&plan.where_params);
        params.extend(string_params(&plan.score_params));
        params.push(SqlValue::Integer(plan.effective_limit.to_string()));
        params.push(SqlValue::Integer(plan.effective_offset.to_string()));
        let rows = self
            .client
            .query(
                self.connection,
                QueryRequest {
                    sql: plan.select_query,
                    params,
                    result_schema: ResultSpec::Raw,
                },
            )
            .await?;
        let rows = runtara_object_model_core::mapping::instance_objects(rows).map_err(invalid)?;
        let instances: Vec<_> = rows
            .into_iter()
            .map(|mut row| {
                for (sql, logical) in [("created_at", "createdAt"), ("updated_at", "updatedAt")] {
                    let value = row.remove(sql).unwrap_or(Value::String(String::new()));
                    row.insert(logical.into(), value);
                }
                row.entry("id").or_insert(Value::String(String::new()));
                row.insert("schemaName".into(), Value::String(schema.name.clone()));
                if let Some(alias) = &plan.score_alias
                    && let Some(value) = row.remove(alias)
                {
                    row.insert("computed".into(), json!({alias:value}));
                }
                Value::Object(row)
            })
            .collect();
        Ok(json!({"success":true,"instances":instances,"total_count":total_count}))
    }

    pub async fn update(
        &self,
        schema_name: &str,
        id: &str,
        properties: Value,
    ) -> Result<Value, DatabaseError> {
        let schema = self.require_schema(schema_name).await?;
        if let Some(plan) = plan_update(&self.config, &schema, &properties, id).map_err(invalid)? {
            let result = self.client.execute(self.connection, plan).await?;
            if result.rows_affected == 0 {
                return Err(invalid(format!("Instance not found: {id}")));
            }
        }
        Ok(json!({"success":true,"instance_id":id}))
    }

    pub async fn delete(&self, schema_name: &str, id: &str) -> Result<Value, DatabaseError> {
        let schema = self.require_schema(schema_name).await?;
        let result = self
            .client
            .execute(self.connection, plan_delete(&self.config, &schema, id))
            .await?;
        if result.rows_affected == 0 {
            return Err(invalid(format!("Instance not found: {id}")));
        }
        Ok(json!({"success":true}))
    }

    pub async fn aggregate(
        &self,
        schema_name: &str,
        request: runtara_object_model_core::sql::AggregateRequest,
    ) -> Result<Value, DatabaseError> {
        let schema = self.require_schema(schema_name).await?;
        let subqueries = self.subquery_schemas(request.condition.as_ref()).await?;
        let plan = runtara_object_model_core::sql::build_aggregate_query_with_subqueries(
            &schema,
            &request,
            &subqueries,
        )
        .map_err(invalid)?;
        let cap = runtara_object_model_core::config::DEFAULT_AGGREGATE_RESULT_ROW_LIMIT as i64;
        let limit = request.limit.map(|v| v.clamp(0, cap)).unwrap_or(cap + 1);
        let mut params = string_params(&plan.params);
        params.push(SqlValue::Integer(limit.to_string()));
        params.push(SqlValue::Integer(
            request.offset.unwrap_or(0).max(0).to_string(),
        ));
        let result = self
            .client
            .query(
                self.connection,
                QueryRequest {
                    sql: plan.data_sql,
                    params,
                    result_schema: ResultSpec::Raw,
                },
            )
            .await?;
        if request.limit.is_none() && result.rows.len() > cap as usize {
            return Err(invalid("Aggregate result exceeds the row limit"));
        }
        let rows = result
            .rows
            .into_iter()
            .map(|row| {
                row.into_iter()
                    .map(legacy_json)
                    .collect::<Result<Vec<_>, _>>()
            })
            .collect::<Result<Vec<_>, _>>()
            .map_err(invalid)?;
        let group_count = if let Some(sql) = plan.count_sql {
            let count = self.rows(sql, string_params(&plan.params)).await?;
            count
                .first()
                .and_then(|row| row.values().next())
                .and_then(Value::as_i64)
                .unwrap_or(0)
        } else {
            1
        };
        Ok(json!({"success":true,"columns":plan.columns,"rows":rows,"group_count":group_count}))
    }

    pub async fn create_schema(
        &self,
        request: CreateSchemaRequest,
    ) -> Result<Value, DatabaseError> {
        runtara_object_model_core::validation::SchemaValidator::validate_schema(
            &request.table_name,
            &request.columns,
            &request.indexes,
        )
        .map_err(invalid)?;
        if self.schema(&request.name).await?.is_some() {
            return Ok(json!({"success":true,"schema_id":null}));
        }
        let metadata = quote_identifier(&self.config.metadata_table);
        let ddl = runtara_object_model_core::sql::DdlGenerator::new(&self.config);
        let mut statements = vec![
            Statement {
                sql: runtara_object_model_core::sql::DdlGenerator::METADATA_LOCK_SQL.into(),
                params: vec![text(&self.config.metadata_table)],
                returning: Some(ResultSpec::Raw),
            },
            statement(ddl.generate_metadata_table(), vec![]),
        ];
        // Recover tombstoned logical names without reusing their physical tables.
        let tombstones = self.rows(format!("SELECT id, table_name, EXISTS(SELECT 1 FROM information_schema.tables t WHERE t.table_schema=current_schema() AND t.table_name=m.table_name AND t.table_type='BASE TABLE') AS table_exists, (SELECT COALESCE(jsonb_agg(indexname),'[]'::jsonb) FROM pg_indexes WHERE schemaname=current_schema() AND tablename=m.table_name) AS index_names FROM {metadata} m WHERE deleted=TRUE AND (name=$1 OR table_name=$2)"), vec![text(&request.name), text(&request.table_name)]).await;
        let tombstones = match tombstones {
            Ok(rows) => rows,
            Err(error) if error.sqlstate.as_deref() == Some("42P01") => vec![],
            Err(error) => return Err(error),
        };
        for row in tombstones {
            let id = row["id"]
                .as_str()
                .ok_or_else(|| invalid("Invalid schema metadata ID"))?;
            let old_table = row["table_name"]
                .as_str()
                .ok_or_else(|| invalid("Invalid schema table metadata"))?;
            let tombstone_table = format!("deleted_table_{}", uuid::Uuid::new_v4().simple());
            let tombstone_name = format!("deleted_schema_{}", uuid::Uuid::new_v4().simple());
            if row["table_exists"].as_bool() == Some(true) {
                let indexes: Vec<String> =
                    serde_json::from_value(row["index_names"].clone()).map_err(invalid)?;
                for sql in runtara_object_model_core::sql::DdlGenerator::tombstone_table(
                    old_table,
                    &tombstone_table,
                    &indexes,
                    || format!("deleted_index_{}", uuid::Uuid::new_v4().simple()),
                ) {
                    statements.push(statement(sql, vec![]));
                }
            }
            statements.push(statement(
                format!(
                    "UPDATE {metadata} SET name=$2, table_name=$3, updated_at=NOW() WHERE id=$1"
                ),
                vec![text(id), text(tombstone_name), text(tombstone_table)],
            ));
        }
        let id = uuid::Uuid::new_v4().to_string();
        statements.push(statement(format!("INSERT INTO {metadata} (id,name,description,table_name,columns,indexes) VALUES ($1,$2,$3,$4,$5,$6)"), vec![
            text(&id), text(&request.name), request.description.as_ref().map(text).unwrap_or(SqlValue::Null(SqlType::Text)), text(&request.table_name),
            SqlValue::Json(serde_json::to_value(&request.columns).map_err(invalid)?),
            request.indexes.as_ref().map(|indexes| serde_json::to_value(indexes).map(SqlValue::Json).map_err(invalid)).transpose()?.unwrap_or(SqlValue::Null(SqlType::Json)),
        ]));
        let ddl = runtara_object_model_core::sql::DdlGenerator::new(&self.config);
        statements.push(statement(
            ddl.generate_create_table(&request.table_name, &request.columns),
            vec![],
        ));
        statements.push(statement(
            ddl.generate_default_index(&request.table_name),
            vec![],
        ));
        for sql in ddl
            .generate_unique_column_indexes(&request.table_name, &request.columns)
            .into_iter()
            .chain(ddl.generate_trigram_indexes(&request.table_name, &request.columns))
            .chain(ddl.generate_tsvector_indexes(&request.table_name, &request.columns))
            .chain(ddl.generate_vector_indexes(&request.table_name, &request.columns))
        {
            statements.push(statement(sql, vec![]));
        }
        if let Some(indexes) = &request.indexes {
            for index in indexes {
                statements.push(statement(
                    ddl.generate_create_index(&request.table_name, index),
                    vec![],
                ));
            }
        }
        match self
            .client
            .execute_batch(
                self.connection,
                BatchRequest {
                    mode: BatchMode::Atomic,
                    statements,
                },
            )
            .await
        {
            Ok(_) => Ok(json!({"success":true,"schema_id":id})),
            Err(error)
                if error.outcome == Outcome::RolledBack
                    && matches!(
                        error.sqlstate.as_deref(),
                        Some("23505" | "42P07" | "42P01" | "42704")
                    ) =>
            {
                // A concurrent bootstrap may have created this schema.
                if self.schema(&request.name).await?.is_some() {
                    Ok(json!({"success":true,"schema_id":null}))
                } else {
                    Err(error)
                }
            }
            Err(error) => Err(error),
        }
    }
}

#[derive(serde::Deserialize)]
pub struct BulkCreateRequest {
    pub schema_name: String,
    pub instances: Option<Vec<Value>>,
    pub columns: Option<Vec<String>>,
    pub rows: Option<Vec<Vec<Value>>>,
    #[serde(default)]
    pub constants: Map<String, Value>,
    #[serde(default)]
    pub nullify_empty_strings: bool,
    #[serde(default)]
    pub on_conflict: Option<String>,
    #[serde(default)]
    pub on_error: Option<String>,
    #[serde(default)]
    pub conflict_columns: Vec<String>,
}

#[derive(serde::Deserialize)]
#[serde(tag = "mode", rename_all = "camelCase")]
pub enum BulkUpdateMode {
    ByCondition {
        properties: Value,
        condition: Condition,
    },
    ByIds {
        updates: Vec<UpdateById>,
    },
}
#[derive(serde::Deserialize)]
pub struct UpdateById {
    pub id: String,
    pub properties: Value,
}
#[derive(serde::Deserialize)]
pub struct BulkUpdateRequest {
    pub schema_name: String,
    #[serde(flatten)]
    pub mode: BulkUpdateMode,
}
#[derive(serde::Deserialize)]
pub struct BulkDeleteRequest {
    pub schema_name: String,
    pub ids: Option<Vec<String>>,
    pub condition: Option<Condition>,
}

impl<C: SqlClient> ObjectModel<'_, C> {
    async fn atomic(&self, statements: Vec<Statement>) -> Result<i64, DatabaseError> {
        if statements.is_empty() {
            return Ok(0);
        }
        let result = self
            .client
            .execute_batch(
                self.connection,
                BatchRequest {
                    mode: BatchMode::Atomic,
                    statements,
                },
            )
            .await?;
        let mut affected = 0i64;
        for entry in result.results {
            affected += entry.result?.rows_affected as i64;
        }
        Ok(affected)
    }

    pub async fn bulk_create(&self, request: BulkCreateRequest) -> Result<Value, DatabaseError> {
        use runtara_object_model_core::{BulkCreateOptions, ConflictMode, ValidationMode};
        let schema = self.require_schema(&request.schema_name).await?;
        let instances = runtara_object_model_core::bulk::normalize_bulk_create_inputs(
            request.instances.as_deref(),
            request.columns.as_deref(),
            request.rows.as_deref(),
            &request.constants,
            request.nullify_empty_strings,
            &schema,
        )
        .map_err(invalid)?;
        let conflict_mode = match request.on_conflict.as_deref().unwrap_or("error") {
            "error" => ConflictMode::Error,
            "skip" => ConflictMode::Skip {
                conflict_columns: request.conflict_columns,
            },
            "upsert" => ConflictMode::Upsert {
                conflict_columns: request.conflict_columns,
            },
            _ => return Err(invalid("Invalid bulk conflict mode")),
        };
        let validation_mode = match request.on_error.as_deref().unwrap_or("stop") {
            "stop" => ValidationMode::Stop,
            "skip" => ValidationMode::Skip,
            _ => return Err(invalid("Invalid bulk validation mode")),
        };
        let mut plan = runtara_object_model_core::bulk::plan_bulk_create(
            &self.config,
            &schema,
            instances,
            BulkCreateOptions {
                conflict_mode,
                validation_mode,
            },
            || uuid::Uuid::new_v4().to_string(),
        )
        .map_err(invalid)?;
        let affected = self.atomic(std::mem::take(&mut plan.statements)).await?;
        let result = plan.finish(affected);
        Ok(
            json!({"success":true,"created_count":result.created_count,"skipped_count":result.skipped_count,"errors":result.errors}),
        )
    }

    pub async fn bulk_update(&self, request: BulkUpdateRequest) -> Result<Value, DatabaseError> {
        let schema = self.require_schema(&request.schema_name).await?;
        let affected = match request.mode {
            BulkUpdateMode::ByCondition {
                properties,
                condition,
            } => {
                let schemas = self.subquery_schemas(Some(&condition)).await?;
                match runtara_object_model_core::planning::plan_update_where(
                    &self.config,
                    &schema,
                    &properties,
                    &condition,
                    &schemas,
                )
                .map_err(invalid)?
                {
                    Some(statement) => {
                        self.client
                            .execute(self.connection, statement)
                            .await?
                            .rows_affected as i64
                    }
                    None => 0,
                }
            }
            BulkUpdateMode::ByIds { updates } => {
                let plans = runtara_object_model_core::planning::plan_update_by_ids(
                    &self.config,
                    &schema,
                    updates
                        .into_iter()
                        .map(|entry| (entry.id, entry.properties))
                        .collect(),
                )
                .map_err(invalid)?;
                self.atomic(plans).await?
            }
        };
        Ok(json!({"success":true,"updated_count":affected}))
    }

    pub async fn bulk_delete(&self, request: BulkDeleteRequest) -> Result<Value, DatabaseError> {
        let schema = self.require_schema(&request.schema_name).await?;
        let condition = match (request.ids, request.condition) {
            (Some(ids), _) if !ids.is_empty() => {
                Condition::r#in("id", ids.into_iter().map(Value::String).collect())
            }
            (_, Some(condition)) => condition,
            _ => return Err(invalid("Either ids or condition must be provided")),
        };
        let schemas = self.subquery_schemas(Some(&condition)).await?;
        let plan = runtara_object_model_core::planning::plan_delete_where(
            &self.config,
            &schema,
            &condition,
            &schemas,
        )
        .map_err(invalid)?;
        let result = self.client.execute(self.connection, plan).await?;
        Ok(json!({"success":true,"deleted_count":result.rows_affected}))
    }
}
