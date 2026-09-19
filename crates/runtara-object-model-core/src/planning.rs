//! Pure query planning shared by the native store and the WASM agent.
use crate::config::{DEFAULT_FILTER_RESULT_ROW_LIMIT, StoreConfig};
use crate::instance::{FilterRequest, OrderByEntry, OrderByTarget};
use crate::schema::Schema;
use crate::sql::condition::{
    build_condition_clause_with_subqueries, build_order_by_clause, field_to_sql,
};
use crate::sql::expr::{ExprNode, render_row_expression, validate_row_expression};
use crate::sql::sanitize::quote_identifier;
use std::collections::HashMap;

pub struct FilterPlan {
    pub count_query: String,
    pub select_query: String,
    pub where_params: Vec<serde_json::Value>,
    pub score_params: Vec<serde_json::Value>,
    pub score_alias: Option<String>,
    pub effective_limit: i64,
    pub effective_offset: i64,
}

pub fn plan_filter(
    config: &StoreConfig,
    schema: &Schema,
    filter: FilterRequest,
    subquery_schemas: &HashMap<String, Schema>,
) -> Result<FilterPlan, String> {
    // Build column list
    let mut select_columns = Vec::new();

    if config.auto_columns.id {
        select_columns.push("id".to_string());
    }
    if config.auto_columns.created_at {
        select_columns.push("created_at".to_string());
    }
    if config.auto_columns.updated_at {
        select_columns.push("updated_at".to_string());
    }

    // Optional projection: select only the requested non-generated columns
    // (the auto id/created_at/updated_at columns above are always kept).
    // We intersect against `schema.columns` and quote each name, so an
    // unknown projected name is simply ignored — never interpolated — and
    // this stays injection-safe. `None` keeps the original "all columns"
    // behaviour. This is what lets a report page skip pulling large unused
    // columns (e.g. base64 uploads) it never displays.
    let projected: Option<std::collections::HashSet<&str>> = filter
        .projection
        .as_ref()
        .map(|cols| cols.iter().map(String::as_str).collect());
    for col in &schema.columns {
        if col.column_type.is_generated() {
            continue;
        }
        if let Some(set) = &projected
            && !set.contains(col.name.as_str())
        {
            continue;
        }
        select_columns.push(quote_identifier(&col.name));
    }

    // Build WHERE clause from condition (params: $1..$N1)
    let (where_clause, where_params) = if let Some(condition) = filter.condition {
        let mut param_offset = 1;
        build_condition_clause_with_subqueries(
            &condition,
            &mut param_offset,
            schema,
            subquery_schemas,
        )?
    } else {
        ("TRUE".to_string(), Vec::new())
    };

    // Validate + render `score_expression` if provided. Score-expression
    // params append after WHERE params, so placeholders continue at
    // $(where_params.len() + 1).
    let mut score_params: Vec<serde_json::Value> = Vec::new();
    let mut score_alias: Option<String> = None;
    if let Some(score_expr) = filter.score_expression.as_ref() {
        validate_score_alias(&score_expr.alias)?;

        let node: ExprNode = serde_json::from_value(score_expr.expression.clone())
            .map_err(|e| format!("score_expression: invalid expression JSON: {}", e))?;
        validate_row_expression(&node, schema, 0)?;

        let mut score_offset = (where_params.len() as i32) + 1;
        let score_sql =
            render_row_expression(&node, schema, &mut score_params, &mut score_offset, 0)?;

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
        render_order_by_entries(entries, schema, score_alias.as_deref())?
    } else {
        build_order_by_clause(&filter.sort_by, &filter.sort_order, schema)?
    };

    // Clamp the caller-supplied LIMIT to a server cap so a large (or
    // `i64::MAX`) `limit` can't force a full-table materialization. A
    // negative limit is treated as 0. Mirrors the silent clamp applied by
    // `aggregate_instances`.
    let effective_limit = filter
        .limit
        .clamp(0, DEFAULT_FILTER_RESULT_ROW_LIMIT as i64);
    let effective_offset = filter.offset.max(0);

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

    Ok(FilterPlan {
        count_query,
        select_query,
        where_params,
        score_params,
        score_alias,
        effective_limit,
        effective_offset,
    })
}

pub fn validate_instance_for_insert(
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

/// SQL and bindings for a single object insert. IDs are supplied by the caller
/// (native UUID generation or agent-side UUID generation), never by this planner.
pub fn plan_insert(
    config: &StoreConfig,
    schema: &Schema,
    properties: &serde_json::Value,
    id: &str,
) -> Result<runtara_database_contract::Statement, String> {
    use runtara_database_contract::{SqlValue, Statement};
    let properties = validate_instance_for_insert(schema, properties)?;
    let mut columns = Vec::new();
    let mut params = Vec::new();
    if config.auto_columns.id {
        columns.push("id".into());
        params.push(SqlValue::Text(id.into()));
    }
    for column in &schema.columns {
        if column.column_type.is_generated() {
            continue;
        }
        if let Some(value) = properties.get(&column.name) {
            columns.push(quote_identifier(&column.name));
            params.push(crate::mapping::object_param(&column.column_type, value)?);
        }
    }
    let sql = if columns.is_empty() {
        format!(
            "INSERT INTO {} DEFAULT VALUES",
            quote_identifier(&schema.table_name)
        )
    } else {
        format!(
            "INSERT INTO {} ({}) VALUES ({})",
            quote_identifier(&schema.table_name),
            columns.join(", "),
            (1..=params.len())
                .map(|i| format!("${i}"))
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    Ok(Statement {
        sql,
        params,
        returning: None,
    })
}

/// Absent update properties are not assignments. Empty updates remain a no-op,
/// including when the table has an automatically maintained updated_at column.
pub fn plan_update(
    config: &StoreConfig,
    schema: &Schema,
    properties: &serde_json::Value,
    id: &str,
) -> Result<Option<runtara_database_contract::Statement>, String> {
    use runtara_database_contract::{SqlValue, Statement};
    let properties = properties
        .as_object()
        .ok_or("Properties must be a JSON object")?;
    let mut assignments = Vec::new();
    let mut params = vec![SqlValue::Text(id.into())];
    for column in &schema.columns {
        if column.column_type.is_generated() {
            if properties
                .get(&column.name)
                .is_some_and(|value| !value.is_null())
            {
                return Err(format!(
                    "Column '{}' is generated and cannot be set",
                    column.name
                ));
            }
            continue;
        }
        if let Some(value) = properties.get(&column.name) {
            params.push(crate::mapping::object_param(&column.column_type, value)?);
            assignments.push(format!(
                "{} = ${}",
                quote_identifier(&column.name),
                params.len()
            ));
        }
    }
    if assignments.is_empty() {
        return Ok(None);
    }
    if config.auto_columns.updated_at {
        assignments.insert(0, "updated_at = NOW()".into());
    }
    Ok(Some(Statement {
        sql: format!(
            "UPDATE {} SET {} WHERE id = $1 AND deleted = FALSE",
            quote_identifier(&schema.table_name),
            assignments.join(", ")
        ),
        params,
        returning: None,
    }))
}

pub fn plan_delete(
    config: &StoreConfig,
    schema: &Schema,
    id: &str,
) -> runtara_database_contract::Statement {
    let table = quote_identifier(&schema.table_name);
    let sql = if config.soft_delete {
        let set = if config.auto_columns.updated_at {
            "deleted = TRUE, updated_at = NOW()"
        } else {
            "deleted = TRUE"
        };
        format!("UPDATE {table} SET {set} WHERE id = $1 AND deleted = FALSE")
    } else {
        format!("DELETE FROM {table} WHERE id = $1")
    };
    runtara_database_contract::Statement {
        sql,
        params: vec![runtara_database_contract::SqlValue::Text(id.into())],
        returning: None,
    }
}

pub fn plan_update_where(
    config: &StoreConfig,
    schema: &Schema,
    properties: &serde_json::Value,
    condition: &crate::Condition,
    subqueries: &HashMap<String, Schema>,
) -> Result<Option<runtara_database_contract::Statement>, String> {
    let properties = properties
        .as_object()
        .ok_or("Properties must be a JSON object")?;
    let mut assignments = Vec::new();
    let mut params = Vec::new();
    for column in &schema.columns {
        if column.column_type.is_generated() {
            continue;
        }
        if let Some(value) = properties.get(&column.name) {
            params.push(crate::mapping::object_param(&column.column_type, value)?);
            assignments.push(format!(
                "{} = ${}",
                quote_identifier(&column.name),
                params.len()
            ));
        }
    }
    if assignments.is_empty() {
        return Ok(None);
    }
    if config.auto_columns.updated_at {
        assignments.insert(0, "updated_at = NOW()".into());
    }
    let mut offset = params.len() as i32 + 1;
    let (condition, bindings) =
        build_condition_clause_with_subqueries(condition, &mut offset, schema, subqueries)?;
    params.extend(condition_bindings(bindings));
    Ok(Some(runtara_database_contract::Statement {
        sql: format!(
            "UPDATE {} SET {} WHERE deleted = FALSE AND ({condition})",
            quote_identifier(&schema.table_name),
            assignments.join(", ")
        ),
        params,
        returning: None,
    }))
}

pub fn plan_delete_where(
    config: &StoreConfig,
    schema: &Schema,
    condition: &crate::Condition,
    subqueries: &HashMap<String, Schema>,
) -> Result<runtara_database_contract::Statement, String> {
    let (condition, bindings) =
        build_condition_clause_with_subqueries(condition, &mut 1, schema, subqueries)?;
    let table = quote_identifier(&schema.table_name);
    let sql = if config.soft_delete {
        let set = if config.auto_columns.updated_at {
            "deleted = TRUE, updated_at = NOW()"
        } else {
            "deleted = TRUE"
        };
        format!("UPDATE {table} SET {set} WHERE deleted = FALSE AND ({condition})")
    } else {
        format!("DELETE FROM {table} WHERE ({condition})")
    };
    Ok(runtara_database_contract::Statement {
        sql,
        params: condition_bindings(bindings),
        returning: None,
    })
}

pub fn condition_bindings(
    values: Vec<serde_json::Value>,
) -> Vec<runtara_database_contract::SqlValue> {
    values
        .into_iter()
        .map(|value| {
            runtara_database_contract::SqlValue::Text(match value {
                serde_json::Value::String(s) => s,
                other => other.to_string(),
            })
        })
        .collect()
}

pub fn plan_update_by_ids(
    config: &StoreConfig,
    schema: &Schema,
    updates: Vec<(String, serde_json::Value)>,
) -> Result<Vec<runtara_database_contract::Statement>, String> {
    if updates.len() > config.bulk_request_limit {
        return Err(format!(
            "Bulk request size exceeds limit of {}",
            config.bulk_request_limit
        ));
    }
    let mut statements = Vec::new();
    for (index, (id, properties)) in updates.into_iter().enumerate() {
        let mut properties = properties
            .as_object()
            .cloned()
            .ok_or_else(|| format!("Update at index {index}: properties must be a JSON object"))?;
        // Bulk-by-ID has historically ignored generated columns, whereas a
        // single-row update rejects a caller trying to set one.
        for column in &schema.columns {
            if column.column_type.is_generated() {
                properties.remove(&column.name);
            }
        }
        if let Some(statement) =
            plan_update(config, schema, &serde_json::Value::Object(properties), &id)
                .map_err(|error| format!("Update at index {index}: {error}"))?
        {
            statements.push(statement);
        }
    }
    Ok(statements)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{ColumnDefinition, ColumnType};
    use runtara_database_contract::{SqlType, SqlValue};

    fn schema() -> Schema {
        Schema {
            id: "schema".into(),
            created_at: String::new(),
            updated_at: String::new(),
            name: "example".into(),
            description: None,
            table_name: "quoted\"table".into(),
            columns: vec![
                ColumnDefinition::new("name", ColumnType::String).default("'default'"),
                ColumnDefinition::new("data", ColumnType::Json),
            ],
            indexes: None,
        }
    }

    #[test]
    fn omitted_defaults_and_two_nulls_remain_distinct() {
        let config = StoreConfig::builder("").build();
        let omitted = plan_insert(&config, &schema(), &serde_json::json!({}), "new-id").unwrap();
        assert_eq!(omitted.params, vec![SqlValue::Text("new-id".into())]);
        assert!(!omitted.sql.contains("name"));
        let nulls = plan_insert(
            &config,
            &schema(),
            &serde_json::json!({"name":null,"data":null}),
            "new-id",
        )
        .unwrap();
        assert_eq!(nulls.params[1], SqlValue::Null(SqlType::Text));
        assert_eq!(nulls.params[2], SqlValue::Json(serde_json::Value::Null));
        assert!(nulls.sql.contains("\"quoted\"\"table\""));
    }

    #[test]
    fn empty_update_is_noop_and_missing_fields_are_not_erased() {
        let config = StoreConfig::builder("").build();
        assert!(
            plan_update(&config, &schema(), &serde_json::json!({}), "id")
                .unwrap()
                .is_none()
        );
        let update = plan_update(
            &config,
            &schema(),
            &serde_json::json!({"data":{"v":1}}),
            "id",
        )
        .unwrap()
        .unwrap();
        assert!(!update.sql.contains("\"name\""));
        assert_eq!(update.params[1], SqlValue::Json(serde_json::json!({"v":1})));
    }
}
