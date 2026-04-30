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

use std::collections::{HashMap, HashSet};

use serde::de::{self, Deserializer};
use serde::ser::SerializeStruct;
use serde::{Deserialize, Serialize, Serializer};

use crate::instance::Condition;
use crate::schema::Schema;
use crate::sql::condition::{
    build_condition_clause, build_condition_clause_with_subqueries, field_to_sql, resolve_sql_cast,
};
use crate::sql::expr::{
    AliasKind, AliasSqlMap, ExprNode, column_kind, render_expression, validate_expression,
};
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
    /// Arithmetic mean over a numeric column. Renders as `AVG(col::numeric)`,
    /// returning `numeric` (NULL on an empty filtered set, matching SQL
    /// semantics). Same column/distinct/order_by rules as `SUM`.
    Avg,
    Min,
    Max,
    FirstValue,
    LastValue,
    /// Continuous percentile over a numeric column. Renders as
    /// `percentile_cont(p) WITHIN GROUP (ORDER BY col::numeric <dir>)`.
    /// Requires `percentile` ∈ [0.0, 1.0] and exactly one numeric `order_by`
    /// entry. Returns `numeric` (NULL on an empty filtered set).
    PercentileCont,
    /// Discrete percentile over a numeric column. Same shape as
    /// `PERCENTILE_CONT` but renders `percentile_disc(p) WITHIN GROUP (...)`.
    PercentileDisc,
    /// Sample standard deviation over a numeric column. Renders as
    /// `STDDEV_SAMP(col::numeric)`. Returns NULL when fewer than 2 rows match
    /// (standard SQL).
    StddevSamp,
    /// Sample variance over a numeric column. Renders as
    /// `VAR_SAMP(col::numeric)`. Returns NULL when fewer than 2 rows match.
    VarSamp,
    /// A column computed from previously-declared aliases and constants via
    /// arithmetic / comparison / logical operators. Reads no DB column.
    Expr,
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
    pub(crate) fn as_sql(self) -> &'static str {
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
#[derive(Debug, Clone)]
pub struct AggregateOrderBy {
    pub column: String,
    pub direction: SortDirection,
}

const ORDER_BY_EXPRESSION_PREFIX: &str = "__runtara_order_expression:";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum DistanceOrderFn {
    CosineDistance,
    L2Distance,
}

impl DistanceOrderFn {
    fn name(self) -> &'static str {
        match self {
            DistanceOrderFn::CosineDistance => "COSINE_DISTANCE",
            DistanceOrderFn::L2Distance => "L2_DISTANCE",
        }
    }

    fn pgvector_op(self) -> &'static str {
        match self {
            DistanceOrderFn::CosineDistance => "<=>",
            DistanceOrderFn::L2Distance => "<->",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DistanceOrderExpression {
    #[serde(rename = "fn")]
    fn_: DistanceOrderFn,
    field: String,
    value: Vec<f64>,
}

enum TopLevelOrderByTarget {
    Column(String),
    Distance(DistanceOrderExpression),
}

impl Serialize for AggregateOrderBy {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("AggregateOrderBy", 2)?;
        if let Some(expr) = decode_order_expression(&self.column)
            .transpose()
            .map_err(serde::ser::Error::custom)?
        {
            state.serialize_field("expression", &expr)?;
        } else {
            state.serialize_field("column", &self.column)?;
        }
        state.serialize_field("direction", &self.direction)?;
        state.end()
    }
}

impl<'de> Deserialize<'de> for AggregateOrderBy {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct RawOrderBy {
            column: Option<String>,
            expression: Option<DistanceOrderExpression>,
            #[serde(default)]
            direction: SortDirection,
        }

        let raw = RawOrderBy::deserialize(deserializer)?;
        match (raw.column, raw.expression) {
            (Some(column), None) => Ok(AggregateOrderBy {
                column,
                direction: raw.direction,
            }),
            (None, Some(expression)) => {
                let encoded = encode_order_expression(&expression).map_err(de::Error::custom)?;
                Ok(AggregateOrderBy {
                    column: encoded,
                    direction: raw.direction,
                })
            }
            (Some(_), Some(_)) => Err(de::Error::custom(
                "order_by entry must specify either `column` or `expression`, not both",
            )),
            (None, None) => Err(de::Error::custom(
                "order_by entry must specify either `column` or `expression`",
            )),
        }
    }
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
    /// other function. Must be absent for `EXPR`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub column: Option<String>,

    /// Apply `DISTINCT`. Only valid with `fn = COUNT` and a non-null `column`.
    #[serde(default)]
    pub distinct: bool,

    /// Required for `FIRST_VALUE` / `LAST_VALUE`; ignored for everything else.
    #[serde(default, rename = "orderBy", alias = "order_by")]
    pub order_by: Vec<AggregateOrderBy>,

    /// Required for `EXPR`; rejected for every other function. An expression
    /// tree over prior aliases and constants. Typed as raw JSON here so the
    /// DTO boundary can pass it through; parsed and validated as an
    /// [`ExprNode`] inside `build_aggregate_query`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expression: Option<serde_json::Value>,

    /// Fraction in `[0.0, 1.0]` for `PERCENTILE_CONT` / `PERCENTILE_DISC`.
    /// Required for those two functions, rejected for every other.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub percentile: Option<f64>,
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
    build_aggregate_query_with_subqueries(schema, req, &HashMap::new())
}

pub fn build_aggregate_query_with_subqueries(
    schema: &Schema,
    req: &AggregateRequest,
    subquery_schemas: &HashMap<String, Schema>,
) -> Result<AggregateSql, String> {
    if req.aggregates.is_empty() {
        return Err("aggregates must contain at least one entry".to_string());
    }

    // Validate group_by columns and normalize them to SQL column names first —
    // we need this to reject aliases that collide with group_by column names.
    let mut group_sql_cols: Vec<String> = Vec::with_capacity(req.group_by.len());
    for raw in &req.group_by {
        validate_field_chars(raw, "group_by")?;
        let sql_field = field_to_sql(raw);
        if !column_exists(sql_field, schema) {
            return Err(format!("Unknown group_by column: '{}'", raw));
        }
        group_sql_cols.push(sql_field.to_string());
    }

    // Validate aggregate specs up front, building (alias → inferred kind) as
    // we go so each EXPR can resolve references to prior aliases. Parsed
    // ExprNodes are cached here keyed by alias so we don't deserialize twice.
    let group_sql_set_for_validation: HashSet<&str> =
        group_sql_cols.iter().map(|s| s.as_str()).collect();
    let mut seen_aliases: HashSet<&str> = HashSet::new();
    let mut prior_alias_kinds: Vec<(String, AliasKind)> = Vec::with_capacity(req.aggregates.len());
    let mut parsed_exprs: HashMap<String, ExprNode> = HashMap::new();
    for spec in &req.aggregates {
        validate_alias(&spec.alias)?;
        if !seen_aliases.insert(spec.alias.as_str()) {
            return Err(format!("Duplicate aggregate alias: {}", spec.alias));
        }
        if group_sql_set_for_validation.contains(spec.alias.as_str()) {
            return Err(format!(
                "Aggregate alias '{}' collides with a group_by column",
                spec.alias
            ));
        }
        let (kind, parsed) = validate_spec(spec, schema, &prior_alias_kinds)?;
        if let Some(node) = parsed {
            parsed_exprs.insert(spec.alias.clone(), node);
        }
        prior_alias_kinds.push((spec.alias.clone(), kind));
    }

    // Validate top-level order_by — each column must resolve to either a
    // group_by column or an aggregate alias. Distance expressions sort by a
    // vector field's pgvector distance to a validated literal.
    let group_sql_set: HashSet<&str> = group_sql_cols.iter().map(|s| s.as_str()).collect();
    let alias_set: HashSet<&str> = req.aggregates.iter().map(|a| a.alias.as_str()).collect();
    let mut top_level_order_by: Vec<TopLevelOrderByTarget> = Vec::with_capacity(req.order_by.len());
    for ob in &req.order_by {
        if let Some(expr) = decode_order_expression(&ob.column).transpose()? {
            validate_distance_order_expression(&expr, schema)?;
            top_level_order_by.push(TopLevelOrderByTarget::Distance(expr));
        } else {
            validate_field_chars(&ob.column, "order_by")?;
            let sql_field = field_to_sql(&ob.column);
            let ok = group_sql_set.contains(sql_field) || alias_set.contains(ob.column.as_str());
            if !ok {
                return Err(format!(
                    "order_by column '{}' is not in group_by or aggregates",
                    ob.column
                ));
            }
            let ident = if group_sql_set.contains(sql_field) {
                quote_identifier(sql_field)
            } else {
                quote_identifier(&ob.column)
            };
            top_level_order_by.push(TopLevelOrderByTarget::Column(ident));
        }
    }

    // ---- WHERE clause (condition) -----------------------------------------
    let (where_clause, params) = if let Some(condition) = &req.condition {
        let mut param_offset = 1i32;
        if subquery_schemas.is_empty() {
            build_condition_clause(condition, &mut param_offset, schema)?
        } else {
            build_condition_clause_with_subqueries(
                condition,
                &mut param_offset,
                schema,
                subquery_schemas,
            )?
        }
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

    let mut prior_alias_sql: AliasSqlMap = HashMap::with_capacity(req.aggregates.len());
    for (spec, (_, kind)) in req.aggregates.iter().zip(prior_alias_kinds.iter()) {
        let expr_sql = render_aggregate_expr(spec, schema, &prior_alias_sql, &parsed_exprs)?;
        inner_select_parts.push(format!("{} AS {}", expr_sql, quote_identifier(&spec.alias)));
        output_columns.push(spec.alias.clone());
        prior_alias_sql.insert(spec.alias.clone(), (expr_sql, *kind));
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
            .zip(top_level_order_by.iter())
            .map(|(ob, target)| {
                let target_sql = match target {
                    TopLevelOrderByTarget::Column(ident) => ident.clone(),
                    TopLevelOrderByTarget::Distance(expr) => render_distance_order_expression(expr),
                };
                format!("{} {}", target_sql, ob.direction.as_sql())
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

fn encode_order_expression(expr: &DistanceOrderExpression) -> Result<String, String> {
    let json = serde_json::to_string(expr)
        .map_err(|e| format!("failed to encode order_by expression: {}", e))?;
    Ok(format!("{}{}", ORDER_BY_EXPRESSION_PREFIX, json))
}

fn decode_order_expression(column: &str) -> Option<Result<DistanceOrderExpression, String>> {
    column.strip_prefix(ORDER_BY_EXPRESSION_PREFIX).map(|raw| {
        serde_json::from_str(raw).map_err(|e| format!("invalid encoded order_by expression: {}", e))
    })
}

fn validate_distance_order_expression(
    expr: &DistanceOrderExpression,
    schema: &Schema,
) -> Result<(), String> {
    validate_field_chars(&expr.field, "order_by expression")?;
    let sql_field = field_to_sql(&expr.field);
    let dim = match column_type(sql_field, schema) {
        Some(ColumnType::Vector { dimension, .. }) => *dimension,
        Some(other) => {
            return Err(format!(
                "order_by expression {} requires a vector field; '{}' has type {:?}",
                expr.fn_.name(),
                expr.field,
                other
            ));
        }
        None => {
            return Err(format!(
                "order_by expression {} field '{}' not found in schema",
                expr.fn_.name(),
                expr.field
            ));
        }
    };
    if expr.value.len() as u32 != dim {
        return Err(format!(
            "order_by expression {} vector dimension {} does not match field '{}' dimension {}",
            expr.fn_.name(),
            expr.value.len(),
            expr.field,
            dim
        ));
    }
    for (i, f) in expr.value.iter().enumerate() {
        if !f.is_finite() {
            return Err(format!(
                "order_by expression {} vector element at index {} is not finite ({})",
                expr.fn_.name(),
                i,
                f
            ));
        }
    }
    Ok(())
}

fn render_distance_order_expression(expr: &DistanceOrderExpression) -> String {
    let sql_field = field_to_sql(&expr.field);
    let parts: Vec<String> = expr.value.iter().map(|f| format_pg_float(*f)).collect();
    let vector_literal = format!("[{}]", parts.join(","));
    format!(
        "((array_agg({}))[1] {} '{}'::vector)",
        quote_identifier(sql_field),
        expr.fn_.pgvector_op(),
        vector_literal
    )
}

/// Format an f64 in a way pgvector's text input accepts. Inputs are validated
/// as finite before rendering.
fn format_pg_float(f: f64) -> String {
    serde_json::Number::from_f64(f)
        .map(|n| n.to_string())
        .unwrap_or_else(|| f.to_string())
}

/// Returns `(kind, parsed_expression_node)`. The parsed node is cached by the
/// caller so `render_aggregate_expr` doesn't have to deserialize a second
/// time.
fn validate_spec(
    spec: &AggregateSpec,
    schema: &Schema,
    prior_alias_kinds: &[(String, AliasKind)],
) -> Result<(AliasKind, Option<ExprNode>), String> {
    // Every non-EXPR function rejects an `expression` field to keep the wire
    // format unambiguous.
    if !matches!(spec.fn_, AggregateFn::Expr) && spec.expression.is_some() {
        return Err(format!(
            "Aggregate '{}': `expression` is only valid for EXPR",
            spec.alias
        ));
    }
    if !matches!(
        spec.fn_,
        AggregateFn::PercentileCont | AggregateFn::PercentileDisc
    ) && spec.percentile.is_some()
    {
        return Err(format!(
            "Aggregate '{}': `percentile` is only valid for PERCENTILE_CONT/PERCENTILE_DISC",
            spec.alias
        ));
    }
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
            Ok((AliasKind::Numeric, None))
        }
        AggregateFn::Sum | AggregateFn::Avg => {
            let fn_name = if matches!(spec.fn_, AggregateFn::Sum) {
                "SUM"
            } else {
                "AVG"
            };
            let col = spec.column.as_ref().ok_or_else(|| {
                format!("Aggregate '{}': {} requires a column", spec.alias, fn_name)
            })?;
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
                        "Aggregate '{}': {} requires a numeric column, '{}' is not numeric",
                        spec.alias, fn_name, col
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
            Ok((AliasKind::Numeric, None))
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
            Ok((column_kind(sql, schema), None))
        }
        AggregateFn::StddevSamp | AggregateFn::VarSamp => {
            let fn_name = if matches!(spec.fn_, AggregateFn::StddevSamp) {
                "STDDEV_SAMP"
            } else {
                "VAR_SAMP"
            };
            let col = spec.column.as_ref().ok_or_else(|| {
                format!("Aggregate '{}': {} requires a column", spec.alias, fn_name)
            })?;
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
                        "Aggregate '{}': {} requires a numeric column, '{}' is not numeric",
                        spec.alias, fn_name, col
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
                    "Aggregate '{}': order_by is only valid for FIRST_VALUE/LAST_VALUE/PERCENTILE_*",
                    spec.alias
                ));
            }
            Ok((AliasKind::Numeric, None))
        }
        AggregateFn::PercentileCont | AggregateFn::PercentileDisc => {
            let fn_name = if matches!(spec.fn_, AggregateFn::PercentileCont) {
                "PERCENTILE_CONT"
            } else {
                "PERCENTILE_DISC"
            };
            if spec.column.is_some() {
                return Err(format!(
                    "Aggregate '{}': {} takes its value column from order_by, not `column`",
                    spec.alias, fn_name
                ));
            }
            if spec.distinct {
                return Err(format!(
                    "Aggregate '{}': distinct is only valid for COUNT",
                    spec.alias
                ));
            }
            let p = spec.percentile.ok_or_else(|| {
                format!(
                    "Aggregate '{}': {} requires `percentile` ∈ [0.0, 1.0]",
                    spec.alias, fn_name
                )
            })?;
            if !p.is_finite() || !(0.0..=1.0).contains(&p) {
                return Err(format!(
                    "Aggregate '{}': {} `percentile` must be a finite number in [0.0, 1.0], got {}",
                    spec.alias, fn_name, p
                ));
            }
            if spec.order_by.len() != 1 {
                return Err(format!(
                    "Aggregate '{}': {} requires exactly one order_by entry (the value column)",
                    spec.alias, fn_name
                ));
            }
            let ob = &spec.order_by[0];
            validate_field_chars(&ob.column, "aggregate order_by")?;
            let osql = field_to_sql(&ob.column);
            match column_type(osql, schema) {
                Some(ColumnType::Integer) | Some(ColumnType::Decimal { .. }) => {}
                Some(_) => {
                    return Err(format!(
                        "Aggregate '{}': {} requires a numeric order_by column, '{}' is not numeric",
                        spec.alias, fn_name, ob.column
                    ));
                }
                None => {
                    return Err(format!(
                        "Aggregate '{}': order_by column '{}' not found in schema",
                        spec.alias, ob.column
                    ));
                }
            }
            Ok((AliasKind::Numeric, None))
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
            Ok((column_kind(sql, schema), None))
        }
        AggregateFn::Expr => {
            let raw = spec.expression.as_ref().ok_or_else(|| {
                format!("Aggregate '{}': EXPR requires an `expression`", spec.alias)
            })?;
            if spec.column.is_some() {
                return Err(format!(
                    "Aggregate '{}': EXPR must not specify `column`",
                    spec.alias
                ));
            }
            if spec.distinct {
                return Err(format!(
                    "Aggregate '{}': EXPR must not specify `distinct`",
                    spec.alias
                ));
            }
            if !spec.order_by.is_empty() {
                return Err(format!(
                    "Aggregate '{}': EXPR must not specify `order_by`",
                    spec.alias
                ));
            }
            let node: ExprNode = serde_json::from_value(raw.clone()).map_err(|e| {
                format!(
                    "Aggregate '{}': invalid `expression` JSON: {}",
                    spec.alias, e
                )
            })?;
            let kind = validate_expression(&node, prior_alias_kinds, 0)
                .map_err(|e| format!("Aggregate '{}': {}", spec.alias, e))?;
            Ok((kind, Some(node)))
        }
    }
}

// ============================================================================
// SQL rendering
// ============================================================================

fn render_aggregate_expr(
    spec: &AggregateSpec,
    schema: &Schema,
    prior_alias_sql: &AliasSqlMap,
    parsed_exprs: &HashMap<String, ExprNode>,
) -> Result<String, String> {
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
        AggregateFn::Sum | AggregateFn::Avg => {
            let col = spec.column.as_ref().unwrap();
            let sql = field_to_sql(col);
            // Both render as numeric — covers Integer and Decimal source
            // columns without loss. AVG returns NULL on an empty filtered set
            // (standard SQL); callers can wrap in COALESCE via `EXPR` if they
            // want a default.
            let fn_name = if matches!(spec.fn_, AggregateFn::Sum) {
                "SUM"
            } else {
                "AVG"
            };
            Ok(format!("{}({}::numeric)", fn_name, quote_identifier(sql)))
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
        AggregateFn::StddevSamp | AggregateFn::VarSamp => {
            let col = spec.column.as_ref().unwrap();
            let sql = field_to_sql(col);
            let fn_name = if matches!(spec.fn_, AggregateFn::StddevSamp) {
                "STDDEV_SAMP"
            } else {
                "VAR_SAMP"
            };
            Ok(format!("{}({}::numeric)", fn_name, quote_identifier(sql)))
        }
        AggregateFn::PercentileCont | AggregateFn::PercentileDisc => {
            let p = spec.percentile.unwrap();
            let fn_name = if matches!(spec.fn_, AggregateFn::PercentileCont) {
                "percentile_cont"
            } else {
                "percentile_disc"
            };
            let ob = &spec.order_by[0];
            let osql = field_to_sql(&ob.column);
            Ok(format!(
                "{}({:.6}) WITHIN GROUP (ORDER BY {}::numeric {})",
                fn_name,
                p,
                quote_identifier(osql),
                ob.direction.as_sql(),
            ))
        }
        AggregateFn::Expr => {
            // Use the ExprNode that was parsed during validation — saves a
            // second serde round-trip and guarantees the rendered tree is
            // the same one that passed validation.
            let node = parsed_exprs.get(spec.alias.as_str()).expect(
                "validate_spec should have cached the parsed ExprNode for every EXPR alias",
            );
            render_expression(node, prior_alias_sql, 0)
                .map_err(|e| format!("Aggregate '{}': {}", spec.alias, e))
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
                    text_index: crate::types::TextIndexKind::None,
                },
                ColumnDefinition {
                    name: "qty".into(),
                    column_type: ColumnType::Integer,
                    nullable: true,
                    unique: false,
                    default_value: None,
                    text_index: crate::types::TextIndexKind::None,
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
                    text_index: crate::types::TextIndexKind::None,
                },
                ColumnDefinition {
                    name: "snapshot_date".into(),
                    column_type: ColumnType::Timestamp,
                    nullable: true,
                    unique: false,
                    default_value: None,
                    text_index: crate::types::TextIndexKind::None,
                },
                ColumnDefinition {
                    name: "notes".into(),
                    column_type: ColumnType::Json,
                    nullable: true,
                    unique: false,
                    default_value: None,
                    text_index: crate::types::TextIndexKind::None,
                },
                ColumnDefinition {
                    name: "embedding".into(),
                    column_type: ColumnType::Vector {
                        dimension: 3,
                        index_method: None,
                    },
                    nullable: true,
                    unique: false,
                    default_value: None,
                    text_index: crate::types::TextIndexKind::None,
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
            expression: None,
            percentile: None,
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
                expression: None,
                percentile: None,
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
                expression: None,
                percentile: None,
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
                    expression: None,
                    percentile: None,
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
                    expression: None,
                    percentile: None,
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
                expression: None,
                percentile: None,
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
                expression: None,
                percentile: None,
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
    fn top_level_order_by_cosine_distance_renders_pgvector_operator() {
        let schema = stock_snapshot_schema();
        let req: AggregateRequest = serde_json::from_value(serde_json::json!({
            "groupBy": ["sku"],
            "aggregates": [{"alias": "n", "fn": "COUNT"}],
            "orderBy": [{
                "expression": {
                    "fn": "COSINE_DISTANCE",
                    "field": "embedding",
                    "value": [0.1, 0.2, 0.3]
                },
                "direction": "ASC"
            }]
        }))
        .unwrap();
        let sql = build_aggregate_query(&schema, &req).unwrap();
        assert!(
            sql.data_sql.contains(
                "ORDER BY ((array_agg(\"embedding\"))[1] <=> '[0.1,0.2,0.3]'::vector) ASC"
            ),
            "{}",
            sql.data_sql
        );
        assert!(sql.params.is_empty());
        assert!(sql.data_sql.contains("LIMIT $1 OFFSET $2"));
    }

    #[test]
    fn top_level_order_by_l2_distance_renders_pgvector_operator() {
        let schema = stock_snapshot_schema();
        let req: AggregateRequest = serde_json::from_value(serde_json::json!({
            "aggregates": [{"alias": "n", "fn": "COUNT"}],
            "orderBy": [{
                "expression": {
                    "fn": "L2_DISTANCE",
                    "field": "embedding",
                    "value": [1, 2, 3]
                },
                "direction": "DESC"
            }]
        }))
        .unwrap();
        let sql = build_aggregate_query(&schema, &req).unwrap();
        assert!(
            sql.data_sql.contains(
                "ORDER BY ((array_agg(\"embedding\"))[1] <-> '[1.0,2.0,3.0]'::vector) DESC"
            ),
            "{}",
            sql.data_sql
        );
    }

    #[test]
    fn top_level_order_by_distance_validates_vector_field_and_dimension() {
        let schema = stock_snapshot_schema();
        let non_vector: AggregateRequest = serde_json::from_value(serde_json::json!({
            "aggregates": [{"alias": "n", "fn": "COUNT"}],
            "orderBy": [{
                "expression": {
                    "fn": "COSINE_DISTANCE",
                    "field": "sku",
                    "value": [0.1, 0.2, 0.3]
                }
            }]
        }))
        .unwrap();
        let err = build_aggregate_query(&schema, &non_vector).unwrap_err();
        assert!(err.contains("requires a vector field"), "{}", err);

        let wrong_dimension: AggregateRequest = serde_json::from_value(serde_json::json!({
            "aggregates": [{"alias": "n", "fn": "COUNT"}],
            "orderBy": [{
                "expression": {
                    "fn": "L2_DISTANCE",
                    "field": "embedding",
                    "value": [0.1, 0.2]
                }
            }]
        }))
        .unwrap();
        let err = build_aggregate_query(&schema, &wrong_dimension).unwrap_err();
        assert!(err.contains("dimension 2 does not match"), "{}", err);

        let bad_field: AggregateRequest = serde_json::from_value(serde_json::json!({
            "aggregates": [{"alias": "n", "fn": "COUNT"}],
            "orderBy": [{
                "expression": {
                    "fn": "COSINE_DISTANCE",
                    "field": "embed ding",
                    "value": [0.1, 0.2, 0.3]
                }
            }]
        }))
        .unwrap();
        let err = build_aggregate_query(&schema, &bad_field).unwrap_err();
        assert!(err.contains("contains invalid characters"), "{}", err);
    }

    #[test]
    fn top_level_order_by_distance_requires_numeric_array() {
        let err = serde_json::from_value::<AggregateRequest>(serde_json::json!({
            "aggregates": [{"alias": "n", "fn": "COUNT"}],
            "orderBy": [{
                "expression": {
                    "fn": "COSINE_DISTANCE",
                    "field": "embedding",
                    "value": [0.1, "nope", 0.3]
                }
            }]
        }))
        .unwrap_err();
        assert!(err.to_string().contains("invalid type"), "{}", err);
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
                expression: None,
                percentile: None,
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
                expression: None,
                percentile: None,
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

    // ========================================================================
    // EXPR aggregate (v1.1) tests
    // ========================================================================

    fn first_last_qty_specs() -> Vec<AggregateSpec> {
        vec![
            AggregateSpec {
                alias: "first_qty".into(),
                fn_: AggregateFn::FirstValue,
                column: Some("qty".into()),
                distinct: false,
                order_by: vec![AggregateOrderBy {
                    column: "snapshot_date".into(),
                    direction: SortDirection::Asc,
                }],
                expression: None,
                percentile: None,
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
                expression: None,
                percentile: None,
            },
        ]
    }

    fn expr_spec(alias: &str, expression: serde_json::Value) -> AggregateSpec {
        AggregateSpec {
            alias: alias.into(),
            fn_: AggregateFn::Expr,
            column: None,
            distinct: false,
            order_by: vec![],
            expression: Some(expression),
            percentile: None,
        }
    }

    #[test]
    fn expr_delta_compiles_with_inlined_alias_sql() {
        let schema = stock_snapshot_schema();
        let mut aggregates = first_last_qty_specs();
        aggregates.push(expr_spec(
            "delta",
            serde_json::json!({
                "op": "SUB",
                "arguments": [
                    {"valueType": "alias", "value": "last_qty"},
                    {"valueType": "alias", "value": "first_qty"}
                ]
            }),
        ));
        let req = AggregateRequest {
            group_by: vec!["sku".into()],
            aggregates,
            ..Default::default()
        };
        let sql = build_aggregate_query(&schema, &req).unwrap();
        assert_eq!(
            sql.columns,
            vec![
                "sku".to_string(),
                "first_qty".to_string(),
                "last_qty".to_string(),
                "delta".to_string()
            ]
        );
        // Both aggregate SQLs are inlined inside the delta expression.
        assert!(
            sql.data_sql
                .matches("array_agg(\"qty\"::bigint ORDER BY")
                .count()
                >= 4,
            "expected alias SQL inlined into delta: {}",
            sql.data_sql
        );
        assert!(sql.data_sql.contains(" - "), "{}", sql.data_sql);
        assert!(sql.data_sql.contains("AS \"delta\""), "{}", sql.data_sql);
    }

    #[test]
    fn expr_missing_expression_rejected() {
        let schema = stock_snapshot_schema();
        let req = AggregateRequest {
            aggregates: vec![AggregateSpec {
                alias: "x".into(),
                fn_: AggregateFn::Expr,
                column: None,
                distinct: false,
                order_by: vec![],
                expression: None,
                percentile: None,
            }],
            ..Default::default()
        };
        let err = build_aggregate_query(&schema, &req).unwrap_err();
        assert!(err.contains("EXPR requires an `expression`"), "{}", err);
    }

    #[test]
    fn expr_with_column_rejected() {
        let schema = stock_snapshot_schema();
        let req = AggregateRequest {
            aggregates: vec![AggregateSpec {
                alias: "x".into(),
                fn_: AggregateFn::Expr,
                column: Some("qty".into()),
                distinct: false,
                order_by: vec![],
                expression: Some(serde_json::json!({
                    "valueType": "immediate", "value": 1
                })),
                percentile: None,
            }],
            ..Default::default()
        };
        let err = build_aggregate_query(&schema, &req).unwrap_err();
        assert!(err.contains("EXPR must not specify `column`"), "{}", err);
    }

    #[test]
    fn expression_on_non_expr_rejected() {
        let schema = stock_snapshot_schema();
        let req = AggregateRequest {
            aggregates: vec![AggregateSpec {
                alias: "total".into(),
                fn_: AggregateFn::Sum,
                column: Some("qty".into()),
                distinct: false,
                order_by: vec![],
                expression: Some(serde_json::json!({
                    "valueType": "immediate", "value": 1
                })),
                percentile: None,
            }],
            ..Default::default()
        };
        let err = build_aggregate_query(&schema, &req).unwrap_err();
        assert!(
            err.contains("`expression` is only valid for EXPR"),
            "{}",
            err
        );
    }

    #[test]
    fn expr_invalid_json_shape_rejected() {
        let schema = stock_snapshot_schema();
        let req = AggregateRequest {
            aggregates: vec![AggregateSpec {
                alias: "x".into(),
                fn_: AggregateFn::Expr,
                column: None,
                distinct: false,
                order_by: vec![],
                // Missing both `op` and `valueType` — matches no ExprNode variant.
                expression: Some(serde_json::json!({"nope": 1})),
                percentile: None,
            }],
            ..Default::default()
        };
        let err = build_aggregate_query(&schema, &req).unwrap_err();
        assert!(err.contains("invalid `expression` JSON"), "{}", err);
    }

    #[test]
    fn expr_alias_out_of_order_rejected() {
        let schema = stock_snapshot_schema();
        let mut aggregates = vec![expr_spec(
            "delta",
            serde_json::json!({
                "op": "SUB",
                "arguments": [
                    {"valueType": "alias", "value": "last_qty"},
                    {"valueType": "alias", "value": "first_qty"}
                ]
            }),
        )];
        // first/last declared AFTER delta
        aggregates.extend(first_last_qty_specs());
        let req = AggregateRequest {
            group_by: vec!["sku".into()],
            aggregates,
            ..Default::default()
        };
        let err = build_aggregate_query(&schema, &req).unwrap_err();
        assert!(err.contains("is not declared before its use"), "{}", err);
    }

    #[test]
    fn expr_field_reference_rejected() {
        let schema = stock_snapshot_schema();
        let req = AggregateRequest {
            aggregates: vec![expr_spec(
                "x",
                serde_json::json!({
                    "op": "GT",
                    "arguments": [
                        {"valueType": "reference", "value": "qty"},
                        {"valueType": "immediate", "value": 5}
                    ]
                }),
            )],
            ..Default::default()
        };
        let err = build_aggregate_query(&schema, &req).unwrap_err();
        assert!(
            err.contains("field reference 'qty' is not allowed inside EXPR"),
            "{}",
            err
        );
    }

    #[test]
    fn expr_div_renders_nullif() {
        let schema = stock_snapshot_schema();
        let mut aggregates = first_last_qty_specs();
        aggregates.push(expr_spec(
            "ratio",
            serde_json::json!({
                "op": "DIV",
                "arguments": [
                    {"valueType": "alias", "value": "last_qty"},
                    {"valueType": "alias", "value": "first_qty"}
                ]
            }),
        ));
        let req = AggregateRequest {
            group_by: vec!["sku".into()],
            aggregates,
            ..Default::default()
        };
        let sql = build_aggregate_query(&schema, &req).unwrap();
        assert!(sql.data_sql.contains("NULLIF("), "{}", sql.data_sql);
    }

    #[test]
    fn expr_arithmetic_on_text_alias_rejected() {
        let schema = stock_snapshot_schema();
        let req = AggregateRequest {
            group_by: vec!["sku".into()],
            aggregates: vec![
                AggregateSpec {
                    alias: "min_sku".into(),
                    fn_: AggregateFn::Min,
                    column: Some("sku".into()),
                    distinct: false,
                    order_by: vec![],
                    expression: None,
                    percentile: None,
                },
                expr_spec(
                    "bad",
                    serde_json::json!({
                        "op": "SUB",
                        "arguments": [
                            {"valueType": "alias", "value": "min_sku"},
                            {"valueType": "immediate", "value": 1}
                        ]
                    }),
                ),
            ],
            ..Default::default()
        };
        let err = build_aggregate_query(&schema, &req).unwrap_err();
        assert!(err.contains("must be numeric"), "{}", err);
    }

    #[test]
    fn expr_order_by_on_expr_alias_renders_in_sql() {
        let schema = stock_snapshot_schema();
        let mut aggregates = first_last_qty_specs();
        aggregates.push(expr_spec(
            "delta",
            serde_json::json!({
                "op": "SUB",
                "arguments": [
                    {"valueType": "alias", "value": "last_qty"},
                    {"valueType": "alias", "value": "first_qty"}
                ]
            }),
        ));
        aggregates.push(expr_spec(
            "delta_abs",
            serde_json::json!({
                "op": "ABS",
                "arguments": [
                    {"valueType": "alias", "value": "delta"}
                ]
            }),
        ));
        let req = AggregateRequest {
            group_by: vec!["sku".into()],
            aggregates,
            order_by: vec![AggregateOrderBy {
                column: "delta_abs".into(),
                direction: SortDirection::Desc,
            }],
            limit: Some(10),
            offset: Some(0),
            ..Default::default()
        };
        let sql = build_aggregate_query(&schema, &req).unwrap();
        assert!(
            sql.data_sql.contains("ORDER BY \"delta_abs\" DESC"),
            "{}",
            sql.data_sql
        );
    }

    #[test]
    fn expr_alias_collides_with_group_by() {
        let schema = stock_snapshot_schema();
        let req = AggregateRequest {
            group_by: vec!["sku".into()],
            aggregates: vec![AggregateSpec {
                alias: "sku".into(),
                fn_: AggregateFn::Count,
                column: None,
                distinct: false,
                order_by: vec![],
                expression: None,
                percentile: None,
            }],
            ..Default::default()
        };
        let err = build_aggregate_query(&schema, &req).unwrap_err();
        assert!(err.contains("collides with a group_by column"), "{}", err);
    }

    #[test]
    fn expr_v1_0_spec_serializes_without_expression_field() {
        let spec = AggregateSpec {
            alias: "total".into(),
            fn_: AggregateFn::Sum,
            column: Some("qty".into()),
            distinct: false,
            order_by: vec![],
            expression: None,
            percentile: None,
        };
        let json = serde_json::to_value(&spec).unwrap();
        assert!(
            json.get("expression").is_none(),
            "v1.0 specs should not emit `expression`: {}",
            json
        );
    }

    // ========================================================================
    // PERCENTILE / STDDEV / VARIANCE tests
    // ========================================================================

    fn percentile_spec(alias: &str, p: f64, value_col: &str) -> AggregateSpec {
        AggregateSpec {
            alias: alias.into(),
            fn_: AggregateFn::PercentileCont,
            column: None,
            distinct: false,
            order_by: vec![AggregateOrderBy {
                column: value_col.into(),
                direction: SortDirection::Asc,
            }],
            expression: None,
            percentile: Some(p),
        }
    }

    #[test]
    fn percentile_cont_renders_within_group() {
        let schema = stock_snapshot_schema();
        let req = AggregateRequest {
            group_by: vec!["sku".into()],
            aggregates: vec![percentile_spec("p50_qty", 0.5, "qty")],
            ..Default::default()
        };
        let sql = build_aggregate_query(&schema, &req).unwrap();
        assert!(
            sql.data_sql
                .contains("percentile_cont(0.500000) WITHIN GROUP (ORDER BY \"qty\"::numeric ASC)"),
            "{}",
            sql.data_sql
        );
    }

    #[test]
    fn percentile_disc_renders_within_group() {
        let schema = stock_snapshot_schema();
        let mut spec = percentile_spec("p95_price", 0.95, "price");
        spec.fn_ = AggregateFn::PercentileDisc;
        let req = AggregateRequest {
            aggregates: vec![spec],
            ..Default::default()
        };
        let sql = build_aggregate_query(&schema, &req).unwrap();
        assert!(
            sql.data_sql.contains(
                "percentile_disc(0.950000) WITHIN GROUP (ORDER BY \"price\"::numeric ASC)"
            ),
            "{}",
            sql.data_sql
        );
    }

    #[test]
    fn percentile_requires_fraction_in_unit_interval() {
        let schema = stock_snapshot_schema();
        let mut spec = percentile_spec("bad", 1.5, "qty");
        let req = AggregateRequest {
            aggregates: vec![spec.clone()],
            ..Default::default()
        };
        let err = build_aggregate_query(&schema, &req).unwrap_err();
        assert!(
            err.contains("must be a finite number in [0.0, 1.0]"),
            "{}",
            err
        );

        spec.percentile = Some(f64::NAN);
        let req = AggregateRequest {
            aggregates: vec![spec.clone()],
            ..Default::default()
        };
        let err = build_aggregate_query(&schema, &req).unwrap_err();
        assert!(err.contains("must be a finite number"), "{}", err);

        spec.percentile = None;
        let req = AggregateRequest {
            aggregates: vec![spec],
            ..Default::default()
        };
        let err = build_aggregate_query(&schema, &req).unwrap_err();
        assert!(err.contains("requires `percentile`"), "{}", err);
    }

    #[test]
    fn percentile_requires_single_numeric_order_by() {
        let schema = stock_snapshot_schema();
        // zero entries
        let mut spec = percentile_spec("p", 0.5, "qty");
        spec.order_by.clear();
        let req = AggregateRequest {
            aggregates: vec![spec],
            ..Default::default()
        };
        let err = build_aggregate_query(&schema, &req).unwrap_err();
        assert!(
            err.contains("requires exactly one order_by entry"),
            "{}",
            err
        );

        // non-numeric column
        let spec = percentile_spec("p", 0.5, "sku");
        let req = AggregateRequest {
            aggregates: vec![spec],
            ..Default::default()
        };
        let err = build_aggregate_query(&schema, &req).unwrap_err();
        assert!(
            err.contains("requires a numeric order_by column"),
            "{}",
            err
        );
    }

    #[test]
    fn percentile_rejects_column_field() {
        let schema = stock_snapshot_schema();
        let mut spec = percentile_spec("p", 0.5, "qty");
        spec.column = Some("price".into());
        let req = AggregateRequest {
            aggregates: vec![spec],
            ..Default::default()
        };
        let err = build_aggregate_query(&schema, &req).unwrap_err();
        assert!(
            err.contains("takes its value column from order_by"),
            "{}",
            err
        );
    }

    #[test]
    fn percentile_field_rejected_on_non_percentile_fn() {
        let schema = stock_snapshot_schema();
        let req = AggregateRequest {
            aggregates: vec![AggregateSpec {
                alias: "total".into(),
                fn_: AggregateFn::Sum,
                column: Some("qty".into()),
                distinct: false,
                order_by: vec![],
                expression: None,
                percentile: Some(0.5),
            }],
            ..Default::default()
        };
        let err = build_aggregate_query(&schema, &req).unwrap_err();
        assert!(
            err.contains("`percentile` is only valid for PERCENTILE_CONT/PERCENTILE_DISC"),
            "{}",
            err
        );
    }

    #[test]
    fn stddev_samp_renders_numeric_cast() {
        let schema = stock_snapshot_schema();
        let req = AggregateRequest {
            group_by: vec!["sku".into()],
            aggregates: vec![AggregateSpec {
                alias: "vol".into(),
                fn_: AggregateFn::StddevSamp,
                column: Some("qty".into()),
                distinct: false,
                order_by: vec![],
                expression: None,
                percentile: None,
            }],
            ..Default::default()
        };
        let sql = build_aggregate_query(&schema, &req).unwrap();
        assert!(
            sql.data_sql.contains("STDDEV_SAMP(\"qty\"::numeric)"),
            "{}",
            sql.data_sql
        );
    }

    #[test]
    fn var_samp_rejects_text_column() {
        let schema = stock_snapshot_schema();
        let req = AggregateRequest {
            aggregates: vec![AggregateSpec {
                alias: "vv".into(),
                fn_: AggregateFn::VarSamp,
                column: Some("sku".into()),
                distinct: false,
                order_by: vec![],
                expression: None,
                percentile: None,
            }],
            ..Default::default()
        };
        let err = build_aggregate_query(&schema, &req).unwrap_err();
        assert!(
            err.contains("VAR_SAMP requires a numeric column"),
            "{}",
            err
        );
    }
}
