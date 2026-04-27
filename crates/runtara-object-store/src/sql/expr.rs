//! Expression trees for `fn: "EXPR"` aggregates.
//!
//! An EXPR aggregate computes a column from already-declared aliases (or
//! constants) using arithmetic / comparison / logical operators. No DB column
//! is read — alias operands resolve to prior aggregates in the same request,
//! which are textually substituted into the compiled SQL.
//!
//! Layering note: this module is intentionally independent of `runtara-dsl`
//! so the store crate doesn't pick up the full mapping-value / template
//! hierarchy for what is a small arithmetic/boolean mini-language.
//!
//! JSON wire shape (untagged — variants distinguished by field presence):
//! - `{"op": "...", "arguments": [...]}` — operation node
//! - `{"valueType": "alias" | "immediate" | "reference", "value": ...}` — operand
//!
//! `reference` is parsed so we can emit a precise error; it is always rejected
//! inside an EXPR (row-level values don't exist post-aggregation).
//!
//! Depth is capped at [`EXPR_MAX_DEPTH`] in both validation and rendering.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::schema::Schema;
use crate::types::ColumnType;

/// Maximum nesting depth for an expression tree. Prevents pathological input.
pub const EXPR_MAX_DEPTH: u8 = 8;

// ============================================================================
// Public types
// ============================================================================

/// Operators supported inside an EXPR expression tree.
///
/// v1.1 scope: arithmetic + COALESCE + comparison + boolean + nullability.
/// String / array operators are deferred to a future version.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ExprOp {
    Add,
    Sub,
    Mul,
    Div,
    Neg,
    Abs,
    Coalesce,
    Eq,
    Ne,
    Gt,
    Gte,
    Lt,
    Lte,
    And,
    Or,
    Not,
    IsDefined,
    IsEmpty,
    IsNotEmpty,
}

impl ExprOp {
    fn name(self) -> &'static str {
        match self {
            ExprOp::Add => "ADD",
            ExprOp::Sub => "SUB",
            ExprOp::Mul => "MUL",
            ExprOp::Div => "DIV",
            ExprOp::Neg => "NEG",
            ExprOp::Abs => "ABS",
            ExprOp::Coalesce => "COALESCE",
            ExprOp::Eq => "EQ",
            ExprOp::Ne => "NE",
            ExprOp::Gt => "GT",
            ExprOp::Gte => "GTE",
            ExprOp::Lt => "LT",
            ExprOp::Lte => "LTE",
            ExprOp::And => "AND",
            ExprOp::Or => "OR",
            ExprOp::Not => "NOT",
            ExprOp::IsDefined => "IS_DEFINED",
            ExprOp::IsEmpty => "IS_EMPTY",
            ExprOp::IsNotEmpty => "IS_NOT_EMPTY",
        }
    }
}

/// A node in an EXPR expression tree.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ExprNode {
    Operation(ExprOperation),
    /// Whitelisted function call (e.g. `similarity`, `greatest`). Distinguished
    /// from `Operation` by serde via the `fn` field.
    Fn(ExprFnCall),
    Value(ExprValue),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExprOperation {
    pub op: ExprOp,
    pub arguments: Vec<ExprNode>,
}

/// A whitelisted function-call node. Currently only used by row-level
/// scoring (`score_expression`), not by aggregate `EXPR`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExprFnCall {
    #[serde(rename = "fn")]
    pub fn_: ExprFn,
    pub arguments: Vec<ExprNode>,
}

/// Whitelisted SQL functions exposed to row-level scoring expressions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ExprFn {
    /// `pg_trgm` `similarity(text, text) -> real` (range 0.0..1.0).
    Similarity,
    /// PostgreSQL `GREATEST(numeric, ...)` — takes >= 1 numeric arguments.
    Greatest,
    /// PostgreSQL `LEAST(numeric, ...)` — takes >= 1 numeric arguments.
    Least,
    /// PostgreSQL `ts_rank(tsvector, tsquery) -> real` for full-text scoring.
    /// Two arguments: a column reference to a `tsvector` column and a string
    /// query to be tokenized via `plainto_tsquery`.
    TsRank,
}

impl ExprFn {
    fn name(self) -> &'static str {
        match self {
            ExprFn::Similarity => "SIMILARITY",
            ExprFn::Greatest => "GREATEST",
            ExprFn::Least => "LEAST",
            ExprFn::TsRank => "TS_RANK",
        }
    }
}

/// Operand variants — tagged by `valueType` (matches existing `MappingValue`
/// shape used by the agent/DSL layer).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "valueType", rename_all = "lowercase")]
pub enum ExprValue {
    /// Reference to a previously-declared aggregate alias in the same request.
    Alias { value: String },
    /// Literal value: JSON number, bool, string, or null.
    /// Arrays/objects are rejected at validation.
    Immediate { value: serde_json::Value },
    /// Field reference — parsed only so the validator can produce a precise
    /// error. Always rejected inside an EXPR.
    Reference { value: String },
}

/// Coarse type classification used for EXPR type-checking.
///
/// `Nullable` is an internal kind used only during type inference to
/// represent a bare `null` literal — it promotes to whatever sibling kind
/// the expression expects (matching PG's `NULL` propagation).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AliasKind {
    Numeric,
    Text,
    Bool,
    Timestamp,
    Json,
    Nullable,
}

/// Compile a column's schema type into an [`AliasKind`].
pub(crate) fn column_kind(sql_field: &str, schema: &Schema) -> AliasKind {
    match sql_field {
        "id" => return AliasKind::Text,
        "created_at" | "updated_at" => return AliasKind::Timestamp,
        _ => {}
    }
    match schema.columns.iter().find(|c| c.name == sql_field) {
        Some(col) => match &col.column_type {
            ColumnType::String | ColumnType::Enum { .. } => AliasKind::Text,
            ColumnType::Integer | ColumnType::Decimal { .. } => AliasKind::Numeric,
            ColumnType::Boolean => AliasKind::Bool,
            ColumnType::Timestamp => AliasKind::Timestamp,
            ColumnType::Json => AliasKind::Json,
            // tsvector columns surface as Json so generic aggregate/expression
            // operators reject them; TS_RANK does its own column-type check
            // via `is_tsvector_column` below.
            ColumnType::Tsvector { .. } => AliasKind::Json,
        },
        None => AliasKind::Text,
    }
}

fn is_tsvector_column(name: &str, schema: &Schema) -> bool {
    schema
        .columns
        .iter()
        .find(|c| c.name == name)
        .map(|c| matches!(c.column_type, ColumnType::Tsvector { .. }))
        .unwrap_or(false)
}

// ============================================================================
// Validation
// ============================================================================

/// Validate an expression tree and infer its result kind.
///
/// `prior` is the ordered list of aliases already declared (with their
/// inferred kinds). Aliases must resolve against this list — forward refs
/// are rejected.
pub(crate) fn validate_expression(
    node: &ExprNode,
    prior: &[(String, AliasKind)],
    depth: u8,
) -> Result<AliasKind, String> {
    if depth > EXPR_MAX_DEPTH {
        return Err(format!(
            "expression tree exceeds max depth ({})",
            EXPR_MAX_DEPTH
        ));
    }
    match node {
        ExprNode::Value(ExprValue::Reference { value }) => Err(format!(
            "field reference '{}' is not allowed inside EXPR — row-level \
             values do not exist after aggregation; declare an aggregate \
             alias and reference it instead",
            value
        )),
        ExprNode::Value(ExprValue::Alias { value }) => prior
            .iter()
            .find(|(name, _)| name == value)
            .map(|(_, k)| *k)
            .ok_or_else(|| {
                format!(
                    "alias '{}' is not declared before its use (aliases must \
                     be listed earlier in the aggregates array)",
                    value
                )
            }),
        ExprNode::Value(ExprValue::Immediate { value }) => literal_kind(value),
        ExprNode::Operation(op) => validate_operation(op, prior, depth),
        ExprNode::Fn(call) => Err(format!(
            "function call '{}' is not allowed inside EXPR — only \
             arithmetic / comparison / boolean operators",
            call.fn_.name()
        )),
    }
}

fn literal_kind(v: &serde_json::Value) -> Result<AliasKind, String> {
    match v {
        serde_json::Value::Null => Ok(AliasKind::Nullable),
        serde_json::Value::Bool(_) => Ok(AliasKind::Bool),
        serde_json::Value::Number(_) => Ok(AliasKind::Numeric),
        serde_json::Value::String(_) => Ok(AliasKind::Text),
        serde_json::Value::Array(_) | serde_json::Value::Object(_) => Err(
            "composite literals (arrays / objects) are not allowed in EXPR immediates".to_string(),
        ),
    }
}

fn validate_operation(
    op: &ExprOperation,
    prior: &[(String, AliasKind)],
    depth: u8,
) -> Result<AliasKind, String> {
    check_arity(op)?;
    let kinds: Vec<AliasKind> = op
        .arguments
        .iter()
        .map(|a| validate_expression(a, prior, depth + 1))
        .collect::<Result<_, _>>()?;

    match op.op {
        ExprOp::Add | ExprOp::Sub | ExprOp::Mul | ExprOp::Div | ExprOp::Neg | ExprOp::Abs => {
            for (i, k) in kinds.iter().enumerate() {
                if !is_numeric_compatible(*k) {
                    return Err(format!(
                        "{}: argument {} must be numeric, got {:?}",
                        op.op.name(),
                        i,
                        k
                    ));
                }
            }
            Ok(AliasKind::Numeric)
        }
        ExprOp::Coalesce => coalesce_kind(op.op, &kinds),
        ExprOp::Eq | ExprOp::Ne | ExprOp::Gt | ExprOp::Gte | ExprOp::Lt | ExprOp::Lte => {
            check_comparable(op.op, kinds[0], kinds[1])?;
            Ok(AliasKind::Bool)
        }
        ExprOp::And | ExprOp::Or => {
            for (i, k) in kinds.iter().enumerate() {
                if !matches!(k, AliasKind::Bool | AliasKind::Nullable) {
                    return Err(format!(
                        "{}: argument {} must be boolean, got {:?}",
                        op.op.name(),
                        i,
                        k
                    ));
                }
            }
            Ok(AliasKind::Bool)
        }
        ExprOp::Not => {
            if !matches!(kinds[0], AliasKind::Bool | AliasKind::Nullable) {
                return Err(format!("NOT: argument must be boolean, got {:?}", kinds[0]));
            }
            Ok(AliasKind::Bool)
        }
        ExprOp::IsDefined | ExprOp::IsEmpty | ExprOp::IsNotEmpty => Ok(AliasKind::Bool),
    }
}

fn check_arity(op: &ExprOperation) -> Result<(), String> {
    let n = op.arguments.len();
    let ok = match op.op {
        ExprOp::Sub | ExprOp::Div => n == 2,
        ExprOp::Neg
        | ExprOp::Abs
        | ExprOp::Not
        | ExprOp::IsDefined
        | ExprOp::IsEmpty
        | ExprOp::IsNotEmpty => n == 1,
        ExprOp::Add | ExprOp::Mul | ExprOp::Coalesce | ExprOp::And | ExprOp::Or => n >= 1,
        ExprOp::Eq | ExprOp::Ne | ExprOp::Gt | ExprOp::Gte | ExprOp::Lt | ExprOp::Lte => n == 2,
    };
    if !ok {
        return Err(format!(
            "{}: wrong number of arguments ({})",
            op.op.name(),
            n
        ));
    }
    Ok(())
}

fn is_numeric_compatible(k: AliasKind) -> bool {
    matches!(k, AliasKind::Numeric | AliasKind::Nullable)
}

fn coalesce_kind(op: ExprOp, kinds: &[AliasKind]) -> Result<AliasKind, String> {
    let mut result: Option<AliasKind> = None;
    for (i, k) in kinds.iter().enumerate() {
        if *k == AliasKind::Nullable {
            continue;
        }
        match result {
            None => result = Some(*k),
            Some(existing) if existing == *k => {}
            Some(existing) => {
                return Err(format!(
                    "{}: arguments must share a kind, got {:?} at position 0 and {:?} at position {}",
                    op.name(),
                    existing,
                    k,
                    i
                ));
            }
        }
    }
    Ok(result.unwrap_or(AliasKind::Nullable))
}

fn check_comparable(op: ExprOp, a: AliasKind, b: AliasKind) -> Result<(), String> {
    if a == AliasKind::Nullable || b == AliasKind::Nullable {
        return Ok(());
    }
    if a == AliasKind::Json || b == AliasKind::Json {
        return Err(format!(
            "{}: JSON values cannot be compared with {}",
            op.name(),
            op.name()
        ));
    }
    if a != b {
        return Err(format!(
            "{}: cannot compare {:?} with {:?}",
            op.name(),
            a,
            b
        ));
    }
    Ok(())
}

// ============================================================================
// SQL rendering
// ============================================================================

/// Map from alias name → (precompiled SQL fragment, inferred kind).
pub(crate) type AliasSqlMap = HashMap<String, (String, AliasKind)>;

/// Compile an expression tree into a standalone SQL fragment.
///
/// Alias operands are substituted with the already-compiled SQL for the
/// referenced alias. PostgreSQL can't reference sibling SELECT-list aliases
/// in the SELECT list itself, so inline substitution keeps the whole
/// aggregate query in a single grouped SELECT. Depth is bounded by
/// [`EXPR_MAX_DEPTH`] so the resulting SQL can't blow up.
pub(crate) fn render_expression(
    node: &ExprNode,
    alias_sql: &AliasSqlMap,
    depth: u8,
) -> Result<String, String> {
    if depth > EXPR_MAX_DEPTH {
        return Err(format!(
            "expression tree exceeds max depth ({})",
            EXPR_MAX_DEPTH
        ));
    }
    match node {
        ExprNode::Value(ExprValue::Reference { .. }) | ExprNode::Fn(_) => {
            unreachable!("references / fn calls are rejected during EXPR validation")
        }
        ExprNode::Value(ExprValue::Alias { value }) => alias_sql
            .get(value)
            .map(|(sql, _)| format!("({})", sql))
            .ok_or_else(|| format!("alias '{}' has no compiled SQL", value)),
        ExprNode::Value(ExprValue::Immediate { value }) => render_literal(value),
        ExprNode::Operation(op) => {
            let children: Vec<String> = op
                .arguments
                .iter()
                .map(|a| render_expression(a, alias_sql, depth + 1))
                .collect::<Result<_, _>>()?;
            Ok(render_op(op.op, &children))
        }
    }
}

fn render_op(op: ExprOp, children: &[String]) -> String {
    // Cast each arithmetic operand to numeric to avoid PostgreSQL's
    // integer-division surprise (bigint / bigint truncates toward zero).
    // This also promotes integer literals to numeric before arithmetic.
    let nc: Vec<String> = children
        .iter()
        .map(|c| format!("({})::numeric", c))
        .collect();
    match op {
        ExprOp::Add => format!("({})", nc.join(" + ")),
        ExprOp::Sub => format!("({} - {})", nc[0], nc[1]),
        ExprOp::Mul => format!("({})", nc.join(" * ")),
        ExprOp::Div => format!("({} / NULLIF({}, 0))", nc[0], nc[1]),
        ExprOp::Neg => format!("(-{})", nc[0]),
        ExprOp::Abs => format!("ABS({})", nc[0]),
        ExprOp::Coalesce => format!("COALESCE({})", children.join(", ")),
        ExprOp::Eq => format!("(({}) = ({}))", children[0], children[1]),
        ExprOp::Ne => format!("(({}) <> ({}))", children[0], children[1]),
        ExprOp::Gt => format!("(({}) > ({}))", children[0], children[1]),
        ExprOp::Gte => format!("(({}) >= ({}))", children[0], children[1]),
        ExprOp::Lt => format!("(({}) < ({}))", children[0], children[1]),
        ExprOp::Lte => format!("(({}) <= ({}))", children[0], children[1]),
        ExprOp::And => format!("({})", children.join(" AND ")),
        ExprOp::Or => format!("({})", children.join(" OR ")),
        ExprOp::Not => format!("(NOT ({}))", children[0]),
        ExprOp::IsDefined => format!("(({}) IS NOT NULL)", children[0]),
        ExprOp::IsEmpty => format!("(({}) IS NULL)", children[0]),
        ExprOp::IsNotEmpty => format!("(({}) IS NOT NULL)", children[0]),
    }
}

fn render_literal(v: &serde_json::Value) -> Result<String, String> {
    Ok(match v {
        serde_json::Value::Null => "NULL".to_string(),
        serde_json::Value::Bool(true) => "TRUE".to_string(),
        serde_json::Value::Bool(false) => "FALSE".to_string(),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::String(s) => format!("'{}'", s.replace('\'', "''")),
        serde_json::Value::Array(_) | serde_json::Value::Object(_) => {
            return Err(
                "composite literals (arrays / objects) are not allowed in EXPR immediates"
                    .to_string(),
            );
        }
    })
}

// ============================================================================
// Row-level expression validation + rendering (for `score_expression`)
// ============================================================================

/// Validate a row-level expression tree.
///
/// Inverse policy from [`validate_expression`]: column references are accepted
/// (validated against `schema`) and alias references are rejected, since
/// row-level scoring runs in the same SELECT as the columns it computes from
/// and has no aggregate aliases to substitute.
pub(crate) fn validate_row_expression(
    node: &ExprNode,
    schema: &Schema,
    depth: u8,
) -> Result<AliasKind, String> {
    if depth > EXPR_MAX_DEPTH {
        return Err(format!(
            "expression tree exceeds max depth ({})",
            EXPR_MAX_DEPTH
        ));
    }
    match node {
        ExprNode::Value(ExprValue::Alias { value }) => Err(format!(
            "alias reference '{}' is not allowed inside a row-level \
             score expression; reference a column instead",
            value
        )),
        ExprNode::Value(ExprValue::Reference { value }) => {
            let exists = matches!(value.as_str(), "id" | "created_at" | "updated_at")
                || schema.columns.iter().any(|c| c.name == *value);
            if !exists {
                return Err(format!(
                    "column '{}' is not declared on schema '{}'",
                    value, schema.name
                ));
            }
            Ok(column_kind(value, schema))
        }
        ExprNode::Value(ExprValue::Immediate { value }) => literal_kind(value),
        ExprNode::Operation(op) => {
            // Reuse the aggregate validator's operator semantics by mapping
            // schema column references through `column_kind`. We construct an
            // ad-hoc `prior` list of "alias-y" entries on the fly so the
            // existing `validate_operation` does the arity / kind checks.
            // For simplicity we recurse manually here.
            check_arity(op)?;
            let kinds: Vec<AliasKind> = op
                .arguments
                .iter()
                .map(|a| validate_row_expression(a, schema, depth + 1))
                .collect::<Result<_, _>>()?;
            row_op_result_kind(op.op, &kinds)
        }
        ExprNode::Fn(call) => validate_fn_call(call, schema, depth),
    }
}

fn validate_fn_call(call: &ExprFnCall, schema: &Schema, depth: u8) -> Result<AliasKind, String> {
    // TS_RANK does its own column-type check on the first argument before
    // recursive validation (we need to *allow* a tsvector column reference
    // even though `validate_row_expression`'s default ColumnType::Tsvector
    // mapping is `AliasKind::Json`).
    if matches!(call.fn_, ExprFn::TsRank) {
        if call.arguments.len() != 2 {
            return Err(format!(
                "{}: expected 2 arguments, got {}",
                call.fn_.name(),
                call.arguments.len()
            ));
        }
        // First arg must be a column reference pointing to a tsvector column.
        let col_name = match &call.arguments[0] {
            ExprNode::Value(ExprValue::Reference { value }) => value.clone(),
            _ => {
                return Err(format!(
                    "{}: first argument must reference a tsvector column",
                    call.fn_.name()
                ));
            }
        };
        if !is_tsvector_column(&col_name, schema) {
            return Err(format!(
                "{}: column '{}' is not declared as a tsvector",
                call.fn_.name(),
                col_name
            ));
        }
        // Second arg must be a text immediate (the query string).
        let q_kind = validate_row_expression(&call.arguments[1], schema, depth + 1)?;
        if !matches!(q_kind, AliasKind::Text | AliasKind::Nullable) {
            return Err(format!(
                "{}: query argument must be text, got {:?}",
                call.fn_.name(),
                q_kind
            ));
        }
        return Ok(AliasKind::Numeric);
    }

    let kinds: Vec<AliasKind> = call
        .arguments
        .iter()
        .map(|a| validate_row_expression(a, schema, depth + 1))
        .collect::<Result<_, _>>()?;
    match call.fn_ {
        ExprFn::Similarity => {
            if kinds.len() != 2 {
                return Err(format!(
                    "{}: expected 2 arguments, got {}",
                    call.fn_.name(),
                    kinds.len()
                ));
            }
            for (i, k) in kinds.iter().enumerate() {
                if !matches!(k, AliasKind::Text | AliasKind::Nullable) {
                    return Err(format!(
                        "{}: argument {} must be text, got {:?}",
                        call.fn_.name(),
                        i,
                        k
                    ));
                }
            }
            Ok(AliasKind::Numeric)
        }
        ExprFn::Greatest | ExprFn::Least => {
            if kinds.is_empty() {
                return Err(format!("{}: requires at least 1 argument", call.fn_.name()));
            }
            for (i, k) in kinds.iter().enumerate() {
                if !matches!(k, AliasKind::Numeric | AliasKind::Nullable) {
                    return Err(format!(
                        "{}: argument {} must be numeric, got {:?}",
                        call.fn_.name(),
                        i,
                        k
                    ));
                }
            }
            Ok(AliasKind::Numeric)
        }
        ExprFn::TsRank => unreachable!("handled at top of function"),
    }
}

fn row_op_result_kind(op: ExprOp, kinds: &[AliasKind]) -> Result<AliasKind, String> {
    match op {
        ExprOp::Add | ExprOp::Sub | ExprOp::Mul | ExprOp::Div | ExprOp::Neg | ExprOp::Abs => {
            for (i, k) in kinds.iter().enumerate() {
                if !is_numeric_compatible(*k) {
                    return Err(format!(
                        "{}: argument {} must be numeric, got {:?}",
                        op.name(),
                        i,
                        k
                    ));
                }
            }
            Ok(AliasKind::Numeric)
        }
        ExprOp::Coalesce => coalesce_kind(op, kinds),
        ExprOp::Eq | ExprOp::Ne | ExprOp::Gt | ExprOp::Gte | ExprOp::Lt | ExprOp::Lte => {
            check_comparable(op, kinds[0], kinds[1])?;
            Ok(AliasKind::Bool)
        }
        ExprOp::And | ExprOp::Or => {
            for (i, k) in kinds.iter().enumerate() {
                if !matches!(k, AliasKind::Bool | AliasKind::Nullable) {
                    return Err(format!(
                        "{}: argument {} must be boolean, got {:?}",
                        op.name(),
                        i,
                        k
                    ));
                }
            }
            Ok(AliasKind::Bool)
        }
        ExprOp::Not => {
            if !matches!(kinds[0], AliasKind::Bool | AliasKind::Nullable) {
                return Err(format!("NOT: argument must be boolean, got {:?}", kinds[0]));
            }
            Ok(AliasKind::Bool)
        }
        ExprOp::IsDefined | ExprOp::IsEmpty | ExprOp::IsNotEmpty => Ok(AliasKind::Bool),
    }
}

/// Render a row-level expression tree to SQL, threading `params` and
/// `param_offset` so string immediates bind positionally rather than being
/// inlined.
///
/// Validation must have already passed. Columns are emitted as quoted
/// identifiers (`"name"`). String immediates are appended to `params` and
/// emitted as `$N::text`. Numeric / bool / null immediates inline. Function
/// calls expand to their SQL builtins.
// `schema` is currently only consulted via the recursion path so it can grow
// schema-aware behaviour later (e.g. type-driven casts) without a signature
// churn.
#[allow(clippy::only_used_in_recursion)]
pub(crate) fn render_row_expression(
    node: &ExprNode,
    schema: &Schema,
    params: &mut Vec<serde_json::Value>,
    param_offset: &mut i32,
    depth: u8,
) -> Result<String, String> {
    if depth > EXPR_MAX_DEPTH {
        return Err(format!(
            "expression tree exceeds max depth ({})",
            EXPR_MAX_DEPTH
        ));
    }
    match node {
        ExprNode::Value(ExprValue::Alias { .. }) => {
            unreachable!("aliases are rejected during row-expression validation")
        }
        ExprNode::Value(ExprValue::Reference { value }) => {
            // Validation already confirmed the column exists; emit a quoted
            // identifier with no cast so callers (e.g. `similarity`) can apply
            // their own.
            Ok(format!("\"{}\"", value))
        }
        ExprNode::Value(ExprValue::Immediate { value }) => {
            render_row_immediate(value, params, param_offset)
        }
        ExprNode::Operation(op) => {
            let children: Vec<String> = op
                .arguments
                .iter()
                .map(|a| render_row_expression(a, schema, params, param_offset, depth + 1))
                .collect::<Result<_, _>>()?;
            Ok(render_op(op.op, &children))
        }
        ExprNode::Fn(call) => {
            // TS_RANK has bespoke rendering — it needs the tsvector column's
            // declared language (looked up on the schema) and binds the
            // query through `plainto_tsquery('<lang>', $N)` rather than as
            // a plain text immediate.
            if matches!(call.fn_, ExprFn::TsRank) {
                let col_name = match &call.arguments[0] {
                    ExprNode::Value(ExprValue::Reference { value }) => value.clone(),
                    _ => unreachable!("validated above"),
                };
                let language = schema
                    .columns
                    .iter()
                    .find(|c| c.name == col_name)
                    .and_then(|c| match &c.column_type {
                        ColumnType::Tsvector { language, .. } => Some(language.clone()),
                        _ => None,
                    })
                    .unwrap_or_else(|| "english".to_string());
                let query_sql = render_row_expression(
                    &call.arguments[1],
                    schema,
                    params,
                    param_offset,
                    depth + 1,
                )?;
                let lang_lit = language.replace('\'', "''");
                return Ok(format!(
                    "ts_rank(\"{}\", plainto_tsquery('{}', {}))",
                    col_name, lang_lit, query_sql
                ));
            }

            let children: Vec<String> = call
                .arguments
                .iter()
                .map(|a| render_row_expression(a, schema, params, param_offset, depth + 1))
                .collect::<Result<_, _>>()?;
            Ok(render_fn(call.fn_, &children))
        }
    }
}

fn render_row_immediate(
    value: &serde_json::Value,
    params: &mut Vec<serde_json::Value>,
    param_offset: &mut i32,
) -> Result<String, String> {
    match value {
        serde_json::Value::String(_) => {
            let placeholder = format!("${}::text", *param_offset);
            params.push(value.clone());
            *param_offset += 1;
            Ok(placeholder)
        }
        serde_json::Value::Null => Ok("NULL".to_string()),
        serde_json::Value::Bool(true) => Ok("TRUE".to_string()),
        serde_json::Value::Bool(false) => Ok("FALSE".to_string()),
        serde_json::Value::Number(n) => Ok(n.to_string()),
        serde_json::Value::Array(_) | serde_json::Value::Object(_) => Err(
            "composite literals (arrays / objects) are not allowed in score expressions"
                .to_string(),
        ),
    }
}

fn render_fn(f: ExprFn, children: &[String]) -> String {
    match f {
        ExprFn::Similarity => {
            // Both args coerced to text so column refs work without an
            // explicit cast in the wire payload.
            format!(
                "similarity(({})::text, ({})::text)",
                children[0], children[1]
            )
        }
        ExprFn::Greatest => format!("GREATEST({})", children.join(", ")),
        ExprFn::Least => format!("LEAST({})", children.join(", ")),
        // Handled inline by `render_row_expression`; this arm is unreachable.
        ExprFn::TsRank => unreachable!("TS_RANK is rendered inline in render_row_expression"),
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn num_alias(name: &str) -> ExprNode {
        ExprNode::Value(ExprValue::Alias {
            value: name.to_string(),
        })
    }

    fn imm(v: serde_json::Value) -> ExprNode {
        ExprNode::Value(ExprValue::Immediate { value: v })
    }

    fn field(name: &str) -> ExprNode {
        ExprNode::Value(ExprValue::Reference {
            value: name.to_string(),
        })
    }

    fn op(op: ExprOp, args: Vec<ExprNode>) -> ExprNode {
        ExprNode::Operation(ExprOperation {
            op,
            arguments: args,
        })
    }

    fn prior_numeric(names: &[&str]) -> Vec<(String, AliasKind)> {
        names
            .iter()
            .map(|n| (n.to_string(), AliasKind::Numeric))
            .collect()
    }

    fn alias_sql_map(entries: &[(&str, &str, AliasKind)]) -> AliasSqlMap {
        entries
            .iter()
            .map(|(name, sql, kind)| (name.to_string(), (sql.to_string(), *kind)))
            .collect()
    }

    #[test]
    fn alias_unknown_rejected() {
        let prior = prior_numeric(&["a"]);
        let node = num_alias("b");
        let err = validate_expression(&node, &prior, 0).unwrap_err();
        assert!(err.contains("is not declared before its use"), "{}", err);
    }

    #[test]
    fn alias_resolves_to_prior_kind() {
        let prior = prior_numeric(&["a"]);
        let node = num_alias("a");
        assert_eq!(
            validate_expression(&node, &prior, 0).unwrap(),
            AliasKind::Numeric
        );
    }

    #[test]
    fn field_reference_rejected() {
        let node = field("qty");
        let err = validate_expression(&node, &[], 0).unwrap_err();
        assert!(err.contains("field reference 'qty'"), "{}", err);
        assert!(err.contains("not allowed inside EXPR"), "{}", err);
    }

    #[test]
    fn depth_cap_enforced_validate() {
        // Build a right-deep ADD tree of depth 10.
        let mut node = imm(serde_json::json!(1));
        for _ in 0..10 {
            node = op(ExprOp::Add, vec![imm(serde_json::json!(1)), node]);
        }
        let err = validate_expression(&node, &[], 0).unwrap_err();
        assert!(err.contains("max depth"), "{}", err);
    }

    #[test]
    fn arithmetic_on_text_alias_rejected() {
        let prior = vec![("s".to_string(), AliasKind::Text)];
        let node = op(ExprOp::Sub, vec![num_alias("s"), imm(serde_json::json!(1))]);
        let err = validate_expression(&node, &prior, 0).unwrap_err();
        assert!(err.contains("must be numeric"), "{}", err);
    }

    #[test]
    fn coalesce_numeric_agreement() {
        let prior = prior_numeric(&["a", "b"]);
        let node = op(
            ExprOp::Coalesce,
            vec![num_alias("a"), num_alias("b"), imm(serde_json::json!(0))],
        );
        assert_eq!(
            validate_expression(&node, &prior, 0).unwrap(),
            AliasKind::Numeric
        );
    }

    #[test]
    fn coalesce_mixed_kind_rejected() {
        let prior = vec![
            ("a".to_string(), AliasKind::Numeric),
            ("b".to_string(), AliasKind::Text),
        ];
        let node = op(ExprOp::Coalesce, vec![num_alias("a"), num_alias("b")]);
        let err = validate_expression(&node, &prior, 0).unwrap_err();
        assert!(err.contains("must share a kind"), "{}", err);
    }

    #[test]
    fn coalesce_with_null_literal() {
        let prior = prior_numeric(&["a"]);
        let node = op(
            ExprOp::Coalesce,
            vec![imm(serde_json::Value::Null), num_alias("a")],
        );
        assert_eq!(
            validate_expression(&node, &prior, 0).unwrap(),
            AliasKind::Numeric
        );
    }

    #[test]
    fn comparison_returns_bool() {
        let prior = prior_numeric(&["a"]);
        let node = op(ExprOp::Gt, vec![num_alias("a"), imm(serde_json::json!(10))]);
        assert_eq!(
            validate_expression(&node, &prior, 0).unwrap(),
            AliasKind::Bool
        );
    }

    #[test]
    fn comparison_rejects_json() {
        let prior = vec![("j".to_string(), AliasKind::Json)];
        let node = op(
            ExprOp::Eq,
            vec![num_alias("j"), imm(serde_json::json!("foo"))],
        );
        let err = validate_expression(&node, &prior, 0).unwrap_err();
        assert!(err.contains("JSON values cannot be compared"), "{}", err);
    }

    #[test]
    fn logical_and_requires_bool() {
        let prior = prior_numeric(&["a"]);
        let node = op(ExprOp::And, vec![num_alias("a")]);
        let err = validate_expression(&node, &prior, 0).unwrap_err();
        assert!(err.contains("must be boolean"), "{}", err);
    }

    #[test]
    fn composite_literal_rejected() {
        let node = imm(serde_json::json!([1, 2, 3]));
        let err = validate_expression(&node, &[], 0).unwrap_err();
        assert!(err.contains("composite literals"), "{}", err);
    }

    #[test]
    fn arity_sub_requires_two() {
        let node = op(ExprOp::Sub, vec![imm(serde_json::json!(1))]);
        let err = validate_expression(&node, &[], 0).unwrap_err();
        assert!(err.contains("SUB"), "{}", err);
    }

    #[test]
    fn arity_add_requires_one_or_more() {
        let node = op(ExprOp::Add, vec![]);
        let err = validate_expression(&node, &[], 0).unwrap_err();
        assert!(err.contains("ADD"), "{}", err);
    }

    #[test]
    fn render_sub_inlines_alias_sql() {
        let amap = alias_sql_map(&[
            ("a", "SUM(x)::numeric", AliasKind::Numeric),
            ("b", "SUM(y)::numeric", AliasKind::Numeric),
        ]);
        let node = op(ExprOp::Sub, vec![num_alias("a"), num_alias("b")]);
        let sql = render_expression(&node, &amap, 0).unwrap();
        assert!(sql.contains("SUM(x)::numeric"), "{}", sql);
        assert!(sql.contains("SUM(y)::numeric"), "{}", sql);
        assert!(sql.contains(" - "), "{}", sql);
    }

    #[test]
    fn render_div_uses_nullif() {
        let amap = alias_sql_map(&[
            ("a", "SUM(x)::numeric", AliasKind::Numeric),
            ("b", "SUM(y)::numeric", AliasKind::Numeric),
        ]);
        let node = op(ExprOp::Div, vec![num_alias("a"), num_alias("b")]);
        let sql = render_expression(&node, &amap, 0).unwrap();
        assert!(sql.contains("NULLIF"), "{}", sql);
    }

    #[test]
    fn render_string_literal_escapes() {
        let node = imm(serde_json::json!("it's"));
        let sql = render_expression(&node, &HashMap::new(), 0).unwrap();
        assert_eq!(sql, "'it''s'");
    }

    #[test]
    fn render_numeric_literal() {
        let node = imm(serde_json::json!(42));
        assert_eq!(render_expression(&node, &HashMap::new(), 0).unwrap(), "42");
    }

    #[test]
    fn render_bool_literal() {
        assert_eq!(
            render_expression(&imm(serde_json::json!(true)), &HashMap::new(), 0).unwrap(),
            "TRUE"
        );
        assert_eq!(
            render_expression(&imm(serde_json::json!(false)), &HashMap::new(), 0).unwrap(),
            "FALSE"
        );
    }

    #[test]
    fn render_null_literal() {
        assert_eq!(
            render_expression(&imm(serde_json::Value::Null), &HashMap::new(), 0).unwrap(),
            "NULL"
        );
    }

    #[test]
    fn render_depth_cap_enforced() {
        // Even though we'd build from Rust, the render path double-checks.
        let mut node = imm(serde_json::json!(1));
        for _ in 0..10 {
            node = op(ExprOp::Add, vec![imm(serde_json::json!(1)), node]);
        }
        let err = render_expression(&node, &HashMap::new(), 0).unwrap_err();
        assert!(err.contains("max depth"), "{}", err);
    }

    #[test]
    fn serde_roundtrip_alias_operand() {
        let json = serde_json::json!({"valueType": "alias", "value": "first_qty"});
        let node: ExprNode = serde_json::from_value(json).unwrap();
        match node {
            ExprNode::Value(ExprValue::Alias { value }) => assert_eq!(value, "first_qty"),
            _ => panic!("expected alias"),
        }
    }

    #[test]
    fn serde_roundtrip_immediate_operand() {
        let json = serde_json::json!({"valueType": "immediate", "value": 42});
        let node: ExprNode = serde_json::from_value(json).unwrap();
        match node {
            ExprNode::Value(ExprValue::Immediate { value }) => {
                assert_eq!(value, serde_json::json!(42))
            }
            _ => panic!("expected immediate"),
        }
    }

    #[test]
    fn serde_roundtrip_operation() {
        let json = serde_json::json!({
            "op": "SUB",
            "arguments": [
                {"valueType": "alias", "value": "a"},
                {"valueType": "alias", "value": "b"}
            ]
        });
        let node: ExprNode = serde_json::from_value(json).unwrap();
        match node {
            ExprNode::Operation(op) => {
                assert_eq!(op.op, ExprOp::Sub);
                assert_eq!(op.arguments.len(), 2);
            }
            _ => panic!("expected operation"),
        }
    }

    // =========================================================================
    // Row-level expression tests
    // =========================================================================

    use crate::types::{ColumnDefinition, ColumnType};

    fn product_schema() -> Schema {
        Schema::new(
            "test",
            "Product",
            "product",
            vec![
                ColumnDefinition::new("name", ColumnType::String),
                ColumnDefinition::new("keywords", ColumnType::String).with_trigram_index(),
                ColumnDefinition::new("price", ColumnType::Integer),
                ColumnDefinition::new(
                    "keywords_tsv",
                    ColumnType::Tsvector {
                        source_column: "keywords".to_string(),
                        language: "english".to_string(),
                    },
                )
                .not_null(),
            ],
        )
    }

    fn col_ref(name: &str) -> ExprNode {
        ExprNode::Value(ExprValue::Reference {
            value: name.to_string(),
        })
    }

    fn fn_call(f: ExprFn, args: Vec<ExprNode>) -> ExprNode {
        ExprNode::Fn(ExprFnCall {
            fn_: f,
            arguments: args,
        })
    }

    #[test]
    fn row_expr_accepts_column_ref() {
        let schema = product_schema();
        let node = col_ref("name");
        assert_eq!(
            validate_row_expression(&node, &schema, 0).unwrap(),
            AliasKind::Text
        );
    }

    #[test]
    fn row_expr_rejects_alias() {
        let schema = product_schema();
        let node = num_alias("score");
        let err = validate_row_expression(&node, &schema, 0).unwrap_err();
        assert!(err.contains("alias reference"), "{}", err);
    }

    #[test]
    fn row_expr_rejects_unknown_column() {
        let schema = product_schema();
        let node = col_ref("nope");
        let err = validate_row_expression(&node, &schema, 0).unwrap_err();
        assert!(err.contains("not declared"), "{}", err);
    }

    #[test]
    fn row_expr_similarity_two_text_args() {
        let schema = product_schema();
        let node = fn_call(
            ExprFn::Similarity,
            vec![col_ref("keywords"), imm(serde_json::json!("blue jacket"))],
        );
        assert_eq!(
            validate_row_expression(&node, &schema, 0).unwrap(),
            AliasKind::Numeric
        );
    }

    #[test]
    fn row_expr_similarity_rejects_non_text() {
        let schema = product_schema();
        let node = fn_call(
            ExprFn::Similarity,
            vec![col_ref("price"), imm(serde_json::json!("blue"))],
        );
        let err = validate_row_expression(&node, &schema, 0).unwrap_err();
        assert!(err.contains("must be text"), "{}", err);
    }

    #[test]
    fn row_expr_similarity_arity() {
        let schema = product_schema();
        let node = fn_call(ExprFn::Similarity, vec![col_ref("keywords")]);
        let err = validate_row_expression(&node, &schema, 0).unwrap_err();
        assert!(err.contains("expected 2 arguments"), "{}", err);
    }

    #[test]
    fn row_expr_greatest_numeric() {
        let schema = product_schema();
        let node = fn_call(
            ExprFn::Greatest,
            vec![
                fn_call(
                    ExprFn::Similarity,
                    vec![col_ref("name"), imm(serde_json::json!("foo"))],
                ),
                fn_call(
                    ExprFn::Similarity,
                    vec![col_ref("keywords"), imm(serde_json::json!("foo"))],
                ),
            ],
        );
        assert_eq!(
            validate_row_expression(&node, &schema, 0).unwrap(),
            AliasKind::Numeric
        );
    }

    #[test]
    fn row_expr_render_similarity_binds_param() {
        let schema = product_schema();
        let node = fn_call(
            ExprFn::Similarity,
            vec![col_ref("keywords"), imm(serde_json::json!("blue jacket"))],
        );
        let mut params = vec![];
        let mut offset = 1_i32;
        let sql = render_row_expression(&node, &schema, &mut params, &mut offset, 0).unwrap();
        assert_eq!(sql, r#"similarity(("keywords")::text, ($1::text)::text)"#);
        assert_eq!(params, vec![serde_json::json!("blue jacket")]);
        assert_eq!(offset, 2);
    }

    #[test]
    fn row_expr_render_greatest_two_similarities() {
        let schema = product_schema();
        let node = fn_call(
            ExprFn::Greatest,
            vec![
                fn_call(
                    ExprFn::Similarity,
                    vec![col_ref("name"), imm(serde_json::json!("blue"))],
                ),
                fn_call(
                    ExprFn::Similarity,
                    vec![col_ref("keywords"), imm(serde_json::json!("blue"))],
                ),
            ],
        );
        let mut params = vec![];
        let mut offset = 5_i32;
        let sql = render_row_expression(&node, &schema, &mut params, &mut offset, 0).unwrap();
        assert!(sql.starts_with("GREATEST("), "{}", sql);
        assert!(sql.contains(r#"("name")::text"#), "{}", sql);
        assert!(sql.contains(r#"("keywords")::text"#), "{}", sql);
        assert_eq!(params.len(), 2);
        assert_eq!(offset, 7);
    }

    #[test]
    fn row_expr_serde_roundtrip_fn_call() {
        let json = serde_json::json!({
            "fn": "SIMILARITY",
            "arguments": [
                {"valueType": "reference", "value": "keywords"},
                {"valueType": "immediate", "value": "blue"}
            ]
        });
        let node: ExprNode = serde_json::from_value(json).unwrap();
        match node {
            ExprNode::Fn(call) => {
                assert_eq!(call.fn_, ExprFn::Similarity);
                assert_eq!(call.arguments.len(), 2);
            }
            _ => panic!("expected fn call"),
        }
    }

    #[test]
    fn aggregate_validate_rejects_fn() {
        let prior = prior_numeric(&["a"]);
        let node = fn_call(ExprFn::Similarity, vec![imm(serde_json::json!("x"))]);
        let err = validate_expression(&node, &prior, 0).unwrap_err();
        assert!(err.contains("function call"), "{}", err);
    }

    // ===== TS_RANK =====

    #[test]
    fn ts_rank_accepts_tsvector_column_and_text_query() {
        let schema = product_schema();
        let node = fn_call(
            ExprFn::TsRank,
            vec![
                col_ref("keywords_tsv"),
                imm(serde_json::json!("blue jacket")),
            ],
        );
        assert_eq!(
            validate_row_expression(&node, &schema, 0).unwrap(),
            AliasKind::Numeric
        );
    }

    #[test]
    fn ts_rank_rejects_non_tsvector_column() {
        let schema = product_schema();
        let node = fn_call(
            ExprFn::TsRank,
            vec![col_ref("keywords"), imm(serde_json::json!("blue"))],
        );
        let err = validate_row_expression(&node, &schema, 0).unwrap_err();
        assert!(err.contains("not declared as a tsvector"), "{}", err);
    }

    #[test]
    fn ts_rank_rejects_non_reference_first_arg() {
        let schema = product_schema();
        let node = fn_call(
            ExprFn::TsRank,
            vec![imm(serde_json::json!("col")), imm(serde_json::json!("q"))],
        );
        let err = validate_row_expression(&node, &schema, 0).unwrap_err();
        assert!(err.contains("must reference a tsvector column"), "{}", err);
    }

    #[test]
    fn ts_rank_arity() {
        let schema = product_schema();
        let node = fn_call(ExprFn::TsRank, vec![col_ref("keywords_tsv")]);
        let err = validate_row_expression(&node, &schema, 0).unwrap_err();
        assert!(err.contains("expected 2 arguments"), "{}", err);
    }

    #[test]
    fn ts_rank_renders_with_column_language() {
        let schema = product_schema();
        let node = fn_call(
            ExprFn::TsRank,
            vec![
                col_ref("keywords_tsv"),
                imm(serde_json::json!("blue jacket")),
            ],
        );
        let mut params = vec![];
        let mut offset = 1_i32;
        let sql = render_row_expression(&node, &schema, &mut params, &mut offset, 0).unwrap();
        assert_eq!(
            sql,
            r#"ts_rank("keywords_tsv", plainto_tsquery('english', $1::text))"#
        );
        assert_eq!(params, vec![serde_json::json!("blue jacket")]);
        assert_eq!(offset, 2);
    }
}
