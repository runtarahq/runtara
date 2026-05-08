use serde_json::Value;
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

use crate::api::dto::object_model::{AggregateRequest, AggregateSpec, Condition};
use crate::api::dto::reports::{ReportOrderBy, ReportSourceJoin};

use super::{ReportServiceError, combine_conditions};

#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
pub(super) enum ReportDiagnosticSeverity {
    Warning,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
pub(super) struct ReportDiagnostic {
    pub severity: ReportDiagnosticSeverity,
    pub block_id: String,
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub(super) struct ReportQueryPlan {
    pub source: ReportSourcePlan,
    pub projections: Vec<ProjectionPlan>,
    pub diagnostics: Vec<ReportDiagnostic>,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub(super) struct ReportSourcePlan {
    pub schema: String,
    pub joins: Vec<JoinPlan>,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub(super) struct JoinPlan {
    pub schema: String,
    pub alias: String,
    pub parent_field: String,
    pub field: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
pub(super) struct ProjectionPlan {
    pub field: String,
}

impl ReportQueryPlan {
    #[cfg(test)]
    fn for_source(schema: impl Into<String>) -> Self {
        Self {
            source: ReportSourcePlan {
                schema: schema.into(),
                joins: Vec::new(),
            },
            projections: Vec::new(),
            diagnostics: Vec::new(),
        }
    }
}

/// Resolved dimension data for a single block-level join. Held in memory
/// during a single aggregate render; sized by `MAX_BROADCAST_JOIN_DIM_ROWS`.
pub(super) struct JoinResolution {
    /// Distinct values of the dim's `field` column. Used to build an
    /// `IN [...]` filter against the primary's `parent_field`.
    pub(super) parent_keys: Vec<Value>,
    /// Lookup of dim row by stringified `field` value. Each row is a
    /// flattened instance map (system fields + properties merged).
    pub(super) by_key: HashMap<String, serde_json::Map<String, Value>>,
}

/// `(primary_condition, terms_grouped_by_alias)` returned by
/// [`split_qualified_condition`].
type SplitCondition = (Option<Condition>, HashMap<String, Vec<Condition>>);

pub(super) fn build_alias_index<'a>(
    joins: &'a [ReportSourceJoin],
    block_id: &str,
) -> Result<HashMap<String, &'a ReportSourceJoin>, ReportServiceError> {
    let mut alias_to_join: HashMap<String, &ReportSourceJoin> = HashMap::new();
    for join in joins {
        let alias = join.effective_alias().to_string();
        if alias.contains('.') || alias.is_empty() {
            return Err(ReportServiceError::Validation(format!(
                "Block '{}' join alias '{}' must be a non-empty identifier without '.'",
                block_id, alias
            )));
        }
        if alias_to_join.insert(alias.clone(), join).is_some() {
            return Err(ReportServiceError::Validation(format!(
                "Block '{}' has duplicate join alias '{}'",
                block_id, alias
            )));
        }
    }
    Ok(alias_to_join)
}

/// Reject features that v1 broadcast-hash join does not yet support:
/// qualified `aggregates[].field`, qualified `orderBy.column`, qualified
/// `groupBy` entries whose join's `parent_field` isn't also in the
/// (unqualified) groupBy.
pub(super) fn validate_join_request(
    request: &AggregateRequest,
    alias_to_join: &HashMap<String, &ReportSourceJoin>,
    block_id: &str,
) -> Result<(), ReportServiceError> {
    for aggregate in &request.aggregates {
        if let Some(column) = aggregate.column.as_deref()
            && let Some(alias) = field_alias_prefix(column)
            && alias_to_join.contains_key(alias)
        {
            return Err(ReportServiceError::Validation(format!(
                "Block '{}' aggregate '{}' references qualified field '{}'; \
                 qualified refs in aggregate.field are not supported in v1.",
                block_id, aggregate.alias, column
            )));
        }
    }

    for order in &request.order_by {
        if let Some(alias) = field_alias_prefix(&order.column)
            && alias_to_join.contains_key(alias)
        {
            return Err(ReportServiceError::Validation(format!(
                "Block '{}' orderBy '{}' references qualified field; \
                 qualified refs in orderBy are not supported in v1. Use \
                 an aggregate alias or an unqualified primary-schema field.",
                block_id, order.column
            )));
        }
    }

    let unqualified_group_by: HashSet<&str> = request
        .group_by
        .iter()
        .filter_map(|field| {
            if field_alias_prefix(field).is_none() {
                Some(field.as_str())
            } else {
                None
            }
        })
        .collect();

    for field in &request.group_by {
        let Some(alias) = field_alias_prefix(field) else {
            continue;
        };
        let Some(join) = alias_to_join.get(alias) else {
            return Err(ReportServiceError::Validation(format!(
                "Block '{}' groupBy field '{}' references unknown alias '{}'",
                block_id, field, alias
            )));
        };
        if !unqualified_group_by.contains(join.parent_field.as_str()) {
            return Err(ReportServiceError::Validation(format!(
                "Block '{}' groupBy uses qualified field '{}' but is missing the \
                 join's parent field '{}' from groupBy. Add '{}' to groupBy so \
                 the dimension can be enriched per row.",
                block_id, field, join.parent_field, join.parent_field
            )));
        }
    }

    Ok(())
}

/// Returns the alias portion of a qualified `<alias>.<field>` reference,
/// or `None` for unqualified names.
pub(super) fn field_alias_prefix(field: &str) -> Option<&str> {
    field.split_once('.').map(|(alias, _)| alias)
}

pub(super) fn split_qualified_condition(
    condition: Option<Condition>,
    alias_set: &HashSet<&str>,
    block_id: &str,
) -> Result<SplitCondition, ReportServiceError> {
    let Some(c) = condition else {
        return Ok((None, HashMap::new()));
    };

    if c.op.eq_ignore_ascii_case("AND") {
        let raw_args = c.arguments.clone().unwrap_or_default();
        let children: Vec<Condition> = raw_args
            .into_iter()
            .filter_map(|v| serde_json::from_value(v).ok())
            .collect();
        let mut primary: Vec<Condition> = Vec::new();
        let mut by_alias: HashMap<String, Vec<Condition>> = HashMap::new();
        for child in children {
            match alias_for_condition(&child, alias_set, block_id)? {
                Some(alias) => by_alias.entry(alias).or_default().push(child),
                None => primary.push(child),
            }
        }
        return Ok((combine_conditions(primary), by_alias));
    }

    match alias_for_condition(&c, alias_set, block_id)? {
        Some(alias) => {
            let mut by_alias = HashMap::new();
            by_alias.insert(alias, vec![c]);
            Ok((None, by_alias))
        }
        None => Ok((Some(c), HashMap::new())),
    }
}

pub(super) fn primary_pushdown_condition(
    condition: Option<Condition>,
    alias_set: &HashSet<&str>,
    block_id: &str,
) -> Result<Option<Condition>, ReportServiceError> {
    let Some(condition) = condition else {
        return Ok(None);
    };
    primary_pushdown_condition_inner(condition, alias_set, block_id)
}

fn primary_pushdown_condition_inner(
    condition: Condition,
    alias_set: &HashSet<&str>,
    block_id: &str,
) -> Result<Option<Condition>, ReportServiceError> {
    if condition.op.eq_ignore_ascii_case("AND") {
        let children = condition
            .arguments
            .unwrap_or_default()
            .into_iter()
            .filter_map(|argument| serde_json::from_value::<Condition>(argument).ok())
            .map(|child| primary_pushdown_condition_inner(child, alias_set, block_id))
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .flatten()
            .collect::<Vec<_>>();
        return Ok(combine_conditions(children));
    }

    if condition_can_run_on_primary(&condition, alias_set, block_id)? {
        Ok(Some(condition))
    } else {
        Ok(None)
    }
}

fn condition_can_run_on_primary(
    condition: &Condition,
    alias_set: &HashSet<&str>,
    block_id: &str,
) -> Result<bool, ReportServiceError> {
    let mut uses_join_alias = false;
    let mut found_unknown: Option<String> = None;
    walk_condition_field_refs(condition, &mut |field| {
        let Some(alias) = field_alias_prefix(field) else {
            return;
        };
        if alias_set.contains(alias) {
            uses_join_alias = true;
        } else if found_unknown.is_none() {
            found_unknown = Some(alias.to_string());
        }
    });

    if let Some(alias) = found_unknown {
        return Err(ReportServiceError::Validation(format!(
            "Block '{}' condition references unknown join alias '{}'",
            block_id, alias
        )));
    }

    Ok(!uses_join_alias)
}

pub(super) fn condition_matches_row(
    condition: &Condition,
    row: &serde_json::Map<String, Value>,
    block_id: &str,
) -> Result<bool, ReportServiceError> {
    let op = condition.op.to_ascii_uppercase();
    let args = condition.arguments.as_deref().unwrap_or(&[]);
    match op.as_str() {
        "AND" => {
            for argument in args {
                let child = condition_child(argument, block_id)?;
                if !condition_matches_row(&child, row, block_id)? {
                    return Ok(false);
                }
            }
            Ok(true)
        }
        "OR" => {
            for argument in args {
                let child = condition_child(argument, block_id)?;
                if condition_matches_row(&child, row, block_id)? {
                    return Ok(true);
                }
            }
            Ok(false)
        }
        "NOT" => {
            let Some(argument) = args.first() else {
                return Err(ReportServiceError::Validation(format!(
                    "Block '{}' NOT condition requires one argument",
                    block_id
                )));
            };
            let child = condition_child(argument, block_id)?;
            Ok(!condition_matches_row(&child, row, block_id)?)
        }
        "EQ" => compare_binary(args, row, block_id, |ordering, equal| {
            equal || ordering == Some(Ordering::Equal)
        }),
        "NE" => compare_binary(args, row, block_id, |ordering, equal| {
            !(equal || ordering == Some(Ordering::Equal))
        }),
        "GT" => compare_binary(args, row, block_id, |ordering, _| {
            ordering == Some(Ordering::Greater)
        }),
        "GTE" => compare_binary(args, row, block_id, |ordering, equal| {
            equal || matches!(ordering, Some(Ordering::Greater | Ordering::Equal))
        }),
        "LT" => compare_binary(args, row, block_id, |ordering, _| {
            ordering == Some(Ordering::Less)
        }),
        "LTE" => compare_binary(args, row, block_id, |ordering, equal| {
            equal || matches!(ordering, Some(Ordering::Less | Ordering::Equal))
        }),
        "IN" | "NOT_IN" => {
            if args.len() != 2 {
                return Err(ReportServiceError::Validation(format!(
                    "Block '{}' {} condition requires two arguments",
                    block_id, condition.op
                )));
            }
            let left = operand_value(&args[0], row, true);
            let values = args[1].as_array().ok_or_else(|| {
                ReportServiceError::Validation(format!(
                    "Block '{}' {} condition second argument must be an array",
                    block_id, condition.op
                ))
            })?;
            let contains = values.iter().any(|value| values_equal(&left, value));
            Ok(if op == "IN" { contains } else { !contains })
        }
        "CONTAINS" => {
            if args.len() != 2 {
                return Err(ReportServiceError::Validation(format!(
                    "Block '{}' CONTAINS condition requires two arguments",
                    block_id
                )));
            }
            let left = operand_value(&args[0], row, true);
            let needle = args[1].as_str().ok_or_else(|| {
                ReportServiceError::Validation(format!(
                    "Block '{}' CONTAINS condition second argument must be a string",
                    block_id
                ))
            })?;
            Ok(left
                .as_str()
                .is_some_and(|haystack| haystack.contains(needle)))
        }
        "IS_DEFINED" => unary_field(args, row, block_id, |value| !value.is_null()),
        "IS_EMPTY" => unary_field(args, row, block_id, row_value_is_empty),
        "IS_NOT_EMPTY" => unary_field(args, row, block_id, |value| !row_value_is_empty(value)),
        _ => Err(ReportServiceError::Validation(format!(
            "Block '{}' joined table post-filter does not support condition op '{}'",
            block_id, condition.op
        ))),
    }
}

fn condition_child(argument: &Value, block_id: &str) -> Result<Condition, ReportServiceError> {
    serde_json::from_value::<Condition>(argument.clone()).map_err(|err| {
        ReportServiceError::Validation(format!(
            "Block '{}' condition child is invalid: {}",
            block_id, err
        ))
    })
}

fn compare_binary(
    args: &[Value],
    row: &serde_json::Map<String, Value>,
    block_id: &str,
    predicate: impl FnOnce(Option<Ordering>, bool) -> bool,
) -> Result<bool, ReportServiceError> {
    if args.len() != 2 {
        return Err(ReportServiceError::Validation(format!(
            "Block '{}' comparison condition requires two arguments",
            block_id
        )));
    }
    let left = operand_value(&args[0], row, true);
    let right = operand_value(&args[1], row, false);
    Ok(predicate(
        compare_values(&left, &right),
        values_equal(&left, &right),
    ))
}

fn unary_field(
    args: &[Value],
    row: &serde_json::Map<String, Value>,
    block_id: &str,
    predicate: impl FnOnce(&Value) -> bool,
) -> Result<bool, ReportServiceError> {
    if args.len() != 1 {
        return Err(ReportServiceError::Validation(format!(
            "Block '{}' unary condition requires one argument",
            block_id
        )));
    }
    Ok(predicate(&operand_value(&args[0], row, true)))
}

fn operand_value(argument: &Value, row: &serde_json::Map<String, Value>, field_ref: bool) -> Value {
    if field_ref && let Some(field) = argument.as_str() {
        return row_value(row, field).cloned().unwrap_or(Value::Null);
    }
    argument.clone()
}

fn row_value<'a>(row: &'a serde_json::Map<String, Value>, field: &str) -> Option<&'a Value> {
    if let Some(value) = row.get(field) {
        return Some(value);
    }

    let mut parts = field.split('.');
    let first = parts.next()?;
    let mut current = row.get(first)?;
    for part in parts {
        current = match current {
            Value::Object(object) => object.get(part)?,
            Value::Array(values) => {
                let index = part.parse::<usize>().ok()?;
                values.get(index)?
            }
            _ => return None,
        };
    }
    Some(current)
}

pub(super) fn sort_rows(rows: &mut [serde_json::Map<String, Value>], sort: &[ReportOrderBy]) {
    if sort.is_empty() {
        return;
    }
    rows.sort_by(|left, right| compare_rows(left, right, sort));
}

fn compare_rows(
    left: &serde_json::Map<String, Value>,
    right: &serde_json::Map<String, Value>,
    sort: &[ReportOrderBy],
) -> Ordering {
    for entry in sort {
        let left_value = row_value(left, &entry.field).unwrap_or(&Value::Null);
        let right_value = row_value(right, &entry.field).unwrap_or(&Value::Null);
        let ordering = compare_values(left_value, right_value).unwrap_or(Ordering::Equal);
        let ordering = if entry.direction.eq_ignore_ascii_case("desc") {
            ordering.reverse()
        } else {
            ordering
        };
        if ordering != Ordering::Equal {
            return ordering;
        }
    }
    Ordering::Equal
}

fn compare_values(left: &Value, right: &Value) -> Option<Ordering> {
    match (left, right) {
        (Value::Null, Value::Null) => Some(Ordering::Equal),
        (Value::Null, _) => Some(Ordering::Greater),
        (_, Value::Null) => Some(Ordering::Less),
        (Value::Number(left), Value::Number(right)) => left.as_f64()?.partial_cmp(&right.as_f64()?),
        (Value::String(left), Value::String(right)) => Some(left.cmp(right)),
        (Value::Bool(left), Value::Bool(right)) => Some(left.cmp(right)),
        _ => Some(value_sort_key(left).cmp(&value_sort_key(right))),
    }
}

fn values_equal(left: &Value, right: &Value) -> bool {
    match (left, right) {
        (Value::Number(_), Value::Number(_)) => {
            compare_values(left, right) == Some(Ordering::Equal)
        }
        _ => left == right,
    }
}

fn value_sort_key(value: &Value) -> String {
    match value {
        Value::String(value) => value.clone(),
        Value::Bool(value) => value.to_string(),
        Value::Number(value) => value.to_string(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

fn row_value_is_empty(value: &Value) -> bool {
    match value {
        Value::Null => true,
        Value::String(value) => value.trim().is_empty(),
        Value::Array(values) => values.is_empty(),
        Value::Object(values) => values.is_empty(),
        _ => false,
    }
}

/// Identify the join alias used by a single condition term, if any. Rejects
/// terms that would mix references across a join boundary or against an
/// unknown alias — both indicate caller error.
fn alias_for_condition(
    c: &Condition,
    alias_set: &HashSet<&str>,
    block_id: &str,
) -> Result<Option<String>, ReportServiceError> {
    let mut found_alias: Option<String> = None;
    let mut found_unknown: Option<String> = None;
    walk_condition_field_refs(c, &mut |field| {
        let Some(alias) = field_alias_prefix(field) else {
            return;
        };
        if alias_set.contains(alias) {
            if found_alias.as_deref() != Some(alias) {
                if found_alias.is_some() {
                    found_alias = Some(format!("__multiple__:{}", alias));
                } else {
                    found_alias = Some(alias.to_string());
                }
            }
        } else if found_unknown.is_none() {
            found_unknown = Some(alias.to_string());
        }
    });

    if let Some(alias) = found_unknown {
        return Err(ReportServiceError::Validation(format!(
            "Block '{}' condition references unknown join alias '{}'",
            block_id, alias
        )));
    }
    if let Some(alias) = &found_alias
        && alias.starts_with("__multiple__:")
    {
        return Err(ReportServiceError::Validation(format!(
            "Block '{}' condition mixes references across joins ({}); v1 \
             requires each AND'd term to reference at most one schema.",
            block_id, alias
        )));
    }
    Ok(found_alias)
}

/// Visit every string-valued field reference in a condition tree.
/// Field refs in this DSL are always the first argument of a binary op or
/// the field arg of IN/NOT_IN — checking arg[0] of every node covers them
/// without rebuilding the operator catalog.
fn walk_condition_field_refs(c: &Condition, visit: &mut impl FnMut(&str)) {
    let Some(args) = &c.arguments else {
        return;
    };
    for (index, arg) in args.iter().enumerate() {
        if index == 0
            && let Some(s) = arg.as_str()
        {
            visit(s);
        }
        if let Ok(child) = serde_json::from_value::<Condition>(arg.clone()) {
            walk_condition_field_refs(&child, visit);
        }
    }
}

pub(super) fn strip_alias_from_condition(mut c: Condition, alias: &str) -> Condition {
    let prefix = format!("{}.", alias);
    if let Some(args) = &mut c.arguments {
        for (index, arg) in args.iter_mut().enumerate() {
            if index == 0
                && let Some(s) = arg.as_str()
                && let Some(unqualified) = s.strip_prefix(&prefix)
            {
                *arg = Value::String(unqualified.to_string());
            }
            if let Ok(child) = serde_json::from_value::<Condition>(arg.clone()) {
                let stripped = strip_alias_from_condition(child, alias);
                if let Ok(stripped_value) = serde_json::to_value(stripped) {
                    *arg = stripped_value;
                }
            }
        }
    }
    c
}

/// Stringify a JSON value for use as a HashMap lookup key. Two JSON values
/// that compare equal under SQL semantics produce the same key.
pub(super) fn value_to_lookup_key(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// Build the empty result when an inner join has no matching dimension rows
/// — preserving the column shape the caller requested so downstream code
/// (table projection, chart rendering) stays uniform.
pub(super) fn empty_join_result(
    group_by: &[String],
    aggregates: &[AggregateSpec],
) -> runtara_object_store::AggregateResult {
    let mut columns = group_by.to_vec();
    columns.extend(aggregates.iter().map(|a| a.alias.clone()));
    runtara_object_store::AggregateResult {
        columns,
        rows: Vec::new(),
        group_count: 0,
    }
}

/// Take the primary aggregate result and add columns sourced from the
/// joined dimensions. Output column order matches the original groupBy
/// order, with aggregate columns appended at the end.
pub(super) fn enrich_aggregate_result(
    primary: runtara_object_store::AggregateResult,
    requested_group_by: &[String],
    alias_to_join: &HashMap<String, &ReportSourceJoin>,
    join_data: &HashMap<String, JoinResolution>,
) -> runtara_object_store::AggregateResult {
    let primary_index: HashMap<&str, usize> = primary
        .columns
        .iter()
        .enumerate()
        .map(|(i, c)| (c.as_str(), i))
        .collect();

    let aggregate_aliases: Vec<&str> = primary
        .columns
        .iter()
        .map(String::as_str)
        .filter(|c| !requested_group_by.iter().any(|g| g == c))
        .collect();

    let mut output_columns: Vec<String> = requested_group_by.to_vec();
    for alias in &aggregate_aliases {
        output_columns.push(alias.to_string());
    }

    let mut output_rows: Vec<Vec<Value>> = Vec::with_capacity(primary.rows.len());
    for row in &primary.rows {
        let mut new_row: Vec<Value> = Vec::with_capacity(output_columns.len());

        for field in requested_group_by {
            if let Some(alias) = field_alias_prefix(field) {
                let Some(join) = alias_to_join.get(alias) else {
                    new_row.push(Value::Null);
                    continue;
                };
                let Some(parent_index) = primary_index.get(join.parent_field.as_str()) else {
                    new_row.push(Value::Null);
                    continue;
                };
                let Some(parent_value) = row.get(*parent_index) else {
                    new_row.push(Value::Null);
                    continue;
                };
                let key = value_to_lookup_key(parent_value);
                let dim_field = field
                    .strip_prefix(&format!("{}.", alias))
                    .unwrap_or(field.as_str());
                let dim_row = join_data.get(alias).and_then(|data| data.by_key.get(&key));
                let cell = dim_row
                    .and_then(|row_map| row_map.get(dim_field))
                    .cloned()
                    .unwrap_or(Value::Null);
                new_row.push(cell);
            } else {
                let value = primary_index
                    .get(field.as_str())
                    .and_then(|i| row.get(*i))
                    .cloned()
                    .unwrap_or(Value::Null);
                new_row.push(value);
            }
        }

        for alias in &aggregate_aliases {
            let value = primary_index
                .get(alias)
                .and_then(|i| row.get(*i))
                .cloned()
                .unwrap_or(Value::Null);
            new_row.push(value);
        }

        output_rows.push(new_row);
    }

    let group_count = primary.group_count;
    runtara_object_store::AggregateResult {
        columns: output_columns,
        rows: output_rows,
        group_count,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::dto::object_model::{AggregateFn, AggregateOrderBy, SortDirection};
    use crate::api::dto::reports::ReportJoinKind;
    use serde_json::json;

    fn cond(op: &str, args: Vec<Value>) -> Condition {
        Condition {
            op: op.to_string(),
            arguments: Some(args),
        }
    }

    fn product_join() -> ReportSourceJoin {
        ReportSourceJoin {
            schema: "TDProduct".to_string(),
            alias: Some("p".to_string()),
            connection_id: None,
            field: "sku".to_string(),
            parent_field: "sku".to_string(),
            op: "eq".to_string(),
            kind: ReportJoinKind::Inner,
        }
    }

    #[test]
    fn planner_structs_hold_source_projection_and_diagnostics() {
        let mut plan = ReportQueryPlan::for_source("StockSnapshot");
        plan.source.joins.push(JoinPlan {
            schema: "TDProduct".to_string(),
            alias: "p".to_string(),
            parent_field: "sku".to_string(),
            field: "sku".to_string(),
        });
        plan.projections.push(ProjectionPlan {
            field: "p.part_number".to_string(),
        });
        plan.diagnostics.push(ReportDiagnostic {
            severity: ReportDiagnosticSeverity::Warning,
            block_id: "stock".to_string(),
            code: "JOIN_BROADCAST".to_string(),
            message: "Using bounded broadcast join".to_string(),
        });

        assert_eq!(plan.source.schema, "StockSnapshot");
        assert_eq!(plan.source.joins[0].schema, "TDProduct");
        assert_eq!(plan.source.joins[0].alias, "p");
        assert_eq!(plan.source.joins[0].parent_field, "sku");
        assert_eq!(plan.source.joins[0].field, "sku");
        assert_eq!(plan.projections[0].field, "p.part_number");
        assert_eq!(
            plan.diagnostics[0].severity,
            ReportDiagnosticSeverity::Warning
        );
    }

    #[test]
    fn split_qualified_condition_separates_aliased_from_primary_terms() {
        let aliases: HashSet<&str> = ["p"].into_iter().collect();
        let condition = cond(
            "AND",
            vec![
                serde_json::to_value(cond("EQ", vec![json!("status"), json!("active")])).unwrap(),
                serde_json::to_value(cond(
                    "IN",
                    vec![json!("p.category_leaf_id"), json!([1, 2, 3])],
                ))
                .unwrap(),
            ],
        );

        let (primary, by_alias) =
            split_qualified_condition(Some(condition), &aliases, "block").unwrap();

        let primary = primary.expect("primary should retain the unqualified term");
        assert_eq!(primary.op, "EQ");
        assert_eq!(primary.arguments.unwrap()[0].as_str().unwrap(), "status");

        let alias_terms = by_alias.get("p").expect("alias bucket present");
        assert_eq!(alias_terms.len(), 1);
        assert_eq!(alias_terms[0].op, "IN");
    }

    #[test]
    fn split_qualified_condition_rejects_unknown_alias() {
        let aliases: HashSet<&str> = ["p"].into_iter().collect();
        let condition = cond("EQ", vec![json!("q.foo"), json!("bar")]);
        let err = split_qualified_condition(Some(condition), &aliases, "block").unwrap_err();
        assert!(err.to_string().contains("unknown join alias 'q'"));
    }

    #[test]
    fn primary_pushdown_keeps_only_safe_and_terms() {
        let aliases: HashSet<&str> = ["p"].into_iter().collect();
        let condition = cond(
            "AND",
            vec![
                serde_json::to_value(cond("EQ", vec![json!("status"), json!("active")])).unwrap(),
                serde_json::to_value(cond("EQ", vec![json!("p.category"), json!("network")]))
                    .unwrap(),
            ],
        );

        let pushed = primary_pushdown_condition(Some(condition), &aliases, "block")
            .unwrap()
            .unwrap();

        assert_eq!(pushed.op, "EQ");
        assert_eq!(
            pushed.arguments.unwrap(),
            vec![json!("status"), json!("active")]
        );
    }

    #[test]
    fn primary_pushdown_drops_or_with_join_terms() {
        let aliases: HashSet<&str> = ["p"].into_iter().collect();
        let condition = cond(
            "OR",
            vec![
                serde_json::to_value(cond("EQ", vec![json!("status"), json!("active")])).unwrap(),
                serde_json::to_value(cond("EQ", vec![json!("p.category"), json!("network")]))
                    .unwrap(),
            ],
        );

        let pushed = primary_pushdown_condition(Some(condition), &aliases, "block").unwrap();

        assert!(pushed.is_none());
    }

    #[test]
    fn condition_matches_row_handles_qualified_or_and_not_terms() {
        let row = serde_json::Map::from_iter([
            ("status".to_string(), json!("inactive")),
            ("p.category".to_string(), json!("network")),
            ("qty".to_string(), json!(10)),
        ]);
        let condition = cond(
            "AND",
            vec![
                serde_json::to_value(cond(
                    "OR",
                    vec![
                        serde_json::to_value(cond("EQ", vec![json!("status"), json!("active")]))
                            .unwrap(),
                        serde_json::to_value(cond(
                            "EQ",
                            vec![json!("p.category"), json!("network")],
                        ))
                        .unwrap(),
                    ],
                ))
                .unwrap(),
                serde_json::to_value(cond(
                    "NOT",
                    vec![serde_json::to_value(cond("LT", vec![json!("qty"), json!(1)])).unwrap()],
                ))
                .unwrap(),
            ],
        );

        assert!(condition_matches_row(&condition, &row, "block").unwrap());
    }

    #[test]
    fn condition_matches_row_resolves_nested_virtual_fields() {
        let row = serde_json::Map::from_iter([
            ("actionKey".to_string(), json!("case_review_decision")),
            (
                "correlation".to_string(),
                json!({"case_id": "case-76", "attempts": [1, 2]}),
            ),
        ]);
        let condition = cond(
            "AND",
            vec![
                serde_json::to_value(cond(
                    "EQ",
                    vec![json!("actionKey"), json!("case_review_decision")],
                ))
                .unwrap(),
                serde_json::to_value(cond(
                    "EQ",
                    vec![json!("correlation.case_id"), json!("case-76")],
                ))
                .unwrap(),
                serde_json::to_value(cond("EQ", vec![json!("correlation.attempts.1"), json!(2)]))
                    .unwrap(),
            ],
        );

        assert!(condition_matches_row(&condition, &row, "block").unwrap());
    }

    #[test]
    fn strip_alias_from_condition_removes_qualifier() {
        let stripped = strip_alias_from_condition(
            cond("IN", vec![json!("p.category_leaf_id"), json!([1, 2, 3])]),
            "p",
        );
        assert_eq!(
            stripped.arguments.unwrap()[0].as_str().unwrap(),
            "category_leaf_id"
        );
    }

    #[test]
    fn enrich_aggregate_result_appends_dim_columns_in_groupby_order() {
        let primary = runtara_object_store::AggregateResult {
            columns: vec!["sku".to_string(), "delta".to_string()],
            rows: vec![
                vec![json!("ABC-1"), json!(5)],
                vec![json!("ABC-2"), json!(-3)],
            ],
            group_count: 2,
        };

        let join = product_join();
        let joins = [join];
        let alias_to_join: HashMap<String, &ReportSourceJoin> =
            joins.iter().map(|j| ("p".to_string(), j)).collect();

        let mut by_key: HashMap<String, serde_json::Map<String, Value>> = HashMap::new();
        let mut row1 = serde_json::Map::new();
        row1.insert("sku".to_string(), json!("ABC-1"));
        row1.insert("vendor".to_string(), json!("HPE"));
        row1.insert("part_number".to_string(), json!("X-1"));
        by_key.insert("ABC-1".to_string(), row1);
        let mut row2 = serde_json::Map::new();
        row2.insert("sku".to_string(), json!("ABC-2"));
        row2.insert("vendor".to_string(), json!("Cisco"));
        row2.insert("part_number".to_string(), json!("Y-2"));
        by_key.insert("ABC-2".to_string(), row2);

        let mut join_data: HashMap<String, JoinResolution> = HashMap::new();
        join_data.insert(
            "p".to_string(),
            JoinResolution {
                parent_keys: vec![json!("ABC-1"), json!("ABC-2")],
                by_key,
            },
        );

        let requested_group_by = vec![
            "sku".to_string(),
            "p.part_number".to_string(),
            "p.vendor".to_string(),
        ];

        let enriched =
            enrich_aggregate_result(primary, &requested_group_by, &alias_to_join, &join_data);

        assert_eq!(
            enriched.columns,
            vec![
                "sku".to_string(),
                "p.part_number".to_string(),
                "p.vendor".to_string(),
                "delta".to_string(),
            ]
        );
        assert_eq!(enriched.rows.len(), 2);
        assert_eq!(enriched.rows[0][0], json!("ABC-1"));
        assert_eq!(enriched.rows[0][1], json!("X-1"));
        assert_eq!(enriched.rows[0][2], json!("HPE"));
        assert_eq!(enriched.rows[0][3], json!(5));
        assert_eq!(enriched.rows[1][2], json!("Cisco"));
    }

    #[test]
    fn validate_join_request_rejects_qualified_aggregate_field() {
        let join = product_join();
        let joins = [join];
        let alias_to_join: HashMap<String, &ReportSourceJoin> =
            joins.iter().map(|j| ("p".to_string(), j)).collect();

        let request = AggregateRequest {
            condition: None,
            group_by: vec!["sku".to_string()],
            aggregates: vec![AggregateSpec {
                alias: "vendor_sample".to_string(),
                fn_: AggregateFn::Max,
                column: Some("p.vendor".to_string()),
                distinct: false,
                order_by: vec![],
                expression: None,
                percentile: None,
            }],
            order_by: vec![],
            limit: Some(10),
            offset: Some(0),
        };
        let err = validate_join_request(&request, &alias_to_join, "block").unwrap_err();
        assert!(
            err.to_string()
                .contains("qualified refs in aggregate.field are not supported")
        );
    }

    #[test]
    fn validate_join_request_rejects_qualified_order_by() {
        let join = product_join();
        let joins = [join];
        let alias_to_join: HashMap<String, &ReportSourceJoin> =
            joins.iter().map(|j| ("p".to_string(), j)).collect();

        let request = AggregateRequest {
            condition: None,
            group_by: vec!["sku".to_string()],
            aggregates: vec![AggregateSpec {
                alias: "n".to_string(),
                fn_: AggregateFn::Count,
                column: None,
                distinct: false,
                order_by: vec![],
                expression: None,
                percentile: None,
            }],
            order_by: vec![AggregateOrderBy {
                column: "p.vendor".to_string(),
                direction: SortDirection::Asc,
            }],
            limit: Some(10),
            offset: Some(0),
        };
        let err = validate_join_request(&request, &alias_to_join, "block").unwrap_err();
        assert!(
            err.to_string()
                .contains("qualified refs in orderBy are not supported")
        );
    }

    #[test]
    fn validate_join_request_requires_parent_field_in_groupby_when_qualified_used() {
        let join = product_join();
        let joins = [join];
        let alias_to_join: HashMap<String, &ReportSourceJoin> =
            joins.iter().map(|j| ("p".to_string(), j)).collect();

        let request = AggregateRequest {
            condition: None,
            group_by: vec!["p.vendor".to_string()],
            aggregates: vec![AggregateSpec {
                alias: "n".to_string(),
                fn_: AggregateFn::Count,
                column: None,
                distinct: false,
                order_by: vec![],
                expression: None,
                percentile: None,
            }],
            order_by: vec![],
            limit: Some(10),
            offset: Some(0),
        };
        let err = validate_join_request(&request, &alias_to_join, "block").unwrap_err();
        assert!(err.to_string().contains("parent field 'sku'"));
    }
}
