// Copyright (C) 2026 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Queries over one workflow's runs (`docs/workflow-records.md`): a
//! workflow is its own store, each run label a record, and its state the
//! record's data. A query filters, groups and aggregates runs by their
//! columns and by paths into their state, nested objects and arrays
//! included, and Postgres answers it where the runs are.
//!
//! A query takes no SQL. Paths, operators and aggregates are named from
//! short lists, every value and path is bound, and the statement around
//! them is this module's own. It reads one tenant's runs of one workflow,
//! in the caller's scope, within [`STATEMENT_TIMEOUT`] and the limits
//! below.
//!
//! # Paths
//!
//! A run's columns are `runId`, `label`, `status`, `version`, `createdAt`,
//! `startedAt`, `finishedAt` and `updatedAt` (when its state was last
//! written). `state` is its whole state, and `state.a.b` reaches into it, a
//! number segment indexing an array (`state.items.0.sku`). In a condition,
//! one segment may end in `[*]` (`state.items[*].sku`): the condition holds
//! if it holds for any element. With [`RunQuery::unnest`] each element of
//! that array is a row of its own, read as `item` and `item.a.b`.

use chrono::{DateTime, Utc};
use futures_util::TryStreamExt;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{Postgres, QueryBuilder, Row};

use crate::instance_repository::InstanceRepository;

/// Rows a query returns unless it asks for another number.
pub const DEFAULT_LIMIT: u32 = 100;
/// Most rows a query returns.
pub const MAX_LIMIT: u32 = 1_000;
/// Furthest a query pages.
pub const MAX_OFFSET: u32 = 100_000;
/// Most bytes of rows (as JSON text) a query returns; past them it stops,
/// and says there is more.
pub const MAX_RESULT_BYTES: usize = 8 * 1024 * 1024;
/// Most operators in one condition.
pub const MAX_CONDITION_NODES: usize = 256;
/// Deepest a condition nests.
pub const MAX_CONDITION_DEPTH: usize = 16;
/// Most segments in a path.
pub const MAX_PATH_SEGMENTS: usize = 16;
/// Longest segment of a path.
pub const MAX_SEGMENT_BYTES: usize = 256;
/// Most values `IN` and `NOT_IN` take.
pub const MAX_IN_VALUES: usize = 1_000;
/// Most values across all `IN` and `NOT_IN` operators in one query, keeping
/// query memory and bind parameters well below PostgreSQL's 65,535 limit.
pub const MAX_TOTAL_IN_VALUES: usize = 4_000;
/// Most `IN` values expanded into `OR`-ed GIN containment predicates; larger
/// lists still execute via `= ANY(ARRAY(...))` with a single bound parameter.
const MAX_CONTAINMENT_IN_VALUES: usize = 32;
/// Most paths a query selects.
pub const MAX_SELECT: usize = 64;
/// Most paths a query groups by.
pub const MAX_GROUP_BY: usize = 16;
/// Most aggregates a query computes.
pub const MAX_AGGREGATES: usize = 32;
/// Most paths a query orders by.
pub const MAX_ORDER_BY: usize = 8;
/// How long Postgres may take over one query.
pub const STATEMENT_TIMEOUT: &str = "30s";

/// A query over one workflow's runs.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunQuery {
    /// One row per run label: its latest run (the one whose state was
    /// written last, then the newest). Runs without a label are left out.
    /// The condition applies to that run, after it is chosen.
    #[serde(default)]
    pub latest_per_label: bool,
    /// Which runs (or elements, with `unnest`) to keep.
    #[serde(default)]
    pub condition: Option<Condition>,
    /// A path to an array in state: each of its elements becomes a row,
    /// read as `item`. A run whose path holds no array has no rows.
    #[serde(default)]
    pub unnest: Option<String>,
    /// What each row returns; by default the run's columns and its state
    /// (and `item`). Not with `groupBy` or `aggregates`.
    #[serde(default)]
    pub select: Vec<String>,
    /// Return one row per distinct value of these paths, with
    /// `aggregates`.
    #[serde(default)]
    pub group_by: Vec<String>,
    /// What to compute over each group, or over every row kept.
    #[serde(default)]
    pub aggregates: Vec<Aggregate>,
    /// The order of the rows: paths, and with groups the names of
    /// aggregates.
    #[serde(default)]
    pub order_by: Vec<OrderBy>,
    /// Most rows to return; [`DEFAULT_LIMIT`] if not given, at most
    /// [`MAX_LIMIT`].
    #[serde(default)]
    pub limit: Option<u32>,
    /// Rows to skip.
    #[serde(default)]
    pub offset: Option<u32>,
    /// Also count every row (or group) the query keeps.
    #[serde(default)]
    pub count: bool,
}

/// A condition, as the object model takes them: `{"op": "AND",
/// "arguments": [conditions]}`, `{"op": "GTE", "arguments": ["state.score",
/// 9]}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Condition {
    /// `AND`, `OR`, `NOT`, `EQ`, `NE`, `GT`, `GTE`, `LT`, `LTE`, `IN`,
    /// `NOT_IN`, `CONTAINS`, `STARTS_WITH`, `ENDS_WITH`, `IS_DEFINED`,
    /// `IS_EMPTY` or `IS_NOT_EMPTY`.
    pub op: String,
    /// Conditions for `AND`, `OR` and `NOT`; a path, then a value, for the
    /// others (a list of values for `IN` and `NOT_IN`; none for the `IS_`
    /// ones).
    #[serde(default)]
    pub arguments: Vec<Value>,
}

/// What an aggregate computes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AggregateFn {
    /// Rows; with a path, rows where it holds a value.
    Count,
    /// Distinct values of a path.
    CountDistinct,
    /// The least value: of the numbers, or else of the strings.
    Min,
    /// The greatest value: of the numbers, or else of the strings.
    Max,
    /// The sum of the numbers.
    Sum,
    /// The mean of the numbers.
    Avg,
}

/// One aggregate.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Aggregate {
    /// What it computes.
    #[serde(rename = "fn")]
    pub function: AggregateFn,
    /// What of each row it computes over; none for a count of rows.
    #[serde(default)]
    pub path: Option<String>,
    /// Its name in the result; by default `fn(path)`.
    #[serde(default, rename = "as")]
    pub alias: Option<String>,
}

/// One key of an order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OrderBy {
    /// A path, or the name of an aggregate.
    pub path: String,
    /// Greatest first.
    #[serde(default)]
    pub desc: bool,
}

/// What a query returned.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunQueryResult {
    /// The names of the columns, in each row's order.
    pub columns: Vec<String>,
    /// The rows, each an array in `columns`' order.
    pub rows: Vec<Vec<Value>>,
    /// Rows (or groups) the query kept in all, if it asked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total: Option<u64>,
    /// More rows follow: ask again with a greater `offset`.
    pub has_more: bool,
}

/// A run's column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Column {
    RunId,
    Label,
    Status,
    Version,
    CreatedAt,
    StartedAt,
    FinishedAt,
    UpdatedAt,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Text,
    Integer,
    Time,
}

impl Column {
    fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "runId" => Self::RunId,
            "label" => Self::Label,
            "status" => Self::Status,
            "version" => Self::Version,
            "createdAt" => Self::CreatedAt,
            "startedAt" => Self::StartedAt,
            "finishedAt" => Self::FinishedAt,
            "updatedAt" => Self::UpdatedAt,
            _ => return None,
        })
    }

    fn sql(self) -> &'static str {
        match self {
            Self::RunId => "r.run_id",
            Self::Label => "r.label",
            Self::Status => "r.status",
            Self::Version => "r.version",
            Self::CreatedAt => "r.created_at",
            Self::StartedAt => "r.started_at",
            Self::FinishedAt => "r.finished_at",
            Self::UpdatedAt => "r.updated_at",
        }
    }

    fn kind(self) -> Kind {
        match self {
            Self::RunId | Self::Label | Self::Status => Kind::Text,
            Self::Version => Kind::Integer,
            _ => Kind::Time,
        }
    }
}

/// Where a path starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Root {
    State,
    Item,
}

impl Root {
    fn sql(self) -> &'static str {
        match self {
            Self::State => "r.state",
            Self::Item => "u.item",
        }
    }
}

/// A parsed path.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Target {
    Column(Column),
    /// `path` from `root`; with `rest`, `path` leads to an array, and the
    /// target is `rest` within any of its elements.
    Json {
        root: Root,
        path: Vec<String>,
        rest: Option<Vec<String>>,
    },
}

fn parse_target(text: &str, unnest: bool, wildcard: bool) -> Result<Target, String> {
    if let Some(column) = Column::parse(text) {
        return Ok(Target::Column(column));
    }
    let (root, tail) = match text.split_once('.') {
        Some((head, tail)) => (head, Some(tail)),
        None => (text, None),
    };
    let root = match root {
        "state" => Root::State,
        "item" if unnest => Root::Item,
        "item" => return Err("`item` is read only with `unnest`".into()),
        _ => {
            return Err(format!(
                "'{text}' is not a path: name a run column (runId, label, status, version, \
                 createdAt, startedAt, finishedAt, updatedAt), state or state.<field>"
            ));
        }
    };
    let mut path = Vec::new();
    let mut rest: Option<Vec<String>> = None;
    for segment in tail
        .map(|tail| tail.split('.').collect())
        .unwrap_or_else(Vec::new)
    {
        let (name, spread) = match segment.strip_suffix("[*]") {
            Some(name) => (name, true),
            None => (segment, false),
        };
        if name.is_empty()
            || name.len() > MAX_SEGMENT_BYTES
            || name.chars().any(|c| c.is_control() || c == '[' || c == ']')
        {
            return Err(format!("'{text}' has an empty or invalid segment"));
        }
        match &mut rest {
            Some(rest) => {
                if spread {
                    return Err(format!("'{text}': a path spreads one array at most"));
                }
                rest.push(name.to_owned());
            }
            None => {
                path.push(name.to_owned());
                if spread {
                    if !wildcard {
                        return Err(format!(
                            "'{text}': [*] is for conditions; to group, order or aggregate \
                             by an array's elements, unnest it"
                        ));
                    }
                    rest = Some(Vec::new());
                }
            }
        }
    }
    if path.len() + rest.as_ref().map_or(0, Vec::len) > MAX_PATH_SEGMENTS {
        return Err(format!(
            "'{text}' has more than {MAX_PATH_SEGMENTS} segments"
        ));
    }
    Ok(Target::Json { root, path, rest })
}

/// A JSON expression: `base` followed along `path`.
#[derive(Debug, Clone)]
struct Json {
    base: &'static str,
    path: Vec<String>,
}

impl Json {
    fn push(&self, q: &mut QueryBuilder<'_, Postgres>) {
        if self.path.is_empty() {
            q.push(self.base);
        } else {
            q.push("(")
                .push(self.base)
                .push(" #> ")
                .push_bind(self.path.clone())
                .push("::text[])");
        }
    }

    fn push_text(&self, q: &mut QueryBuilder<'_, Postgres>) {
        q.push("(");
        self.push(q);
        q.push(" #>> '{}')");
    }
}

/// `value` under `path`, as an object nested one level a segment.
fn nested(path: &[String], value: Value) -> Value {
    path.iter().rev().fold(value, |inner, key| {
        let mut object = serde_json::Map::new();
        object.insert(key.clone(), inner);
        Value::Object(object)
    })
}

/// A segment Postgres would read as an array index, where containment
/// reads a key: a path with one gets no containment test.
fn indexes(path: &[String]) -> bool {
    path.iter()
        .any(|segment| segment.trim().parse::<i64>().is_ok())
}

/// A value of a run column, typed for binding.
enum Typed {
    Text(String),
    Integer(i64),
    Time(DateTime<Utc>),
}

fn typed(kind: Kind, value: &Value, path: &str) -> Result<Typed, String> {
    match (kind, value) {
        (Kind::Text, Value::String(text)) => Ok(Typed::Text(text.clone())),
        (Kind::Integer, Value::Number(number)) if number.as_i64().is_some() => {
            Ok(Typed::Integer(number.as_i64().unwrap_or_default()))
        }
        (Kind::Time, Value::String(text)) => DateTime::parse_from_rfc3339(text)
            .map(|time| Typed::Time(time.with_timezone(&Utc)))
            .map_err(|_| format!("{path} takes an RFC 3339 date-time, not '{text}'")),
        _ => Err(format!(
            "{path} takes {}, not {value}",
            match kind {
                Kind::Text => "a string",
                Kind::Integer => "an integer",
                Kind::Time => "an RFC 3339 date-time string",
            }
        )),
    }
}

fn push_typed(q: &mut QueryBuilder<'_, Postgres>, value: Typed) {
    match value {
        Typed::Text(text) => q.push_bind(text),
        Typed::Integer(number) => q.push_bind(number),
        Typed::Time(time) => q.push_bind(time),
    };
}

fn push_typed_list(
    q: &mut QueryBuilder<'_, Postgres>,
    kind: Kind,
    values: &[Value],
    path: &str,
) -> Result<(), String> {
    match kind {
        Kind::Text => {
            let mut list = Vec::new();
            for value in values {
                if let Typed::Text(text) = typed(kind, value, path)? {
                    list.push(text);
                }
            }
            q.push_bind(list);
        }
        Kind::Integer => {
            let mut list = Vec::new();
            for value in values {
                if let Typed::Integer(number) = typed(kind, value, path)? {
                    list.push(number);
                }
            }
            q.push_bind(list);
        }
        Kind::Time => {
            let mut list = Vec::new();
            for value in values {
                if let Typed::Time(time) = typed(kind, value, path)? {
                    list.push(time);
                }
            }
            q.push_bind(list);
        }
    }
    Ok(())
}

struct Build {
    unnest: bool,
    nodes: usize,
    in_values: usize,
}

impl Build {
    fn condition(
        &mut self,
        q: &mut QueryBuilder<'_, Postgres>,
        condition: &Condition,
        depth: usize,
    ) -> Result<(), String> {
        self.nodes += 1;
        if self.nodes > MAX_CONDITION_NODES {
            return Err(format!(
                "a condition has at most {MAX_CONDITION_NODES} operators"
            ));
        }
        if depth > MAX_CONDITION_DEPTH {
            return Err(format!(
                "a condition nests at most {MAX_CONDITION_DEPTH} deep"
            ));
        }
        let op = condition.op.to_ascii_uppercase();
        match op.as_str() {
            "AND" | "OR" => {
                if condition.arguments.is_empty() {
                    q.push(if op == "AND" { "TRUE" } else { "FALSE" });
                    return Ok(());
                }
                q.push("(");
                for (n, argument) in condition.arguments.iter().enumerate() {
                    if n > 0 {
                        q.push(if op == "AND" { " AND " } else { " OR " });
                    }
                    let inner = sub_condition(argument, &op)?;
                    self.condition(q, &inner, depth + 1)?;
                }
                q.push(")");
                Ok(())
            }
            "NOT" => {
                let [argument] = condition.arguments.as_slice() else {
                    return Err("NOT takes one condition".into());
                };
                let inner = sub_condition(argument, "NOT")?;
                // A condition is NULL where what it reads is missing, which
                // a WHERE treats as false: NOT must too.
                q.push("NOT COALESCE((");
                self.condition(q, &inner, depth + 1)?;
                q.push("), FALSE)");
                Ok(())
            }
            _ => self.leaf(q, &op, &condition.arguments),
        }
    }

    fn leaf(
        &mut self,
        q: &mut QueryBuilder<'_, Postgres>,
        op: &str,
        arguments: &[Value],
    ) -> Result<(), String> {
        let Some(Value::String(path)) = arguments.first() else {
            return Err(format!("{op} takes a path first"));
        };
        let operand = arguments.get(1);
        let wants = match op {
            "IS_DEFINED" | "IS_EMPTY" | "IS_NOT_EMPTY" => 1..=2,
            "EQ" | "NE" | "GT" | "GTE" | "LT" | "LTE" | "IN" | "NOT_IN" | "CONTAINS"
            | "STARTS_WITH" | "ENDS_WITH" => 2..=2,
            _ => return Err(format!("'{op}' is not an operator")),
        };
        if !wants.contains(&arguments.len()) {
            return if wants.start() == wants.end() {
                Err(format!("{op} takes {} arguments", wants.end()))
            } else {
                Err(format!(
                    "{op} takes {} or {} arguments",
                    wants.start(),
                    wants.end()
                ))
            };
        }
        if matches!(op, "IS_DEFINED" | "IS_EMPTY" | "IS_NOT_EMPTY")
            && let Some(arg) = operand
            && !arg.is_boolean()
        {
            return Err(format!(
                "{op}'s second argument must be a boolean, not {arg}"
            ));
        }
        if matches!(op, "IN" | "NOT_IN") {
            match operand {
                Some(Value::Array(values))
                    if !values.is_empty() && values.len() <= MAX_IN_VALUES =>
                {
                    self.in_values = self.in_values.saturating_add(values.len());
                    if self.in_values > MAX_TOTAL_IN_VALUES {
                        return Err(format!(
                            "a query has at most {MAX_TOTAL_IN_VALUES} IN/NOT_IN values in total"
                        ));
                    }
                }
                _ => {
                    return Err(format!("{op} takes a list of 1 to {MAX_IN_VALUES} values"));
                }
            }
        }
        match parse_target(path, self.unnest, true)? {
            Target::Column(column) => column_leaf(q, column, path, op, operand),
            Target::Json {
                root,
                path: lead,
                rest: None,
            } => {
                if root == Root::State {
                    push_containment(q, op, operand, &lead, None);
                }
                let json = Json {
                    base: root.sql(),
                    path: lead,
                };
                json_leaf(q, &json, op, operand)
            }
            Target::Json {
                root,
                path: lead,
                rest: Some(rest),
            } => {
                if root == Root::State {
                    push_containment(q, op, operand, &lead, Some(&rest));
                }
                let array = Json {
                    base: root.sql(),
                    path: lead,
                };
                q.push("EXISTS (SELECT 1 FROM jsonb_array_elements(CASE WHEN jsonb_typeof(");
                array.push(q);
                q.push(") = 'array' THEN ");
                array.push(q);
                q.push(" END) AS w(e) WHERE ");
                json_leaf(
                    q,
                    &Json {
                        base: "w.e",
                        path: rest,
                    },
                    op,
                    operand,
                )?;
                q.push(")");
                Ok(())
            }
        }
    }
}

fn sub_condition(argument: &Value, op: &str) -> Result<Condition, String> {
    serde_json::from_value(argument.clone())
        .map_err(|error| format!("{op} takes conditions ({{op, arguments}}): {error}"))
}

/// A containment test that implies the condition, so the index on state
/// can find its candidates: `state @> {"a": {"b": value}}`, or with a
/// spread `state @> {"items": [{"sku": value}]}`. Pushed as `(test AND `,
/// closed by the caller's condition; nothing where no test implies it.
fn push_containment(
    q: &mut QueryBuilder<'_, Postgres>,
    op: &str,
    operand: Option<&Value>,
    lead: &[String],
    rest: Option<&[String]>,
) {
    if lead.is_empty() || indexes(lead) || rest.is_some_and(indexes) {
        return;
    }
    let shape = |value: Value| match rest {
        Some(rest) => nested(lead, Value::Array(vec![nested(rest, value)])),
        None => nested(lead, value),
    };
    let documents: Vec<Value> = match (op, operand) {
        ("EQ", Some(value)) if !value.is_null() => vec![shape(value.clone())],
        ("IN", Some(Value::Array(values)))
            if values.len() <= MAX_CONTAINMENT_IN_VALUES
                && values.iter().all(|value| !value.is_null()) =>
        {
            values.iter().cloned().map(shape).collect()
        }
        // An array holding the value; a string's substring is not implied.
        ("CONTAINS", Some(value)) if !value.is_string() && !value.is_null() => {
            vec![shape(Value::Array(vec![value.clone()]))]
        }
        _ => return,
    };
    q.push("(");
    for (n, document) in documents.into_iter().enumerate() {
        if n > 0 {
            q.push(" OR ");
        }
        q.push("r.state @> ").push_bind(document).push("::jsonb");
    }
    q.push(") AND ");
}

fn column_leaf(
    q: &mut QueryBuilder<'_, Postgres>,
    column: Column,
    path: &str,
    op: &str,
    operand: Option<&Value>,
) -> Result<(), String> {
    let sql = column.sql();
    let kind = column.kind();
    let operand = operand.unwrap_or(&Value::Null);
    match op {
        "EQ" if operand.is_null() => {
            q.push(sql).push(" IS NULL");
        }
        "NE" if operand.is_null() => {
            q.push(sql).push(" IS NOT NULL");
        }
        "EQ" | "NE" | "GT" | "GTE" | "LT" | "LTE" => {
            let operator = match op {
                "EQ" => " = ",
                "NE" => " <> ",
                "GT" => " > ",
                "GTE" => " >= ",
                "LT" => " < ",
                _ => " <= ",
            };
            q.push(sql).push(operator);
            push_typed(q, typed(kind, operand, path)?);
        }
        "IN" | "NOT_IN" => {
            let values = operand.as_array().map(Vec::as_slice).unwrap_or_default();
            let has_null = values.iter().any(Value::is_null);
            let non_null: Vec<Value> = values
                .iter()
                .filter(|value| !value.is_null())
                .cloned()
                .collect();
            if op == "NOT_IN" {
                q.push("(").push(sql).push(" IS NOT NULL AND NOT ");
            }
            q.push("(");
            if non_null.is_empty() {
                q.push("FALSE");
            } else {
                q.push(sql).push(" = ANY(");
                push_typed_list(q, kind, &non_null, path)?;
                q.push(")");
            }
            if has_null {
                q.push(" OR ").push(sql).push(" IS NULL");
            }
            q.push(")");
            if op == "NOT_IN" {
                q.push(")");
            }
        }
        "IS_DEFINED" | "IS_NOT_EMPTY" | "IS_EMPTY" => {
            let negate = operand == &Value::Bool(false);
            let empty = (op == "IS_EMPTY") != negate;
            let blank = if kind == Kind::Text {
                format!(" OR {sql} = ''")
            } else {
                String::new()
            };
            match (op, empty) {
                ("IS_DEFINED", false) => q.push(format!("{sql} IS NOT NULL")),
                ("IS_DEFINED", true) => q.push(format!("{sql} IS NULL")),
                (_, true) => q.push(format!("({sql} IS NULL{blank})")),
                (_, false) => q.push(format!("NOT ({sql} IS NULL{blank})")),
            };
        }
        "CONTAINS" | "STARTS_WITH" | "ENDS_WITH" => {
            if kind != Kind::Text {
                return Err(format!("{op} reads text, and {path} is not"));
            }
            let Value::String(text) = operand else {
                return Err(format!("{op} takes a string"));
            };
            match op {
                "CONTAINS" => q
                    .push("strpos(")
                    .push(sql)
                    .push(", ")
                    .push_bind(text.clone())
                    .push(") > 0"),
                "STARTS_WITH" => q
                    .push("starts_with(")
                    .push(sql)
                    .push(", ")
                    .push_bind(text.clone())
                    .push(")"),
                _ => q
                    .push("right(")
                    .push(sql)
                    .push(", length(")
                    .push_bind(text.clone())
                    .push(")) = ")
                    .push_bind(text.clone()),
            };
        }
        _ => return Err(format!("'{op}' is not an operator")),
    }
    Ok(())
}

/// A condition over a JSON value, which is SQL NULL where the path is
/// missing. A comparison compares values of the operand's JSON type only.
fn json_leaf(
    q: &mut QueryBuilder<'_, Postgres>,
    json: &Json,
    op: &str,
    operand: Option<&Value>,
) -> Result<(), String> {
    let operand = operand.cloned().unwrap_or(Value::Null);
    let not_null = |q: &mut QueryBuilder<'_, Postgres>| {
        q.push("jsonb_typeof(");
        json.push(q);
        q.push(") <> 'null'");
    };
    match op {
        "EQ" if operand.is_null() => {
            q.push("COALESCE(jsonb_typeof(");
            json.push(q);
            q.push(") = 'null', TRUE)");
        }
        "NE" if operand.is_null() => not_null(q),
        "EQ" => {
            json.push(q);
            q.push(" = ").push_bind(operand).push("::jsonb");
        }
        "NE" => {
            q.push("(");
            not_null(q);
            q.push(" AND ");
            json.push(q);
            q.push(" <> ").push_bind(operand).push("::jsonb)");
        }
        "GT" | "GTE" | "LT" | "LTE" => {
            let operator = match op {
                "GT" => " > ",
                "GTE" => " >= ",
                "LT" => " < ",
                _ => " <= ",
            };
            q.push("(jsonb_typeof(");
            json.push(q);
            match &operand {
                Value::Number(_) => {
                    // CASE, not AND: Postgres may cast before it tests.
                    q.push(") = 'number' AND CASE WHEN jsonb_typeof(");
                    json.push(q);
                    q.push(") = 'number' THEN (");
                    json.push(q);
                    q.push(")::numeric")
                        .push(operator)
                        .push("(")
                        .push_bind(operand.clone())
                        .push("::jsonb)::numeric END)");
                }
                Value::String(text) => {
                    q.push(") = 'string' AND ");
                    json.push_text(q);
                    q.push(" COLLATE \"C\"")
                        .push(operator)
                        .push_bind(text.clone())
                        .push(" COLLATE \"C\")");
                }
                Value::Bool(_) => {
                    q.push(") = 'boolean' AND ");
                    json.push(q);
                    q.push(operator).push_bind(operand.clone()).push("::jsonb)");
                }
                _ => return Err(format!("{op} compares a number, a string or a boolean")),
            }
        }
        "IN" | "NOT_IN" => {
            let values = operand.as_array().cloned().unwrap_or_default();
            let null = values.iter().any(Value::is_null);
            if op == "NOT_IN" {
                q.push("(");
                not_null(q);
                q.push(" AND NOT ");
            }
            q.push("(");
            json.push(q);
            q.push(" = ANY(ARRAY(SELECT jsonb_array_elements(")
                .push_bind(Value::Array(values))
                .push("::jsonb)))");
            if null {
                q.push(" OR COALESCE(jsonb_typeof(");
                json.push(q);
                q.push(") = 'null', TRUE)");
            }
            q.push(")");
            if op == "NOT_IN" {
                q.push(")");
            }
        }
        "IS_DEFINED" | "IS_EMPTY" | "IS_NOT_EMPTY" => {
            let negate = operand == Value::Bool(false);
            let empty_test = |q: &mut QueryBuilder<'_, Postgres>| {
                q.push("COALESCE(jsonb_typeof(");
                json.push(q);
                q.push(") = 'null' OR ");
                json.push(q);
                q.push(" IN ('\"\"'::jsonb, '[]'::jsonb, '{}'::jsonb), TRUE)");
            };
            match (op, negate) {
                ("IS_DEFINED", false) => not_null(q),
                ("IS_DEFINED", true) => {
                    q.push("COALESCE(jsonb_typeof(");
                    json.push(q);
                    q.push(") = 'null', TRUE)");
                }
                ("IS_EMPTY", false) | ("IS_NOT_EMPTY", true) => empty_test(q),
                _ => {
                    q.push("NOT ");
                    empty_test(q);
                }
            }
        }
        "CONTAINS" => {
            if operand.is_null() {
                return Err("CONTAINS takes a value".into());
            }
            q.push("((jsonb_typeof(");
            json.push(q);
            q.push(") = 'array' AND ");
            json.push(q);
            q.push(" @> jsonb_build_array(")
                .push_bind(operand.clone())
                .push("::jsonb))");
            if let Value::String(text) = &operand {
                q.push(" OR (jsonb_typeof(");
                json.push(q);
                q.push(") = 'string' AND strpos(");
                json.push_text(q);
                q.push(", ").push_bind(text.clone()).push(") > 0)");
            }
            q.push(")");
        }
        "STARTS_WITH" | "ENDS_WITH" => {
            let Value::String(text) = operand else {
                return Err(format!("{op} takes a string"));
            };
            q.push("(jsonb_typeof(");
            json.push(q);
            q.push(") = 'string' AND ");
            if op == "STARTS_WITH" {
                q.push("starts_with(");
                json.push_text(q);
                q.push(", ").push_bind(text).push("))");
            } else {
                q.push("right(");
                json.push_text(q);
                q.push(", length(")
                    .push_bind(text.clone())
                    .push(")) = ")
                    .push_bind(text)
                    .push(")");
            }
        }
        _ => return Err(format!("'{op}' is not an operator")),
    }
    Ok(())
}

/// A path read for a row: a column (`kind` set) or JSON.
fn push_value(q: &mut QueryBuilder<'_, Postgres>, target: &Target) -> Option<Kind> {
    match target {
        Target::Column(column) => {
            q.push(column.sql());
            Some(column.kind())
        }
        Target::Json { root, path, .. } => {
            Json {
                base: root.sql(),
                path: path.clone(),
            }
            .push(q);
            None
        }
    }
}

/// The SQL of an aggregate over `value`, a column of `kind` or JSON.
fn aggregate_sql(
    function: AggregateFn,
    value: Option<(&str, Option<Kind>)>,
) -> Result<String, String> {
    let Some((v, kind)) = value else {
        return match function {
            AggregateFn::Count => Ok("to_jsonb(count(*))".into()),
            _ => Err("only count is computed without a path".into()),
        };
    };
    Ok(match (function, kind) {
        (AggregateFn::Count, Some(_)) => format!("to_jsonb(count({v}))"),
        (AggregateFn::Count, None) => {
            format!("to_jsonb(count(*) FILTER (WHERE jsonb_typeof({v}) <> 'null'))")
        }
        (AggregateFn::CountDistinct, Some(_)) => format!("to_jsonb(count(DISTINCT {v}))"),
        (AggregateFn::CountDistinct, None) => {
            format!("to_jsonb(count(DISTINCT {v}) FILTER (WHERE jsonb_typeof({v}) <> 'null'))")
        }
        (AggregateFn::Min | AggregateFn::Max, Some(_)) => {
            format!("to_jsonb({}({v}))", name_of(function))
        }
        (AggregateFn::Min | AggregateFn::Max, None) => {
            let f = name_of(function);
            format!(
                "COALESCE(to_jsonb({f}(CASE WHEN jsonb_typeof({v}) = 'number' THEN ({v})::numeric END)), \
                 to_jsonb({f}(CASE WHEN jsonb_typeof({v}) = 'string' THEN ({v} #>> '{{}}') END COLLATE \"C\")))"
            )
        }
        (AggregateFn::Sum | AggregateFn::Avg, Some(Kind::Integer)) => {
            format!("to_jsonb({}({v}))", name_of(function))
        }
        (AggregateFn::Sum | AggregateFn::Avg, None) => format!(
            "to_jsonb({}(CASE WHEN jsonb_typeof({v}) = 'number' THEN ({v})::numeric END))",
            name_of(function)
        ),
        (AggregateFn::Sum | AggregateFn::Avg, Some(_)) => {
            return Err(format!("{} adds numbers", name_of(function)));
        }
    })
}

fn name_of(function: AggregateFn) -> &'static str {
    match function {
        AggregateFn::Count => "count",
        AggregateFn::CountDistinct => "count_distinct",
        AggregateFn::Min => "min",
        AggregateFn::Max => "max",
        AggregateFn::Sum => "sum",
        AggregateFn::Avg => "avg",
    }
}

const DEFAULT_SELECT: &[&str] = &[
    "runId",
    "label",
    "status",
    "version",
    "createdAt",
    "updatedAt",
    "state",
];

/// The rows of one workflow's runs, aliased `r`: every run, or each
/// label's latest, with the same columns.
fn push_base(q: &mut QueryBuilder<'_, Postgres>, tenant: &str, workflow: &str, latest: bool) {
    let start = i32::try_from(workflow.chars().count() + 2).unwrap_or(i32::MAX);
    let version = |q: &mut QueryBuilder<'_, Postgres>, name: &str| {
        q.push("CASE WHEN split_part(substr(")
            .push(name.to_owned())
            .push(", ")
            .push_bind(start)
            .push("), '@', 1) ~ '^[0-9]{1,9}$' THEN split_part(substr(")
            .push(name.to_owned())
            .push(", ")
            .push_bind(start)
            .push("), '@', 1)::int END");
    };
    if latest {
        q.push(
            "(SELECT k.instance_id AS run_id, k.run_label AS label, \
             k.status, ",
        );
        version(q, "k.image_name");
        q.push(
            " AS version, k.run_created_at AS created_at, k.started_at, k.finished_at, \
             k.state_updated_at AS updated_at, k.state \
             FROM run_keys k WHERE k.tenant_id = ",
        )
        .push_bind(tenant.to_owned())
        .push(" AND k.workflow_id = ")
        .push_bind(workflow.to_owned())
        .push(") r");
    } else {
        q.push("(SELECT i.instance_id AS run_id, i.run_label AS label, i.status::text AS status, ");
        version(q, "img.name");
        q.push(
            " AS version, i.created_at, i.started_at, i.finished_at, \
             s.state_updated_at AS updated_at, s.state \
             FROM instances i JOIN instance_images ii ON ii.instance_id = i.instance_id \
             JOIN images img ON img.image_id = ii.image_id \
             LEFT JOIN instance_state s ON s.instance_id = i.instance_id \
             WHERE i.tenant_id = ",
        )
        .push_bind(tenant.to_owned())
        .push(" AND img.tenant_id = ")
        .push_bind(tenant.to_owned())
        .push(" AND img.name LIKE ")
        .push_bind(format!(
            "{}%",
            crate::db::escape_like_literal(&format!("{workflow}:"))
        ))
        .push(") r");
    }
}

/// The statement a query is answered with, and its columns.
fn build(
    tenant: &str,
    workflow: &str,
    query: &RunQuery,
) -> Result<(QueryBuilder<'static, Postgres>, Vec<String>, u32), String> {
    if workflow.is_empty() || workflow.contains(':') {
        return Err("a query names one workflow".into());
    }
    let limit = query.limit.unwrap_or(DEFAULT_LIMIT);
    if limit == 0 || limit > MAX_LIMIT {
        return Err(format!("limit is 1 to {MAX_LIMIT}"));
    }
    let offset = query.offset.unwrap_or(0);
    if offset > MAX_OFFSET {
        return Err(format!("offset is at most {MAX_OFFSET}"));
    }
    if query.select.len() > MAX_SELECT
        || query.group_by.len() > MAX_GROUP_BY
        || query.aggregates.len() > MAX_AGGREGATES
        || query.order_by.len() > MAX_ORDER_BY
    {
        return Err(format!(
            "a query selects at most {MAX_SELECT} paths, groups by {MAX_GROUP_BY}, \
             computes {MAX_AGGREGATES} aggregates and orders by {MAX_ORDER_BY}"
        ));
    }
    let grouped = !query.group_by.is_empty() || !query.aggregates.is_empty();
    if grouped && !query.select.is_empty() {
        return Err("a query with groupBy or aggregates returns those, not select".into());
    }
    let unnest = match &query.unnest {
        None => None,
        Some(path) => match parse_target(path, false, false)? {
            Target::Json {
                root: Root::State,
                path,
                rest: None,
            } if !path.is_empty() => Some(path),
            _ => return Err("unnest takes a path into state (state.items)".into()),
        },
    };
    let mut build = Build {
        unnest: unnest.is_some(),
        nodes: 0,
        in_values: 0,
    };

    let mut q: QueryBuilder<'static, Postgres> = QueryBuilder::new("");
    let columns: Vec<String> = if grouped {
        // Group keys and aggregate inputs are read in an inner query, so
        // the outer one groups by plain columns.
        let keys: Vec<Target> = query
            .group_by
            .iter()
            .map(|path| parse_target(path, build.unnest, false))
            .collect::<Result<_, _>>()?;
        let inputs: Vec<Option<Target>> = query
            .aggregates
            .iter()
            .map(|aggregate| {
                aggregate
                    .path
                    .as_deref()
                    .map(|path| parse_target(path, build.unnest, false))
                    .transpose()
            })
            .collect::<Result<_, _>>()?;
        let mut names = Vec::with_capacity(query.aggregates.len());
        for aggregate in &query.aggregates {
            let name = match &aggregate.alias {
                Some(alias) => {
                    if alias.is_empty()
                        || alias.len() > MAX_SEGMENT_BYTES
                        || alias.chars().any(char::is_control)
                    {
                        return Err(format!("'{alias}' is not a valid aggregate alias"));
                    }
                    alias.clone()
                }
                None => match &aggregate.path {
                    Some(path) => format!("{}({path})", name_of(aggregate.function)),
                    None => name_of(aggregate.function).to_owned(),
                },
            };
            names.push(name);
        }
        let mut seen = std::collections::HashSet::with_capacity(query.group_by.len() + names.len());
        for name in query.group_by.iter().chain(names.iter()) {
            if !seen.insert(name.as_str()) {
                return Err(format!("duplicate output column '{name}'"));
            }
        }
        let mut sums = Vec::new();
        q.push("SELECT jsonb_build_array(");
        let mut parts = Vec::new();
        for n in 0..keys.len() {
            parts.push(format!("to_jsonb(q.g{n})"));
        }
        for (n, input) in inputs.iter().enumerate() {
            let kind = match input {
                Some(Target::Column(column)) => Some(Some(column.kind())),
                Some(Target::Json { .. }) => Some(None),
                None => None,
            };
            let value = kind.map(|kind| (format!("q.v{n}"), kind));
            sums.push(aggregate_sql(
                query.aggregates[n].function,
                value.as_ref().map(|(v, kind)| (v.as_str(), *kind)),
            )?);
        }
        parts.extend(sums.iter().cloned());
        q.push(parts.join(", ")).push(")::text AS row");
        let mut order = Vec::new();
        for key in &query.order_by {
            let expression = if let Some(n) = query.group_by.iter().position(|g| *g == key.path) {
                format!("q.g{n}")
            } else if let Some(n) = names.iter().position(|name| *name == key.path) {
                sums[n].clone()
            } else {
                return Err(format!(
                    "'{}' orders groups by neither a groupBy path nor an aggregate",
                    key.path
                ));
            };
            order.push((expression, key.desc));
        }
        for (n, (expression, _)) in order.iter().enumerate() {
            q.push(format!(", {expression} AS o{n}"));
        }
        if query.count {
            q.push(", count(*) OVER () AS total");
        }
        q.push(" FROM (SELECT ");
        let mut first = true;
        for (n, key) in keys.iter().enumerate() {
            if !first {
                q.push(", ");
            }
            first = false;
            push_value(&mut q, key);
            q.push(format!(" AS g{n}"));
        }
        for (n, input) in inputs.iter().enumerate() {
            if let Some(target) = input {
                if !first {
                    q.push(", ");
                }
                first = false;
                push_value(&mut q, target);
                q.push(format!(" AS v{n}"));
            }
        }
        if first {
            q.push("1 AS one");
        }
        push_from(&mut q, tenant, workflow, query, unnest.as_deref());
        push_where(&mut q, &mut build, query)?;
        q.push(") q");
        if !keys.is_empty() {
            let by: Vec<String> = (0..keys.len()).map(|n| format!("q.g{n}")).collect();
            q.push(" GROUP BY ").push(by.join(", "));
        }
        let mut terms: Vec<String> = order
            .iter()
            .enumerate()
            .map(|(n, (_, desc))| format!("o{n} {} NULLS LAST", if *desc { "DESC" } else { "ASC" }))
            .collect();
        terms.extend((0..keys.len()).map(|n| format!("q.g{n} ASC NULLS LAST")));
        if !terms.is_empty() {
            q.push(" ORDER BY ").push(terms.join(", "));
        }
        query.group_by.iter().cloned().chain(names).collect()
    } else {
        let mut select: Vec<String> = if query.select.is_empty() {
            DEFAULT_SELECT.iter().map(|s| (*s).to_owned()).collect()
        } else {
            query.select.clone()
        };
        if query.select.is_empty() && unnest.is_some() {
            select.push("item".into());
        }
        let mut seen = std::collections::HashSet::with_capacity(select.len());
        for path in &select {
            if !seen.insert(path.as_str()) {
                return Err(format!("duplicate select path '{path}'"));
            }
        }
        let targets: Vec<Target> = select
            .iter()
            .map(|path| parse_target(path, build.unnest, false))
            .collect::<Result<_, _>>()?;
        let order: Vec<(Target, bool)> = query
            .order_by
            .iter()
            .map(|key| Ok((parse_target(&key.path, build.unnest, false)?, key.desc)))
            .collect::<Result<_, String>>()?;
        q.push("SELECT jsonb_build_array(");
        for (n, target) in targets.iter().enumerate() {
            if n > 0 {
                q.push(", ");
            }
            q.push("to_jsonb(");
            push_value(&mut q, target);
            q.push(")");
        }
        q.push(")::text AS row");
        for (n, (target, _)) in order.iter().enumerate() {
            q.push(", ");
            push_value(&mut q, target);
            q.push(format!(" AS o{n}"));
        }
        if query.count {
            q.push(", count(*) OVER () AS total");
        }
        push_from(&mut q, tenant, workflow, query, unnest.as_deref());
        push_where(&mut q, &mut build, query)?;
        let mut terms: Vec<String> = order
            .iter()
            .enumerate()
            .map(|(n, (_, desc))| format!("o{n} {} NULLS LAST", if *desc { "DESC" } else { "ASC" }))
            .collect();
        if order.is_empty() {
            terms.push(if query.latest_per_label {
                "r.label ASC".into()
            } else {
                "r.created_at DESC".into()
            });
        }
        terms.push(if query.latest_per_label {
            "r.label ASC".into()
        } else {
            "r.run_id DESC".into()
        });
        if unnest.is_some() {
            terms.push("u.n ASC".into());
        }
        q.push(" ORDER BY ").push(terms.join(", "));
        select
    };
    q.push(" LIMIT ")
        .push_bind(i64::from(limit) + 1)
        .push(" OFFSET ")
        .push_bind(i64::from(offset));
    Ok((q, columns, limit))
}

fn push_from(
    q: &mut QueryBuilder<'static, Postgres>,
    tenant: &str,
    workflow: &str,
    query: &RunQuery,
    unnest: Option<&[String]>,
) {
    q.push(" FROM ");
    push_base(q, tenant, workflow, query.latest_per_label);
    if let Some(path) = unnest {
        let array = Json {
            base: "r.state",
            path: path.to_vec(),
        };
        q.push(" CROSS JOIN LATERAL jsonb_array_elements(CASE WHEN jsonb_typeof(");
        array.push(q);
        q.push(") = 'array' THEN ");
        array.push(q);
        q.push(" END) WITH ORDINALITY AS u(item, n)");
    }
}

fn push_where(
    q: &mut QueryBuilder<'static, Postgres>,
    build: &mut Build,
    query: &RunQuery,
) -> Result<(), String> {
    if let Some(condition) = &query.condition {
        q.push(" WHERE ");
        build.condition(q, condition, 1)?;
    }
    Ok(())
}

/// How many queries run at once: at most [`QueryLimits::per_tenant`] for one
/// tenant and [`QueryLimits::total`] in all, so that queries cannot crowd
/// out the engine's own work on the same database. A query waits up to
/// [`QueryLimits::wait`] for its turn, then is refused as busy ([`BUSY`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QueryLimits {
    /// Queries one tenant runs at once.
    pub per_tenant: usize,
    /// Queries every tenant runs at once.
    pub total: usize,
    /// How long a query waits for its turn.
    pub wait: std::time::Duration,
}

impl QueryLimits {
    /// From `RUNTARA_RUN_QUERY_PER_TENANT` (default 1),
    /// `RUNTARA_RUN_QUERY_TOTAL` (default 2) and `RUNTARA_RUN_QUERY_WAIT_MS`
    /// (default 10000).
    ///
    /// Measured on a 16-core Postgres with the engine writing runs at full
    /// speed (`examples/run_query_load.rs`): two heavy queries at a time
    /// left the engine 93 to 95 % of its throughput and its p99 latency;
    /// four left it about 70 %; no limit, 16 % at twenty clients. Raise
    /// the total with the database's cores.
    pub fn from_env() -> Self {
        let read = |name: &str, default: usize| {
            std::env::var(name)
                .ok()
                .and_then(|value| value.parse().ok())
                .filter(|value: &usize| *value > 0)
                .unwrap_or(default)
        };
        Self {
            per_tenant: read("RUNTARA_RUN_QUERY_PER_TENANT", 1),
            total: read("RUNTARA_RUN_QUERY_TOTAL", 2),
            wait: std::time::Duration::from_millis(read("RUNTARA_RUN_QUERY_WAIT_MS", 10_000) as u64),
        }
    }
}

/// The query limit's instruments, so its limits can be tuned from what a
/// deployment sees:
///
/// - `runtara.run_query.running`: queries holding a turn;
/// - `runtara.run_query.waiting`: queries waiting for one;
/// - `runtara.run_query.refused`: queries that waited [`QueryLimits::wait`]
///   and were refused as busy ([`BUSY`]);
/// - `runtara.run_query.wait.duration`: how long a query waited for its
///   turn, by `outcome` (`admitted` or `refused`);
/// - `runtara.run_query.limit`: the limits, by `scope` (`total` or
///   `per_tenant`).
///
/// Each carries the tenant (`tenant_id`) when `RUNTARA_METRICS_TENANT_LABEL`
/// asks for it ([`crate::pipeline_metrics::tenant_label_enabled`]), except
/// the limits, which are the process's.
#[derive(Clone, Debug)]
struct QueryMetrics {
    running: opentelemetry::metrics::UpDownCounter<i64>,
    waiting: opentelemetry::metrics::UpDownCounter<i64>,
    refused: opentelemetry::metrics::Counter<u64>,
    wait: opentelemetry::metrics::Histogram<f64>,
    tenant_label: bool,
}

impl QueryMetrics {
    fn new(meter: &opentelemetry::metrics::Meter, limits: QueryLimits, tenant_label: bool) -> Self {
        meter
            .i64_observable_up_down_counter("runtara.run_query.limit")
            .with_description("Run queries the engine runs at once")
            .with_unit("{query}")
            .with_callback(move |observer| {
                observer.observe(
                    limits.total as i64,
                    &[opentelemetry::KeyValue::new("scope", "total")],
                );
                observer.observe(
                    limits.per_tenant as i64,
                    &[opentelemetry::KeyValue::new("scope", "per_tenant")],
                );
            })
            .build();
        Self {
            running: meter
                .i64_up_down_counter("runtara.run_query.running")
                .with_description("Run queries holding a turn")
                .with_unit("{query}")
                .build(),
            waiting: meter
                .i64_up_down_counter("runtara.run_query.waiting")
                .with_description("Run queries waiting for a turn")
                .with_unit("{query}")
                .build(),
            refused: meter
                .u64_counter("runtara.run_query.refused")
                .with_description("Run queries refused as busy after waiting for a turn")
                .with_unit("{query}")
                .build(),
            wait: meter
                .f64_histogram("runtara.run_query.wait.duration")
                .with_description("How long a run query waited for its turn")
                .with_unit("s")
                .with_boundaries(crate::pipeline_metrics::duration_boundaries())
                .build(),
            tenant_label,
        }
    }

    fn global(limits: QueryLimits) -> Option<Self> {
        crate::pipeline_metrics::telemetry_enabled().then(|| {
            Self::new(
                &opentelemetry::global::meter("runtara-environment.run_query"),
                limits,
                crate::pipeline_metrics::tenant_label_enabled(),
            )
        })
    }

    fn tenant(&self, tenant: &str) -> Vec<opentelemetry::KeyValue> {
        crate::pipeline_metrics::tenant_attribute(self.tenant_label, tenant)
            .into_iter()
            .collect()
    }
}

/// Turns for queries, as [`QueryLimits`] allows them.
struct Turns {
    limits: QueryLimits,
    total: std::sync::Arc<tokio::sync::Semaphore>,
    tenants:
        std::sync::Mutex<std::collections::HashMap<String, std::sync::Arc<tokio::sync::Semaphore>>>,
    metrics: Option<QueryMetrics>,
}

/// A query's turn: held while it runs.
struct Turn {
    _tenant: tokio::sync::OwnedSemaphorePermit,
    _total: tokio::sync::OwnedSemaphorePermit,
    /// Counted as running until the turn ends.
    running: Option<(QueryMetrics, Vec<opentelemetry::KeyValue>)>,
}

impl Drop for Turn {
    fn drop(&mut self) {
        if let Some((metrics, attributes)) = &self.running {
            metrics.running.add(-1, attributes);
        }
    }
}

/// Refused because the engine runs as many queries as it may.
pub const BUSY: &str = "too many run queries at once; try again shortly";

/// Counts a query waiting for a turn until it leaves the wait (whether
/// admitted, timed out, or cancelled by a dropped future).
struct WaitingGuard<'a> {
    metrics: Option<&'a QueryMetrics>,
    attributes: &'a [opentelemetry::KeyValue],
}

impl<'a> WaitingGuard<'a> {
    fn enter(metrics: Option<&'a QueryMetrics>, attributes: &'a [opentelemetry::KeyValue]) -> Self {
        if let Some(metrics) = metrics {
            metrics.waiting.add(1, attributes);
        }
        Self {
            metrics,
            attributes,
        }
    }
}

impl Drop for WaitingGuard<'_> {
    fn drop(&mut self) {
        if let Some(metrics) = self.metrics {
            metrics.waiting.add(-1, self.attributes);
        }
    }
}

impl Turns {
    fn new(limits: QueryLimits, metrics: Option<QueryMetrics>) -> Self {
        Self {
            limits,
            total: std::sync::Arc::new(tokio::sync::Semaphore::new(limits.total)),
            tenants: Default::default(),
            metrics,
        }
    }

    async fn take(&self, tenant: &str) -> Result<Turn, String> {
        let tenant_turns = {
            let mut tenants = self.tenants.lock().unwrap_or_else(|e| e.into_inner());
            // Tenants with nothing running or waiting are forgotten, so the
            // map holds only tenants that query.
            tenants.retain(|_, turns| std::sync::Arc::strong_count(turns) > 1);
            tenants
                .entry(tenant.to_owned())
                .or_insert_with(|| {
                    std::sync::Arc::new(tokio::sync::Semaphore::new(self.limits.per_tenant))
                })
                .clone()
        };
        let waited = async {
            let tenant = tenant_turns
                .acquire_owned()
                .await
                .map_err(|e| e.to_string())?;
            let total = self
                .total
                .clone()
                .acquire_owned()
                .await
                .map_err(|e| e.to_string())?;
            Ok::<_, String>((tenant, total))
        };
        let attributes = self
            .metrics
            .as_ref()
            .map(|metrics| metrics.tenant(tenant))
            .unwrap_or_default();
        let waiting = WaitingGuard::enter(self.metrics.as_ref(), &attributes);
        let started = std::time::Instant::now();
        let outcome = tokio::time::timeout(self.limits.wait, waited).await;
        drop(waiting);
        if let Some(metrics) = &self.metrics {
            let mut waited = attributes.clone();
            waited.push(opentelemetry::KeyValue::new(
                "outcome",
                if outcome.is_ok() {
                    "admitted"
                } else {
                    "refused"
                },
            ));
            metrics
                .wait
                .record(started.elapsed().as_secs_f64(), &waited);
        }
        let (tenant, total) = match outcome {
            Ok(permits) => permits?,
            Err(_) => {
                if let Some(metrics) = &self.metrics {
                    metrics.refused.add(1, &attributes);
                }
                return Err(BUSY.to_owned());
            }
        };
        let running = self.metrics.clone().map(|metrics| {
            metrics.running.add(1, &attributes);
            (metrics, attributes)
        });
        Ok(Turn {
            _tenant: tenant,
            _total: total,
            running,
        })
    }
}

fn turns() -> &'static Turns {
    static TURNS: std::sync::OnceLock<Turns> = std::sync::OnceLock::new();
    TURNS.get_or_init(|| {
        let limits = QueryLimits::from_env();
        Turns::new(limits, QueryMetrics::global(limits))
    })
}

fn is_statement_timeout(error: &sqlx::Error) -> bool {
    matches!(
        error,
        sqlx::Error::Database(db) if db.code().as_deref() == Some("57014")
    )
}

fn map_query_db_error(error: sqlx::Error) -> crate::error::Error {
    if is_statement_timeout(&error) {
        crate::error::Error::Timeout(format!(
            "run query exceeded the {STATEMENT_TIMEOUT} time limit"
        ))
    } else {
        crate::error::Error::Database(error)
    }
}

impl InstanceRepository {
    /// Answers `query` over `tenant`'s runs of `workflow`.
    pub async fn query_runs(
        &self,
        tenant: &str,
        workflow: &str,
        query: RunQuery,
    ) -> crate::error::Result<RunQueryResult> {
        self.answer(tenant, workflow, query, true).await
    }

    /// The latest run of `label`, as [`RunQuery::latest_per_label`] reads
    /// it: one row of [`DEFAULT_SELECT`]'s columns, or none. A single-statement
    /// primary-key lookup on `run_keys` taking no query turn ([`QueryLimits`]),
    /// so a workflow reading its records step after step is not held behind
    /// scans or extra transaction round trips.
    pub async fn latest_run_for_label(
        &self,
        tenant: &str,
        workflow: &str,
        label: &str,
    ) -> crate::error::Result<RunQueryResult> {
        if workflow.is_empty() || workflow.contains(':') {
            return Err(crate::error::Error::InvalidRequest(
                "a query names one workflow".into(),
            ));
        }
        let start = i32::try_from(workflow.chars().count() + 2).unwrap_or(i32::MAX);
        let raw: Option<String> = sqlx::query_scalar(
            r#"
            SELECT jsonb_build_array(
                to_jsonb(k.instance_id),
                to_jsonb(k.run_label),
                to_jsonb(k.status),
                to_jsonb(CASE
                    WHEN split_part(substr(k.image_name, $4), '@', 1) ~ '^[0-9]{1,9}$'
                    THEN split_part(substr(k.image_name, $4), '@', 1)::int
                END),
                to_jsonb(k.run_created_at),
                to_jsonb(k.state_updated_at),
                to_jsonb(k.state)
            )::text
            FROM run_keys k
            WHERE k.tenant_id = $1 AND k.workflow_id = $2 AND k.run_label = $3
            "#,
        )
        .bind(tenant)
        .bind(workflow)
        .bind(label)
        .bind(start)
        .fetch_optional(self.pool())
        .await
        .map_err(map_query_db_error)?;
        let columns: Vec<String> = DEFAULT_SELECT.iter().map(|s| (*s).to_owned()).collect();
        let rows = match raw {
            Some(raw) => {
                if raw.len() > MAX_RESULT_BYTES {
                    return Err(crate::error::Error::InvalidRequest(format!(
                        "row exceeds the {MAX_RESULT_BYTES}-byte result limit"
                    )));
                }
                vec![serde_json::from_str(&raw)?]
            }
            None => Vec::new(),
        };
        Ok(RunQueryResult {
            columns,
            rows,
            total: None,
            has_more: false,
        })
    }

    async fn answer(
        &self,
        tenant: &str,
        workflow: &str,
        query: RunQuery,
        take_turn: bool,
    ) -> crate::error::Result<RunQueryResult> {
        let (mut statement, columns, limit) =
            build(tenant, workflow, &query).map_err(crate::error::Error::InvalidRequest)?;
        let _turn = if take_turn {
            Some(
                turns()
                    .take(tenant)
                    .await
                    .map_err(crate::error::Error::Busy)?,
            )
        } else {
            None
        };
        let mut tx = self.pool().begin().await?;
        sqlx::query(&format!(
            "SET LOCAL statement_timeout = '{STATEMENT_TIMEOUT}'"
        ))
        .execute(&mut *tx)
        .await?;

        let mut rows = Vec::new();
        let mut total = None;
        let mut bytes = 0usize;
        let mut has_more = false;
        let mut capped = false;
        {
            let mut stream = statement.build().fetch(&mut *tx);
            while let Some(row) = stream.try_next().await.map_err(map_query_db_error)? {
                if rows.len() >= limit as usize {
                    has_more = true;
                    break;
                }
                if query.count && total.is_none() {
                    total =
                        Some(u64::try_from(row.try_get::<i64, _>("total")?).unwrap_or_default());
                }
                let raw: String = row.try_get("row")?;
                let next_bytes = bytes.saturating_add(raw.len());
                if next_bytes > MAX_RESULT_BYTES {
                    if rows.is_empty() {
                        return Err(crate::error::Error::InvalidRequest(format!(
                            "a single row exceeds the {MAX_RESULT_BYTES}-byte result limit"
                        )));
                    }
                    has_more = true;
                    capped = true;
                    break;
                }
                bytes = next_bytes;
                let values: Vec<Value> = serde_json::from_str(&raw)?;
                rows.push(values);
            }
        }
        // Only commit cleanly when the stream was drained; rolling back after
        // an early break avoids draining unread rows or leaving portal state.
        if capped || has_more {
            tx.rollback().await?;
        } else {
            tx.commit().await?;
        }
        if query.count && total.is_none() && query.offset.unwrap_or(0) == 0 {
            total = Some(0);
        }
        Ok(RunQueryResult {
            columns,
            rows,
            total,
            has_more,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn query(value: Value) -> RunQuery {
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn paths_parse() {
        assert_eq!(
            parse_target("state.customer.country", false, false).unwrap(),
            Target::Json {
                root: Root::State,
                path: vec!["customer".into(), "country".into()],
                rest: None
            }
        );
        assert_eq!(
            parse_target("state.items[*].sku", false, true).unwrap(),
            Target::Json {
                root: Root::State,
                path: vec!["items".into()],
                rest: Some(vec!["sku".into()])
            }
        );
        assert_eq!(
            parse_target("status", false, false).unwrap(),
            Target::Column(Column::Status)
        );
        assert!(parse_target("state.items[*].sku", false, false).is_err());
        assert!(parse_target("state.a[*].b[*]", false, true).is_err());
        assert!(parse_target("item.sku", false, true).is_err());
        assert!(parse_target("state..a", false, true).is_err());
        assert!(parse_target("fitScore", false, true).is_err());
    }

    #[test]
    fn queries_are_checked() {
        let checked = |value: Value| build("t", "wf", &query(value)).map(|_| ());
        assert!(checked(json!({})).is_ok());
        assert!(checked(json!({"limit": 0})).is_err());
        assert!(checked(json!({"limit": 5000})).is_err());
        assert!(checked(json!({"select": ["state.a"], "groupBy": ["state.a"]})).is_err());
        assert!(
            checked(json!({"condition": {"op": "LIKE", "arguments": ["state.a", "x"]}})).is_err()
        );
        assert!(checked(json!({"condition": {"op": "IN", "arguments": ["state.a", []]}})).is_err());
        assert!(
            checked(json!({"condition": {"op": "EQ", "arguments": ["version", "x"]}})).is_err()
        );
        assert!(
            checked(json!({"condition": {"op": "GT", "arguments": ["state.a", [1]]}})).is_err()
        );
        assert!(checked(json!({"unnest": "status"})).is_err());
        assert!(checked(json!({"groupBy": ["state.items[*].sku"]})).is_err());
        assert!(checked(json!({"aggregates": [{"fn": "sum", "path": "status"}]})).is_err());
        assert!(checked(json!({"orderBy": [{"path": "nope"}], "groupBy": ["status"]})).is_err());
        assert!(serde_json::from_value::<RunQuery>(json!({"bogus": 1})).is_err());
        assert!(
            checked(json!({
                "unnest": "state.items",
                "condition": {"op": "GT", "arguments": ["item.qty", 5]},
                "groupBy": ["item.sku"],
                "aggregates": [{"fn": "sum", "path": "item.qty", "as": "qty"}],
                "orderBy": [{"path": "qty", "desc": true}]
            }))
            .is_ok()
        );
        assert!(build("t", "a:b", &RunQuery::default()).is_err());
    }

    #[test]
    fn deep_conditions_are_refused() {
        let mut condition = json!({"op": "EQ", "arguments": ["state.a", 1]});
        for _ in 0..20 {
            condition = json!({"op": "NOT", "arguments": [condition]});
        }
        assert!(build("t", "wf", &query(json!({ "condition": condition }))).is_err());
    }

    #[tokio::test]
    async fn turns_are_shared_out_per_tenant_and_in_all() {
        let turns = Turns::new(
            QueryLimits {
                per_tenant: 1,
                total: 2,
                wait: std::time::Duration::from_millis(50),
            },
            None,
        );
        let a = turns.take("a").await.unwrap();
        // A's second waits for its first, and is refused when it does not end.
        assert_eq!(turns.take("a").await.err().as_deref(), Some(BUSY));
        let b = turns.take("b").await.unwrap();
        // Two run in all: a third tenant waits too.
        assert!(turns.take("c").await.is_err());
        drop(a);
        let c = turns.take("c").await.unwrap();
        drop((b, c));
        assert!(turns.take("a").await.is_ok());
        // Tenants with nothing running are forgotten.
        assert_eq!(turns.tenants.lock().unwrap().len(), 1);
    }

    /// The limit's instruments count queries running, waiting and refused,
    /// with the tenant when asked, and show the limits.
    #[tokio::test]
    async fn the_query_limit_is_measured() {
        use opentelemetry::metrics::MeterProvider;
        use opentelemetry_sdk::metrics::data::{AggregatedMetrics, MetricData};
        use opentelemetry_sdk::metrics::{InMemoryMetricExporter, SdkMeterProvider};

        let exporter = InMemoryMetricExporter::default();
        let provider = SdkMeterProvider::builder()
            .with_periodic_exporter(exporter.clone())
            .build();
        let limits = QueryLimits {
            per_tenant: 1,
            total: 2,
            wait: std::time::Duration::from_millis(50),
        };
        let metrics = QueryMetrics::new(&provider.meter("test"), limits, true);
        let turns = Turns::new(limits, Some(metrics));
        let value = |name: &str, tenant: Option<&str>| -> i64 {
            provider.force_flush().unwrap();
            exporter
                .get_finished_metrics()
                .unwrap()
                .iter()
                .flat_map(|resource| resource.scope_metrics())
                .flat_map(|scope| scope.metrics())
                .filter(|metric| metric.name() == name)
                .last()
                .map(|metric| {
                    let wanted = |attributes: Vec<String>| {
                        tenant.is_none_or(|tenant| {
                            attributes.contains(&format!("tenant_id={tenant}"))
                        })
                    };
                    match metric.data() {
                        AggregatedMetrics::I64(MetricData::Sum(sum)) => sum
                            .data_points()
                            .filter(|p| {
                                wanted(
                                    p.attributes()
                                        .map(|kv| format!("{}={}", kv.key, kv.value))
                                        .collect(),
                                )
                            })
                            .map(|p| p.value())
                            .sum(),
                        AggregatedMetrics::U64(MetricData::Sum(sum)) => sum
                            .data_points()
                            .filter(|p| {
                                wanted(
                                    p.attributes()
                                        .map(|kv| format!("{}={}", kv.key, kv.value))
                                        .collect(),
                                )
                            })
                            .map(|p| p.value() as i64)
                            .sum(),
                        _ => 0,
                    }
                })
                .unwrap_or(0)
        };

        let held = turns.take("a").await.unwrap();
        assert_eq!(value("runtara.run_query.running", Some("a")), 1);
        assert_eq!(turns.take("a").await.err().as_deref(), Some(BUSY));
        assert_eq!(value("runtara.run_query.refused", Some("a")), 1);
        assert_eq!(value("runtara.run_query.waiting", None), 0);
        assert_eq!(value("runtara.run_query.limit", None), 3);
        drop(held);
        assert_eq!(value("runtara.run_query.running", None), 0);
        provider.shutdown().unwrap();
    }

    #[test]
    fn containment_nests_the_value() {
        assert_eq!(
            nested(&["a".into(), "b".into()], json!(1)),
            json!({"a": {"b": 1}})
        );
        assert!(indexes(&["items".into(), "0".into()]));
        assert!(indexes(&["items".into(), " 0 ".into()]));
    }

    #[test]
    fn hardening_rejects_duplicates_bad_is_args_and_in_amplification() {
        let checked = |value: Value| build("t", "wf", &query(value)).map(|(q, _, _)| q.into_sql());
        // Duplicate select paths and duplicate aggregate/group names are rejected.
        assert!(checked(json!({"select": ["state", "state"]})).is_err());
        assert!(
            checked(json!({
                "groupBy": ["status"],
                "aggregates": [{"fn": "count", "as": "status"}]
            }))
            .is_err()
        );
        assert!(
            checked(json!({
                "aggregates": [{"fn": "count", "as": ""}]
            }))
            .is_err()
        );
        // Non-boolean second arguments to IS_DEFINED / IS_EMPTY / IS_NOT_EMPTY are rejected.
        for op in ["IS_DEFINED", "IS_EMPTY", "IS_NOT_EMPTY"] {
            assert!(
                checked(json!({"condition": {"op": op, "arguments": ["state.a", "false"]}}))
                    .is_err(),
                "{op}"
            );
            assert!(
                checked(json!({"condition": {"op": op, "arguments": ["state.a", false]}})).is_ok(),
                "{op}"
            );
        }
        // Column IN / NOT_IN accepts null consistently with EQ / NE and JSON IN.
        let sql = checked(json!({"condition": {"op": "IN", "arguments": ["label", ["a", null]]}}))
            .unwrap();
        assert!(sql.contains("r.label IS NULL"), "{sql}");
        let sql =
            checked(json!({"condition": {"op": "IN", "arguments": ["label", [null]]}})).unwrap();
        assert!(sql.contains("r.label IS NULL"), "{sql}");
        // Large IN (> MAX_CONTAINMENT_IN_VALUES) skips OR-ed containment predicates.
        let many: Vec<i64> = (0..64).collect();
        let sql =
            checked(json!({"condition": {"op": "IN", "arguments": ["state.n", many]}})).unwrap();
        assert!(!sql.contains("r.state @>"), "{sql}");
        // Total IN values across the whole condition tree are capped.
        let big: Vec<i64> = (0..MAX_IN_VALUES as i64).collect();
        let five: Vec<Value> = (0..5)
            .map(|_| json!({"op": "IN", "arguments": ["state.n", big]}))
            .collect();
        assert!(checked(json!({"condition": {"op": "AND", "arguments": five}})).is_err());
    }

    #[tokio::test]
    async fn cancelling_a_waiting_query_decrements_the_waiting_metric() {
        use opentelemetry::metrics::MeterProvider;
        use opentelemetry_sdk::metrics::data::{AggregatedMetrics, MetricData};
        use opentelemetry_sdk::metrics::{InMemoryMetricExporter, SdkMeterProvider};

        let exporter = InMemoryMetricExporter::default();
        let provider = SdkMeterProvider::builder()
            .with_periodic_exporter(exporter.clone())
            .build();
        let limits = QueryLimits {
            per_tenant: 1,
            total: 2,
            wait: std::time::Duration::from_secs(5),
        };
        let metrics = QueryMetrics::new(&provider.meter("test"), limits, false);
        let turns = std::sync::Arc::new(Turns::new(limits, Some(metrics)));
        let held = turns.take("a").await.unwrap();

        let waiter = {
            let turns = turns.clone();
            tokio::spawn(async move { turns.take("a").await })
        };
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        waiter.abort();
        let _ = waiter.await;

        provider.force_flush().unwrap();
        let waiting: i64 = exporter
            .get_finished_metrics()
            .unwrap()
            .iter()
            .flat_map(|r| r.scope_metrics())
            .flat_map(|s| s.metrics())
            .filter(|m| m.name() == "runtara.run_query.waiting")
            .last()
            .map(|m| match m.data() {
                AggregatedMetrics::I64(MetricData::Sum(sum)) => {
                    sum.data_points().map(|p| p.value()).sum()
                }
                _ => -1,
            })
            .unwrap_or(-1);
        assert_eq!(waiting, 0, "waiting gauge must return to 0 after abort");
        drop(held);
        provider.shutdown().unwrap();
    }
}
