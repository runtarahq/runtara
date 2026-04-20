//! Aggregate query building for GROUP BY-shaped queries.
//!
//! Converts an [`AggregateRequest`] into a pair of SQL queries:
//! - `data_sql` — the grouped rows, with every output column wrapped in
//!   `to_jsonb(...)` so the caller can decode any cell as a
//!   [`serde_json::Value`] without per-column type gymnastics.
//! - `count_sql` — `SELECT COUNT(*) FROM (<group by subquery>) g`, used to
//!   report the total number of groups matched (`group_count`) before
//!   LIMIT/OFFSET. Omitted when there is no `group_by` (in which case the
//!   answer is always `1`).
//!
//! The WHERE clause reuses [`build_condition_clause`] so the aggregate DSL
//! shares the exact same filter semantics as [`ObjectStore::filter_instances`].
//!
//! `FIRST_VALUE` / `LAST_VALUE` render as `(array_agg(... ORDER BY ...))[1]`
//! rather than window functions so the whole query stays a single GROUP BY
//! and composes cleanly with filtering and outer wrappers.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::instance::Condition;
use crate::schema::Schema;
use crate::sql::condition::{build_condition_clause, field_to_sql, resolve_sql_cast};
use crate::sql::sanitize::quote_identifier;
use crate::types::ColumnType;

// ============================================================================
// Public types
// ============================================================================

/// Supported aggregate functions. JSON encoding is SCREAMING_SNAKE_CASE.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum AggregateFn {
    Count,
    Sum,
    Min,
    Max,
    FirstValue,
    LastValue,
}

/// Sort direction for `order_by` entries. JSON encoding is UPPERCASE.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "UPPERCASE")]
pub enum SortDirection {
    #[default]
    Asc,
    Desc,
}

impl SortDirection {
    fn as_sql(self) -> &'static str {
        match self {
            SortDirection::Asc => "ASC",
            SortDirection::Desc => "DESC",
        }
    }

    fn flipped(self) -> SortDirection {
        match self {
            SortDirection::Asc => SortDirection::Desc,
            SortDirection::Desc => SortDirection::Asc,
        }
    }
}

/// A single `(column, direction)` pair used inside aggregate `order_by` and
/// the top-level `order_by`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AggregateOrderBy {
    pub column: String,
    #[serde(default)]
    pub direction: SortDirection,
}

/// A single aggregate expression in an [`AggregateRequest`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AggregateSpec {
    /// Output column name. Must be `[a-zA-Z_][a-zA-Z0-9_]*` and unique.
    pub alias: String,

    /// The aggregate function.
    #[serde(rename = "fn")]
    pub fn_: AggregateFn,

    /// Source column. Optional for `COUNT` (`COUNT(*)`). Required for every
    /// other function.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub column: Option<String>,

    /// Apply `DISTINCT`. Only valid with `fn = COUNT` and a non-null `column`.
    #[serde(default)]
    pub distinct: bool,

    /// Required for `FIRST_VALUE` / `LAST_VALUE`; ignored for everything else.
    #[serde(default, rename = "orderBy", alias = "order_by")]
    pub order_by: Vec<AggregateOrderBy>,
}

/// Request for [`ObjectStore::aggregate_instances`].
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AggregateRequest {
    /// Filter predicate, same DSL as `filter_instances`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub condition: Option<Condition>,

    /// Columns to group by. Zero entries → one output row over the whole
    /// filtered set.
    #[serde(default, rename = "groupBy", alias = "group_by")]
    pub group_by: Vec<String>,

    /// At least one aggregate required.
    pub aggregates: Vec<AggregateSpec>,

    /// Optional top-level sort. Each `column` must be either a `group_by`
    /// column or an aggregate `alias`.
    #[serde(default, rename = "orderBy", alias = "order_by")]
    pub order_by: Vec<AggregateOrderBy>,

    #[serde(default)]
    pub limit: Option<i64>,

    #[serde(default)]
    pub offset: Option<i64>,
}

/// Tabular result from [`ObjectStore::aggregate_instances`]: output columns
/// (group-by cols first, aggregate aliases second), rows aligned to those
/// columns, and the total number of groups matched by the condition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AggregateResult {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<serde_json::Value>>,
    #[serde(rename = "groupCount")]
    pub group_count: i64,
}

/// Compiled SQL form of an [`AggregateRequest`].
///
/// `data_sql` expects two trailing positional parameters after `params`:
/// `LIMIT` and `OFFSET`. `count_sql` uses only `params`.
#[derive(Debug)]
pub struct AggregateSql {
    /// SELECT query that produces the output rows.
    pub data_sql: String,
    /// COUNT query that produces `group_count`. `None` means the caller
    /// should use `1` without hitting the database (no `group_by`).
    pub count_sql: Option<String>,
    /// Parameter values (condition-derived). Bind in order, then `limit`,
    /// then `offset` for `data_sql`.
    pub params: Vec<serde_json::Value>,
    /// Output column names, in row order.
    pub columns: Vec<String>,
}

// ============================================================================
// Builder
// ============================================================================

/// Compile an [`AggregateRequest`] for a given schema to SQL.
///
/// Performs all static validation (alias shape, column existence, function /
/// type compatibility, order_by references) before emitting SQL.
pub fn build_aggregate_query(
    schema: &Schema,
    req: &AggregateRequest,
) -> Result<AggregateSql, String> {
    if req.aggregates.is_empty() {
        return Err("aggregates must contain at least one entry".to_string());
    }

    // Validate aggregate specs up front.
    let mut seen_aliases: HashSet<&str> = HashSet::new();
    for spec in &req.aggregates {
        validate_alias(&spec.alias)?;
        if !seen_aliases.insert(spec.alias.as_str()) {
            return Err(format!("Duplicate aggregate alias: {}", spec.alias));
        }
        validate_spec(spec, schema)?;
    }

    // Validate group_by columns and normalize them to SQL column names.
    let mut group_sql_cols: Vec<String> = Vec::with_capacity(req.group_by.len());
    for raw in &req.group_by {
        validate_field_chars(raw, "group_by")?;
        let sql_field = field_to_sql(raw);
        if !column_exists(sql_field, schema) {
            return Err(format!("Unknown group_by column: '{}'", raw));
        }
        group_sql_cols.push(sql_field.to_string());
    }

    // Validate top-level order_by — each column must resolve to either a
    // group_by column or an aggregate alias.
    let group_sql_set: HashSet<&str> = group_sql_cols.iter().map(|s| s.as_str()).collect();
    let alias_set: HashSet<&str> = req.aggregates.iter().map(|a| a.alias.as_str()).collect();
    for ob in &req.order_by {
        validate_field_chars(&ob.column, "order_by")?;
        let sql_field = field_to_sql(&ob.column);
        let ok = group_sql_set.contains(sql_field) || alias_set.contains(ob.column.as_str());
        if !ok {
            return Err(format!(
                "order_by column '{}' is not in group_by or aggregates",
                ob.column
            ));
        }
    }

    // ---- WHERE clause (condition) -----------------------------------------
    let (where_clause, params) = if let Some(condition) = &req.condition {
        let mut param_offset = 1i32;
        build_condition_clause(condition, &mut param_offset, schema)?
    } else {
        ("TRUE".to_string(), Vec::new())
    };
    let base_where = format!("deleted = FALSE AND ({})", where_clause);
    let table = quote_identifier(&schema.table_name);

    // ---- Inner SELECT: group_by cols + aggregate exprs --------------------
    let mut inner_select_parts: Vec<String> = Vec::new();
    let mut output_columns: Vec<String> = Vec::new();

    for sql_col in &group_sql_cols {
        inner_select_parts.push(quote_identifier(sql_col));
        output_columns.push(sql_col.clone());
    }

    for spec in &req.aggregates {
        let expr = render_aggregate_expr(spec, schema)?;
        inner_select_parts.push(format!("{} AS {}", expr, quote_identifier(&spec.alias)));
        output_columns.push(spec.alias.clone());
    }

    // ---- Inner GROUP BY ---------------------------------------------------
    let group_by_clause = if group_sql_cols.is_empty() {
        String::new()
    } else {
        let cols: Vec<String> = group_sql_cols.iter().map(|c| quote_identifier(c)).collect();
        format!(" GROUP BY {}", cols.join(", "))
    };

    // ---- Inner ORDER BY (targets group cols or aliases) -------------------
    let inner_order_by = if req.order_by.is_empty() {
        String::new()
    } else {
        let parts: Vec<String> = req
            .order_by
            .iter()
            .map(|ob| {
                let sql_field = field_to_sql(&ob.column);
                let ident = if group_sql_set.contains(sql_field) {
                    quote_identifier(sql_field)
                } else {
                    quote_identifier(&ob.column)
                };
                format!("{} {}", ident, ob.direction.as_sql())
            })
            .collect();
        format!(" ORDER BY {}", parts.join(", "))
    };

    // ---- Inner LIMIT / OFFSET ---------------------------------------------
    let limit_param_idx = params.len() + 1;
    let offset_param_idx = params.len() + 2;
    let limit_offset = format!(" LIMIT ${} OFFSET ${}", limit_param_idx, offset_param_idx);

    let inner_sql = format!(
        "SELECT {} FROM {} WHERE {}{}{}{}",
        inner_select_parts.join(", "),
        table,
        base_where,
        group_by_clause,
        inner_order_by,
        limit_offset,
    );

    // ---- Outer wrapper: to_jsonb every column -----------------------------
    let outer_select_parts: Vec<String> = output_columns
        .iter()
        .map(|name| {
            format!(
                "to_jsonb({}) AS {}",
                quote_identifier(name),
                quote_identifier(name)
            )
        })
        .collect();
    let data_sql = format!(
        "SELECT {} FROM ({}) g",
        outer_select_parts.join(", "),
        inner_sql
    );

    // ---- Count query ------------------------------------------------------
    let count_sql = if group_sql_cols.is_empty() {
        None
    } else {
        let cols: Vec<String> = group_sql_cols.iter().map(|c| quote_identifier(c)).collect();
        Some(format!(
            "SELECT COUNT(*)::bigint FROM (SELECT 1 FROM {} WHERE {} GROUP BY {}) g",
            table,
            base_where,
            cols.join(", "),
        ))
    };

    Ok(AggregateSql {
        data_sql,
        count_sql,
        params,
        columns: output_columns,
    })
}

// ============================================================================
// Validation helpers
// ============================================================================

fn validate_alias(alias: &str) -> Result<(), String> {
    if alias.is_empty() {
        return Err("Aggregate alias cannot be empty".to_string());
    }
    let mut chars = alias.chars();
    let first = chars.next().unwrap();
    if !(first.is_ascii_alphabetic() || first == '_') {
        return Err(format!(
            "Aggregate alias '{}' must start with a letter or underscore",
            alias
        ));
    }
    if !chars.all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return Err(format!(
            "Aggregate alias '{}' may only contain letters, digits, and underscores",
            alias
        ));
    }
    Ok(())
}

fn validate_field_chars(field: &str, context: &str) -> Result<(), String> {
    if field.is_empty() {
        return Err(format!("{} field name cannot be empty", context));
    }
    if !field
        .chars()
        .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
    {
        return Err(format!(
            "{} field '{}' contains invalid characters",
            context, field
        ));
    }
    Ok(())
}

fn column_exists(sql_field: &str, schema: &Schema) -> bool {
    matches!(sql_field, "id" | "created_at" | "updated_at")
        || schema.columns.iter().any(|c| c.name == sql_field)
}

fn column_type<'a>(sql_field: &str, schema: &'a Schema) -> Option<&'a ColumnType> {
    schema
        .columns
        .iter()
        .find(|c| c.name == sql_field)
        .map(|c| &c.column_type)
}

fn validate_spec(spec: &AggregateSpec, schema: &Schema) -> Result<(), String> {
    match spec.fn_ {
        AggregateFn::Count => {
            if spec.distinct && spec.column.is_none() {
                return Err(format!(
                    "Aggregate '{}': COUNT with distinct=true requires a column",
                    spec.alias
                ));
            }
            if let Some(col) = &spec.column {
                validate_field_chars(col, "aggregate column")?;
                let sql = field_to_sql(col);
                if !column_exists(sql, schema) {
                    return Err(format!(
                        "Aggregate '{}': unknown column '{}'",
                        spec.alias, col
                    ));
                }
            }
            if !spec.order_by.is_empty() {
                return Err(format!(
                    "Aggregate '{}': order_by is only valid for FIRST_VALUE/LAST_VALUE",
                    spec.alias
                ));
            }
        }
        AggregateFn::Sum => {
            let col = spec
                .column
                .as_ref()
                .ok_or_else(|| format!("Aggregate '{}': SUM requires a column", spec.alias))?;
            if spec.distinct {
                return Err(format!(
                    "Aggregate '{}': distinct is only valid for COUNT",
                    spec.alias
                ));
            }
            validate_field_chars(col, "aggregate column")?;
            let sql = field_to_sql(col);
            match column_type(sql, schema) {
                Some(ColumnType::Integer) | Some(ColumnType::Decimal { .. }) => {}
                Some(_) => {
                    return Err(format!(
                        "Aggregate '{}': SUM requires a numeric column, '{}' is not numeric",
                        spec.alias, col
                    ));
                }
                None => {
                    return Err(format!(
                        "Aggregate '{}': unknown column '{}'",
                        spec.alias, col
                    ));
                }
            }
            if !spec.order_by.is_empty() {
                return Err(format!(
                    "Aggregate '{}': order_by is only valid for FIRST_VALUE/LAST_VALUE",
                    spec.alias
                ));
            }
        }
        AggregateFn::Min | AggregateFn::Max => {
            let col = spec.column.as_ref().ok_or_else(|| {
                format!(
                    "Aggregate '{}': {:?} requires a column",
                    spec.alias, spec.fn_
                )
            })?;
            if spec.distinct {
                return Err(format!(
                    "Aggregate '{}': distinct is only valid for COUNT",
                    spec.alias
                ));
            }
            validate_field_chars(col, "aggregate column")?;
            let sql = field_to_sql(col);
            if !column_exists(sql, schema) {
                return Err(format!(
                    "Aggregate '{}': unknown column '{}'",
                    spec.alias, col
                ));
            }
            if matches!(column_type(sql, schema), Some(ColumnType::Json)) {
                return Err(format!(
                    "Aggregate '{}': MIN/MAX cannot be applied to a JSON column",
                    spec.alias
                ));
            }
            if !spec.order_by.is_empty() {
                return Err(format!(
                    "Aggregate '{}': order_by is only valid for FIRST_VALUE/LAST_VALUE",
                    spec.alias
                ));
            }
        }
        AggregateFn::FirstValue | AggregateFn::LastValue => {
            let col = spec.column.as_ref().ok_or_else(|| {
                format!(
                    "Aggregate '{}': FIRST_VALUE/LAST_VALUE requires a column",
                    spec.alias
                )
            })?;
            if spec.distinct {
                return Err(format!(
                    "Aggregate '{}': distinct is only valid for COUNT",
                    spec.alias
                ));
            }
            validate_field_chars(col, "aggregate column")?;
            let sql = field_to_sql(col);
            if !column_exists(sql, schema) {
                return Err(format!(
                    "Aggregate '{}': unknown column '{}'",
                    spec.alias, col
                ));
            }
            if spec.order_by.is_empty() {
                return Err(format!(
                    "Aggregate '{}': FIRST_VALUE/LAST_VALUE requires non-empty order_by",
                    spec.alias
                ));
            }
            for ob in &spec.order_by {
                validate_field_chars(&ob.column, "aggregate order_by")?;
                let osql = field_to_sql(&ob.column);
                if !column_exists(osql, schema) {
                    return Err(format!(
                        "Aggregate '{}': order_by column '{}' not found in schema",
                        spec.alias, ob.column
                    ));
                }
            }
        }
    }
    Ok(())
}

// ============================================================================
// SQL rendering
// ============================================================================

fn render_aggregate_expr(spec: &AggregateSpec, schema: &Schema) -> Result<String, String> {
    match spec.fn_ {
        AggregateFn::Count => Ok(match &spec.column {
            None => "COUNT(*)::bigint".to_string(),
            Some(col) => {
                let sql = field_to_sql(col);
                let cast = resolve_sql_cast(sql, schema);
                if spec.distinct {
                    format!(
                        "COUNT(DISTINCT {}::{})::bigint",
                        quote_identifier(sql),
                        cast
                    )
                } else {
                    format!("COUNT({}::{})::bigint", quote_identifier(sql), cast)
                }
            }
        }),
        AggregateFn::Sum => {
            let col = spec.column.as_ref().unwrap();
            let sql = field_to_sql(col);
            // Sum always renders as numeric — covers both Integer and Decimal
            // source columns without loss.
            Ok(format!("SUM({}::numeric)", quote_identifier(sql)))
        }
        AggregateFn::Min | AggregateFn::Max => {
            let col = spec.column.as_ref().unwrap();
            let sql = field_to_sql(col);
            let cast = resolve_sql_cast(sql, schema);
            let fn_name = if matches!(spec.fn_, AggregateFn::Min) {
                "MIN"
            } else {
                "MAX"
            };
            Ok(format!("{}({}::{})", fn_name, quote_identifier(sql), cast))
        }
        AggregateFn::FirstValue | AggregateFn::LastValue => {
            let col = spec.column.as_ref().unwrap();
            let sql = field_to_sql(col);
            let val_cast = resolve_sql_cast(sql, schema);
            let flip = matches!(spec.fn_, AggregateFn::LastValue);

            let order_parts: Vec<String> = spec
                .order_by
                .iter()
                .map(|ob| {
                    let osql = field_to_sql(&ob.column);
                    let ocast = resolve_sql_cast(osql, schema);
                    let dir = if flip {
                        ob.direction.flipped()
                    } else {
                        ob.direction
                    };
                    format!("{}::{} {}", quote_identifier(osql), ocast, dir.as_sql())
                })
                .collect();

            Ok(format!(
                "(array_agg({}::{} ORDER BY {}))[1]",
                quote_identifier(sql),
                val_cast,
                order_parts.join(", "),
            ))
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{ColumnDefinition, ColumnType};

    fn stock_snapshot_schema() -> Schema {
        Schema {
            id: "s1".into(),
            created_at: "now".into(),
            updated_at: "now".into(),
            name: "StockSnapshot".into(),
            description: None,
            table_name: "stock_snapshot".into(),
            columns: vec![
                ColumnDefinition {
                    name: "sku".into(),
                    column_type: ColumnType::String,
                    nullable: true,
                    unique: false,
                    default_value: None,
                },
                ColumnDefinition {
                    name: "qty".into(),
                    column_type: ColumnType::Integer,
                    nullable: true,
                    unique: false,
                    default_value: None,
                },
                ColumnDefinition {
                    name: "price".into(),
                    column_type: ColumnType::Decimal {
                        precision: 10,
                        scale: 2,
                    },
                    nullable: true,
                    unique: false,
                    default_value: None,
                },
                ColumnDefinition {
                    name: "snapshot_date".into(),
                    column_type: ColumnType::Timestamp,
                    nullable: true,
                    unique: false,
                    default_value: None,
                },
                ColumnDefinition {
                    name: "notes".into(),
                    column_type: ColumnType::Json,
                    nullable: true,
                    unique: false,
                    default_value: None,
                },
            ],
            indexes: None,
        }
    }

    fn count_all() -> AggregateSpec {
        AggregateSpec {
            alias: "n".into(),
            fn_: AggregateFn::Count,
            column: None,
            distinct: false,
            order_by: vec![],
        }
    }

    #[test]
    fn count_star_no_group_by() {
        let schema = stock_snapshot_schema();
        let req = AggregateRequest {
            aggregates: vec![count_all()],
            ..Default::default()
        };
        let sql = build_aggregate_query(&schema, &req).unwrap();
        assert_eq!(sql.columns, vec!["n".to_string()]);
        assert!(sql.count_sql.is_none(), "no group_by → no count query");
        assert!(sql.data_sql.contains("COUNT(*)::bigint"));
        assert!(sql.data_sql.contains("to_jsonb(\"n\")"));
        // No GROUP BY since group_by is empty.
        assert!(!sql.data_sql.contains("GROUP BY"));
        assert!(sql.params.is_empty());
    }

    #[test]
    fn count_distinct_requires_column() {
        let schema = stock_snapshot_schema();
        let req = AggregateRequest {
            aggregates: vec![AggregateSpec {
                alias: "distinct_skus".into(),
                fn_: AggregateFn::Count,
                column: None,
                distinct: true,
                order_by: vec![],
            }],
            ..Default::default()
        };
        let err = build_aggregate_query(&schema, &req).unwrap_err();
        assert!(
            err.contains("COUNT with distinct=true requires a column"),
            "{}",
            err
        );
    }

    #[test]
    fn sum_grouped_with_condition_and_order_by() {
        let schema = stock_snapshot_schema();
        let req = AggregateRequest {
            condition: Some(Condition {
                op: "EQ".into(),
                arguments: Some(vec![
                    serde_json::json!("sku"),
                    serde_json::json!("WIDGET-1"),
                ]),
            }),
            group_by: vec!["sku".into()],
            aggregates: vec![AggregateSpec {
                alias: "total".into(),
                fn_: AggregateFn::Sum,
                column: Some("qty".into()),
                distinct: false,
                order_by: vec![],
            }],
            order_by: vec![AggregateOrderBy {
                column: "total".into(),
                direction: SortDirection::Desc,
            }],
            limit: Some(10),
            offset: Some(0),
        };
        let sql = build_aggregate_query(&schema, &req).unwrap();
        assert_eq!(sql.columns, vec!["sku".to_string(), "total".to_string()]);
        assert!(sql.count_sql.is_some(), "group_by → count query");
        assert!(sql.data_sql.contains("SUM(\"qty\"::numeric)"));
        assert!(sql.data_sql.contains("GROUP BY \"sku\""));
        assert!(sql.data_sql.contains("ORDER BY \"total\" DESC"));
        // Condition bound one param; LIMIT/OFFSET get $2/$3.
        assert_eq!(sql.params.len(), 1);
        assert!(sql.data_sql.contains("LIMIT $2 OFFSET $3"));
    }

    #[test]
    fn first_and_last_value_render_with_array_agg() {
        let schema = stock_snapshot_schema();
        let req = AggregateRequest {
            group_by: vec!["sku".into()],
            aggregates: vec![
                AggregateSpec {
                    alias: "first_qty".into(),
                    fn_: AggregateFn::FirstValue,
                    column: Some("qty".into()),
                    distinct: false,
                    order_by: vec![AggregateOrderBy {
                        column: "snapshot_date".into(),
                        direction: SortDirection::Asc,
                    }],
                },
                AggregateSpec {
                    alias: "last_qty".into(),
                    fn_: AggregateFn::LastValue,
                    column: Some("qty".into()),
                    distinct: false,
                    order_by: vec![AggregateOrderBy {
                        column: "snapshot_date".into(),
                        direction: SortDirection::Asc,
                    }],
                },
            ],
            order_by: vec![AggregateOrderBy {
                column: "last_qty".into(),
                direction: SortDirection::Desc,
            }],
            limit: Some(200),
            offset: Some(0),
            ..Default::default()
        };
        let sql = build_aggregate_query(&schema, &req).unwrap();
        // FIRST_VALUE keeps direction as-is.
        assert!(sql.data_sql.contains(
            "(array_agg(\"qty\"::bigint ORDER BY \"snapshot_date\"::timestamptz ASC))[1] \
             AS \"first_qty\""
        ));
        // LAST_VALUE flips direction (ASC → DESC).
        assert!(sql.data_sql.contains(
            "(array_agg(\"qty\"::bigint ORDER BY \"snapshot_date\"::timestamptz DESC))[1] \
             AS \"last_qty\""
        ));
        assert!(sql.data_sql.contains("ORDER BY \"last_qty\" DESC"));
    }

    #[test]
    fn sum_rejects_non_numeric_column() {
        let schema = stock_snapshot_schema();
        let req = AggregateRequest {
            aggregates: vec![AggregateSpec {
                alias: "total".into(),
                fn_: AggregateFn::Sum,
                column: Some("sku".into()),
                distinct: false,
                order_by: vec![],
            }],
            ..Default::default()
        };
        let err = build_aggregate_query(&schema, &req).unwrap_err();
        assert!(err.contains("SUM requires a numeric column"), "{}", err);
    }

    #[test]
    fn first_value_requires_order_by() {
        let schema = stock_snapshot_schema();
        let req = AggregateRequest {
            aggregates: vec![AggregateSpec {
                alias: "first_qty".into(),
                fn_: AggregateFn::FirstValue,
                column: Some("qty".into()),
                distinct: false,
                order_by: vec![],
            }],
            ..Default::default()
        };
        let err = build_aggregate_query(&schema, &req).unwrap_err();
        assert!(err.contains("requires non-empty order_by"), "{}", err);
    }

    #[test]
    fn order_by_must_reference_group_or_alias() {
        let schema = stock_snapshot_schema();
        let req = AggregateRequest {
            group_by: vec!["sku".into()],
            aggregates: vec![count_all()],
            order_by: vec![AggregateOrderBy {
                column: "qty".into(),
                direction: SortDirection::Desc,
            }],
            ..Default::default()
        };
        let err = build_aggregate_query(&schema, &req).unwrap_err();
        assert!(err.contains("not in group_by or aggregates"), "{}", err);
    }

    #[test]
    fn duplicate_aliases_rejected() {
        let schema = stock_snapshot_schema();
        let req = AggregateRequest {
            aggregates: vec![count_all(), count_all()],
            ..Default::default()
        };
        let err = build_aggregate_query(&schema, &req).unwrap_err();
        assert!(err.contains("Duplicate aggregate alias"), "{}", err);
    }

    #[test]
    fn unknown_group_by_column_rejected() {
        let schema = stock_snapshot_schema();
        let req = AggregateRequest {
            group_by: vec!["nonexistent".into()],
            aggregates: vec![count_all()],
            ..Default::default()
        };
        let err = build_aggregate_query(&schema, &req).unwrap_err();
        assert!(err.contains("Unknown group_by column"), "{}", err);
    }

    #[test]
    fn empty_aggregates_rejected() {
        let schema = stock_snapshot_schema();
        let req = AggregateRequest {
            aggregates: vec![],
            ..Default::default()
        };
        let err = build_aggregate_query(&schema, &req).unwrap_err();
        assert!(err.contains("at least one"), "{}", err);
    }

    #[test]
    fn min_on_json_column_rejected() {
        let schema = stock_snapshot_schema();
        let req = AggregateRequest {
            aggregates: vec![AggregateSpec {
                alias: "m".into(),
                fn_: AggregateFn::Min,
                column: Some("notes".into()),
                distinct: false,
                order_by: vec![],
            }],
            ..Default::default()
        };
        let err = build_aggregate_query(&schema, &req).unwrap_err();
        assert!(
            err.contains("MIN/MAX cannot be applied to a JSON column"),
            "{}",
            err
        );
    }

    #[test]
    fn alias_validation() {
        assert!(validate_alias("foo").is_ok());
        assert!(validate_alias("_foo").is_ok());
        assert!(validate_alias("foo_bar_123").is_ok());
        assert!(validate_alias("").is_err());
        assert!(validate_alias("1foo").is_err());
        assert!(validate_alias("foo-bar").is_err());
        assert!(validate_alias("foo bar").is_err());
    }

    #[test]
    fn group_count_query_only_when_group_by_present() {
        let schema = stock_snapshot_schema();

        // With group_by.
        let with_group = AggregateRequest {
            group_by: vec!["sku".into()],
            aggregates: vec![count_all()],
            ..Default::default()
        };
        let sql = build_aggregate_query(&schema, &with_group).unwrap();
        let count = sql.count_sql.expect("count_sql");
        assert!(count.contains("SELECT COUNT(*)::bigint FROM"));
        assert!(count.contains("GROUP BY \"sku\""));

        // Without group_by.
        let without = AggregateRequest {
            aggregates: vec![count_all()],
            ..Default::default()
        };
        let sql = build_aggregate_query(&schema, &without).unwrap();
        assert!(sql.count_sql.is_none());
    }

    #[test]
    fn camelcase_system_field_in_group_by_is_mapped() {
        let schema = stock_snapshot_schema();
        let req = AggregateRequest {
            group_by: vec!["createdAt".into()],
            aggregates: vec![count_all()],
            ..Default::default()
        };
        let sql = build_aggregate_query(&schema, &req).unwrap();
        assert_eq!(sql.columns[0], "created_at");
        assert!(sql.data_sql.contains("GROUP BY \"created_at\""));
    }

    #[test]
    fn order_by_empty_on_non_first_last_value_rejected() {
        // Having order_by on a COUNT aggregate is rejected.
        let schema = stock_snapshot_schema();
        let req = AggregateRequest {
            aggregates: vec![AggregateSpec {
                alias: "n".into(),
                fn_: AggregateFn::Count,
                column: None,
                distinct: false,
                order_by: vec![AggregateOrderBy {
                    column: "sku".into(),
                    direction: SortDirection::Asc,
                }],
            }],
            ..Default::default()
        };
        let err = build_aggregate_query(&schema, &req).unwrap_err();
        assert!(
            err.contains("order_by is only valid for FIRST_VALUE/LAST_VALUE"),
            "{}",
            err
        );
    }

    #[test]
    fn condition_params_propagate_then_limit_offset() {
        let schema = stock_snapshot_schema();
        let req = AggregateRequest {
            condition: Some(Condition {
                op: "AND".into(),
                arguments: Some(vec![
                    serde_json::json!({
                        "op": "EQ",
                        "arguments": ["sku", "A"]
                    }),
                    serde_json::json!({
                        "op": "GT",
                        "arguments": ["qty", 5]
                    }),
                ]),
            }),
            group_by: vec!["sku".into()],
            aggregates: vec![count_all()],
            ..Default::default()
        };
        let sql = build_aggregate_query(&schema, &req).unwrap();
        // Two condition params → LIMIT $3 OFFSET $4
        assert_eq!(sql.params.len(), 2);
        assert!(sql.data_sql.contains("LIMIT $3 OFFSET $4"));
    }
}
