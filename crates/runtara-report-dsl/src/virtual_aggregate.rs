//! In-memory aggregate engine for report sources without storage-level
//! pushdown (system + workflow_runtime).
//!
//! The engine takes a row set (already filtered/fetched by the caller) and
//! produces an [`AggregateResult`] using the canonical
//! [`runtara_object_store`] aggregate request shape. Condition matching is
//! delegated through a caller-supplied closure so the engine doesn't need
//! to know how the caller spells errors or which `Condition` variant the
//! request uses.
//!
//! Available only when the `aggregate` feature is enabled. The WASM build
//! disables this feature — the FE has no consumer for the engine (it only
//! renders templates and evaluates row conditions).

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

use serde_json::{Map, Value, json};

pub use runtara_object_store::{
    AggregateFn, AggregateOrderBy, AggregateRequest, AggregateResult, AggregateSpec, Condition,
    SortDirection,
};

/// Cap on the number of output groups per call, mirrors the SQL pushdown limit.
pub const MAX_AGGREGATE_ROWS: i64 = 1000;

/// Error returned by the engine. Errors are stringly-typed so callers map
/// them into whatever error enum they use.
#[derive(Debug, Clone)]
pub struct AggregateError(pub String);

impl std::fmt::Display for AggregateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for AggregateError {}

impl From<String> for AggregateError {
    fn from(value: String) -> Self {
        Self(value)
    }
}

/// Aggregate `rows` according to `request`.
///
/// `condition_matcher` is invoked per row when `request.condition` is set;
/// it should return `Ok(true)` if the row matches. Server callers pass a
/// closure that wraps their `condition_matches_row` helper.
pub fn aggregate_virtual_rows<F>(
    block_id: &str,
    rows: &[Map<String, Value>],
    request: AggregateRequest,
    condition_matcher: F,
) -> Result<AggregateResult, AggregateError>
where
    F: Fn(&Condition, &Map<String, Value>, &str) -> Result<bool, String>,
{
    if request.aggregates.is_empty() {
        return Err(AggregateError(format!(
            "Block '{}' must define at least one aggregate",
            block_id
        )));
    }

    let row_refs = rows
        .iter()
        .filter_map(|row| match request.condition.as_ref() {
            Some(condition) => match condition_matcher(condition, row, block_id) {
                Ok(true) => Some(Ok(row)),
                Ok(false) => None,
                Err(error) => Some(Err(AggregateError(error))),
            },
            None => Some(Ok(row)),
        })
        .collect::<Result<Vec<_>, _>>()?;

    let mut groups: Vec<VirtualAggregateGroup<'_>> = Vec::new();
    if request.group_by.is_empty() {
        groups.push(VirtualAggregateGroup {
            values: Vec::new(),
            rows: row_refs,
        });
    } else {
        let mut group_index_by_key = HashMap::new();
        for row in row_refs {
            let values = request
                .group_by
                .iter()
                .map(|field| {
                    virtual_row_value(row, field)
                        .cloned()
                        .unwrap_or(Value::Null)
                })
                .collect::<Vec<_>>();
            let key = value_to_lookup_key(&Value::Array(values.clone()));
            let index = if let Some(index) = group_index_by_key.get(&key) {
                *index
            } else {
                let index = groups.len();
                group_index_by_key.insert(key, index);
                groups.push(VirtualAggregateGroup {
                    values,
                    rows: Vec::new(),
                });
                index
            };
            groups[index].rows.push(row);
        }
    }

    let mut columns = request.group_by.clone();
    columns.extend(
        request
            .aggregates
            .iter()
            .map(|aggregate| aggregate.alias.clone()),
    );

    let mut output_rows = Vec::with_capacity(groups.len());
    for group in groups {
        let mut row = group.values.clone();
        let mut aliases = HashMap::new();
        for aggregate in &request.aggregates {
            let value = virtual_aggregate_value(block_id, aggregate, &group.rows, &aliases)?;
            aliases.insert(aggregate.alias.clone(), value.clone());
            row.push(value);
        }
        output_rows.push(row);
    }

    let group_count = output_rows.len() as i64;
    sort_virtual_aggregate_rows(&mut output_rows, &columns, &request)?;
    let offset = request.offset.unwrap_or(0).max(0) as usize;
    let rows = output_rows
        .into_iter()
        .skip(offset)
        .take(
            request
                .limit
                .map(|limit| limit.clamp(1, MAX_AGGREGATE_ROWS) as usize)
                .unwrap_or(usize::MAX),
        )
        .collect();

    Ok(AggregateResult {
        columns,
        rows,
        group_count,
    })
}

/// Stringify a JSON value for use as a HashMap lookup key. Two JSON values
/// that compare equal under SQL semantics produce the same key.
pub fn value_to_lookup_key(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

struct VirtualAggregateGroup<'a> {
    values: Vec<Value>,
    rows: Vec<&'a Map<String, Value>>,
}

fn virtual_aggregate_value(
    block_id: &str,
    aggregate: &AggregateSpec,
    rows: &[&Map<String, Value>],
    aliases: &HashMap<String, Value>,
) -> Result<Value, AggregateError> {
    match aggregate.fn_ {
        AggregateFn::Count => virtual_count_value(block_id, aggregate, rows),
        AggregateFn::Sum => {
            let values = virtual_numeric_values(block_id, aggregate, rows)?;
            if values.is_empty() {
                Ok(Value::Null)
            } else {
                Ok(f64_value(values.iter().sum()))
            }
        }
        AggregateFn::Avg => {
            let values = virtual_numeric_values(block_id, aggregate, rows)?;
            if values.is_empty() {
                Ok(Value::Null)
            } else {
                Ok(f64_value(values.iter().sum::<f64>() / values.len() as f64))
            }
        }
        AggregateFn::Min | AggregateFn::Max => virtual_min_max_value(block_id, aggregate, rows),
        AggregateFn::FirstValue | AggregateFn::LastValue => {
            virtual_first_last_value(block_id, aggregate, rows)
        }
        AggregateFn::PercentileCont | AggregateFn::PercentileDisc => {
            virtual_percentile_value(block_id, aggregate, rows)
        }
        AggregateFn::StddevSamp | AggregateFn::VarSamp => {
            virtual_sample_stat_value(block_id, aggregate, rows)
        }
        AggregateFn::Expr => {
            let expression = aggregate.expression.as_ref().ok_or_else(|| {
                AggregateError(format!(
                    "Block '{}' aggregate '{}' expression is required for expr",
                    block_id, aggregate.alias
                ))
            })?;
            evaluate_virtual_expression(block_id, expression, aliases)
        }
    }
}

fn virtual_count_value(
    block_id: &str,
    aggregate: &AggregateSpec,
    rows: &[&Map<String, Value>],
) -> Result<Value, AggregateError> {
    if aggregate.distinct {
        let column = aggregate.column.as_deref().ok_or_else(|| {
            AggregateError(format!(
                "Block '{}' aggregate '{}' distinct count requires field",
                block_id, aggregate.alias
            ))
        })?;
        let mut values = HashSet::new();
        for row in rows {
            let value = virtual_row_value(row, column).unwrap_or(&Value::Null);
            if !value.is_null() {
                values.insert(value_to_lookup_key(value));
            }
        }
        return Ok(json!(values.len()));
    }

    if let Some(column) = aggregate.column.as_deref() {
        Ok(json!(
            rows.iter()
                .filter(|row| {
                    virtual_row_value(row, column).is_some_and(|value| !value.is_null())
                })
                .count()
        ))
    } else {
        Ok(json!(rows.len()))
    }
}

fn virtual_numeric_values(
    block_id: &str,
    aggregate: &AggregateSpec,
    rows: &[&Map<String, Value>],
) -> Result<Vec<f64>, AggregateError> {
    let column = required_aggregate_column(block_id, aggregate)?;
    Ok(rows
        .iter()
        .filter_map(|row| virtual_row_value(row, column).and_then(Value::as_f64))
        .collect())
}

fn virtual_min_max_value(
    block_id: &str,
    aggregate: &AggregateSpec,
    rows: &[&Map<String, Value>],
) -> Result<Value, AggregateError> {
    let column = required_aggregate_column(block_id, aggregate)?;
    let mut values = rows
        .iter()
        .filter_map(|row| virtual_row_value(row, column))
        .filter(|value| !value.is_null());
    let Some(first) = values.next() else {
        return Ok(Value::Null);
    };
    let mut selected = first.clone();
    for value in values {
        let ordering = virtual_compare_values(value, &selected).unwrap_or(Ordering::Equal);
        let should_replace = match aggregate.fn_ {
            AggregateFn::Min => ordering == Ordering::Less,
            AggregateFn::Max => ordering == Ordering::Greater,
            _ => false,
        };
        if should_replace {
            selected = value.clone();
        }
    }
    Ok(selected)
}

fn virtual_first_last_value(
    block_id: &str,
    aggregate: &AggregateSpec,
    rows: &[&Map<String, Value>],
) -> Result<Value, AggregateError> {
    let column = required_aggregate_column(block_id, aggregate)?;
    if aggregate.order_by.is_empty() {
        return Err(AggregateError(format!(
            "Block '{}' aggregate '{}' first_value/last_value requires orderBy",
            block_id, aggregate.alias
        )));
    }
    let mut sorted = rows.to_vec();
    let flip = matches!(aggregate.fn_, AggregateFn::LastValue);
    sorted.sort_by(|left, right| {
        compare_virtual_rows_by_order(left, right, &aggregate.order_by, flip)
    });
    Ok(sorted
        .first()
        .and_then(|row| virtual_row_value(row, column))
        .cloned()
        .unwrap_or(Value::Null))
}

fn virtual_percentile_value(
    block_id: &str,
    aggregate: &AggregateSpec,
    rows: &[&Map<String, Value>],
) -> Result<Value, AggregateError> {
    required_aggregate_column(block_id, aggregate)?;
    let percentile = aggregate.percentile.ok_or_else(|| {
        AggregateError(format!(
            "Block '{}' aggregate '{}' percentile is required",
            block_id, aggregate.alias
        ))
    })?;
    if !(0.0..=1.0).contains(&percentile) {
        return Err(AggregateError(format!(
            "Block '{}' aggregate '{}' percentile must be between 0 and 1",
            block_id, aggregate.alias
        )));
    }
    let order_by = aggregate.order_by.first().ok_or_else(|| {
        AggregateError(format!(
            "Block '{}' aggregate '{}' percentile requires orderBy",
            block_id, aggregate.alias
        ))
    })?;
    let mut values = rows
        .iter()
        .filter_map(|row| virtual_row_value(row, &order_by.column).and_then(Value::as_f64))
        .collect::<Vec<_>>();
    values.sort_by(|left, right| left.partial_cmp(right).unwrap_or(Ordering::Equal));
    if order_by.direction == SortDirection::Desc {
        values.reverse();
    }
    if values.is_empty() {
        return Ok(Value::Null);
    }
    if matches!(aggregate.fn_, AggregateFn::PercentileDisc) {
        let index = ((percentile * values.len() as f64).ceil() as usize)
            .saturating_sub(1)
            .min(values.len() - 1);
        return Ok(f64_value(values[index]));
    }
    if values.len() == 1 {
        return Ok(f64_value(values[0]));
    }
    let rank = percentile * (values.len() - 1) as f64;
    let lower_index = rank.floor() as usize;
    let upper_index = rank.ceil() as usize;
    if lower_index == upper_index {
        return Ok(f64_value(values[lower_index]));
    }
    let fraction = rank - lower_index as f64;
    Ok(f64_value(
        values[lower_index] + ((values[upper_index] - values[lower_index]) * fraction),
    ))
}

fn virtual_sample_stat_value(
    block_id: &str,
    aggregate: &AggregateSpec,
    rows: &[&Map<String, Value>],
) -> Result<Value, AggregateError> {
    let values = virtual_numeric_values(block_id, aggregate, rows)?;
    if values.len() < 2 {
        return Ok(Value::Null);
    }
    let mean = values.iter().sum::<f64>() / values.len() as f64;
    let variance = values
        .iter()
        .map(|value| (value - mean).powi(2))
        .sum::<f64>()
        / (values.len() - 1) as f64;
    if matches!(aggregate.fn_, AggregateFn::StddevSamp) {
        Ok(f64_value(variance.sqrt()))
    } else {
        Ok(f64_value(variance))
    }
}

fn required_aggregate_column<'a>(
    block_id: &str,
    aggregate: &'a AggregateSpec,
) -> Result<&'a str, AggregateError> {
    aggregate.column.as_deref().ok_or_else(|| {
        AggregateError(format!(
            "Block '{}' aggregate '{}' requires field",
            block_id, aggregate.alias
        ))
    })
}

fn sort_virtual_aggregate_rows(
    rows: &mut [Vec<Value>],
    columns: &[String],
    request: &AggregateRequest,
) -> Result<(), AggregateError> {
    let column_index = columns
        .iter()
        .enumerate()
        .map(|(index, column)| (column.as_str(), index))
        .collect::<HashMap<_, _>>();

    if request.order_by.is_empty() {
        rows.sort_by(|left, right| {
            for index in 0..request.group_by.len() {
                let ordering = virtual_compare_values(
                    left.get(index).unwrap_or(&Value::Null),
                    right.get(index).unwrap_or(&Value::Null),
                )
                .unwrap_or(Ordering::Equal);
                if ordering != Ordering::Equal {
                    return ordering;
                }
            }
            Ordering::Equal
        });
        return Ok(());
    }

    for order_by in &request.order_by {
        if !column_index.contains_key(order_by.column.as_str()) {
            return Err(AggregateError(format!(
                "Aggregate orderBy references unavailable field '{}'",
                order_by.column
            )));
        }
    }

    rows.sort_by(|left, right| {
        for order_by in &request.order_by {
            let index = column_index[order_by.column.as_str()];
            let ordering = virtual_compare_values(
                left.get(index).unwrap_or(&Value::Null),
                right.get(index).unwrap_or(&Value::Null),
            )
            .unwrap_or(Ordering::Equal);
            let ordering = if order_by.direction == SortDirection::Desc {
                ordering.reverse()
            } else {
                ordering
            };
            if ordering != Ordering::Equal {
                return ordering;
            }
        }
        Ordering::Equal
    });
    Ok(())
}

fn compare_virtual_rows_by_order(
    left: &Map<String, Value>,
    right: &Map<String, Value>,
    order_by: &[AggregateOrderBy],
    flip_direction: bool,
) -> Ordering {
    for order in order_by {
        let left_value = virtual_row_value(left, &order.column).unwrap_or(&Value::Null);
        let right_value = virtual_row_value(right, &order.column).unwrap_or(&Value::Null);
        let ordering = virtual_compare_values(left_value, right_value).unwrap_or(Ordering::Equal);
        let descending = if flip_direction {
            order.direction == SortDirection::Asc
        } else {
            order.direction == SortDirection::Desc
        };
        let ordering = if descending {
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

fn virtual_row_value<'a>(row: &'a Map<String, Value>, field: &str) -> Option<&'a Value> {
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

fn virtual_compare_values(left: &Value, right: &Value) -> Option<Ordering> {
    match (left, right) {
        (Value::Null, Value::Null) => Some(Ordering::Equal),
        (Value::Null, _) => Some(Ordering::Greater),
        (_, Value::Null) => Some(Ordering::Less),
        (Value::Number(left), Value::Number(right)) => left.as_f64()?.partial_cmp(&right.as_f64()?),
        (Value::String(left), Value::String(right)) => Some(left.cmp(right)),
        (Value::Bool(left), Value::Bool(right)) => Some(left.cmp(right)),
        _ => Some(virtual_value_sort_key(left).cmp(&virtual_value_sort_key(right))),
    }
}

fn virtual_value_sort_key(value: &Value) -> String {
    match value {
        Value::String(value) => value.clone(),
        Value::Bool(value) => value.to_string(),
        Value::Number(value) => value.to_string(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

fn evaluate_virtual_expression(
    block_id: &str,
    expression: &Value,
    aliases: &HashMap<String, Value>,
) -> Result<Value, AggregateError> {
    let normalized = normalize_report_aggregate_expression(expression);
    evaluate_virtual_expression_inner(block_id, &normalized, aliases, 0)
}

fn evaluate_virtual_expression_inner(
    block_id: &str,
    expression: &Value,
    aliases: &HashMap<String, Value>,
    depth: u8,
) -> Result<Value, AggregateError> {
    if depth > 8 {
        return Err(AggregateError(format!(
            "Block '{}' aggregate expression is too deeply nested",
            block_id
        )));
    }
    let Some(object) = expression.as_object() else {
        return Ok(expression.clone());
    };

    if let Some(value_type) = object.get("valueType").and_then(Value::as_str) {
        let value = object.get("value").cloned().unwrap_or(Value::Null);
        return match value_type.to_ascii_lowercase().as_str() {
            "alias" => {
                let alias = value.as_str().ok_or_else(|| {
                    AggregateError(format!(
                        "Block '{}' aggregate expression alias value must be a string",
                        block_id
                    ))
                })?;
                Ok(aliases.get(alias).cloned().unwrap_or(Value::Null))
            }
            "immediate" => Ok(value),
            "reference" => Err(AggregateError(format!(
                "Block '{}' aggregate expressions cannot reference row fields",
                block_id
            ))),
            other => Err(AggregateError(format!(
                "Block '{}' aggregate expression uses unsupported valueType '{}'",
                block_id, other
            ))),
        };
    }

    let op = object
        .get("op")
        .and_then(Value::as_str)
        .map(normalize_expression_token)
        .ok_or_else(|| {
            AggregateError(format!(
                "Block '{}' aggregate expression operation must include op",
                block_id
            ))
        })?;
    let arguments = object
        .get("arguments")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|argument| evaluate_virtual_expression_inner(block_id, &argument, aliases, depth + 1))
        .collect::<Result<Vec<_>, _>>()?;

    evaluate_virtual_expression_op(&op, &arguments)
}

fn evaluate_virtual_expression_op(op: &str, arguments: &[Value]) -> Result<Value, AggregateError> {
    match op {
        "ADD" => Ok(option_f64_value(Some(
            arguments.iter().filter_map(Value::as_f64).sum(),
        ))),
        "SUB" => {
            let Some(first) = arguments.first().and_then(Value::as_f64) else {
                return Ok(Value::Null);
            };
            Ok(f64_value(
                arguments
                    .iter()
                    .skip(1)
                    .filter_map(Value::as_f64)
                    .fold(first, |acc, value| acc - value),
            ))
        }
        "MUL" => Ok(f64_value(
            arguments
                .iter()
                .filter_map(Value::as_f64)
                .fold(1.0, |acc, value| acc * value),
        )),
        "DIV" => {
            let (Some(left), Some(right)) = (
                arguments.first().and_then(Value::as_f64),
                arguments.get(1).and_then(Value::as_f64),
            ) else {
                return Ok(Value::Null);
            };
            if right == 0.0 {
                Ok(Value::Null)
            } else {
                Ok(f64_value(left / right))
            }
        }
        "NEG" => Ok(arguments
            .first()
            .and_then(Value::as_f64)
            .map(|value| f64_value(-value))
            .unwrap_or(Value::Null)),
        "ABS" => Ok(arguments
            .first()
            .and_then(Value::as_f64)
            .map(|value| f64_value(value.abs()))
            .unwrap_or(Value::Null)),
        "COALESCE" => Ok(arguments
            .iter()
            .find(|value| !value_is_empty(value))
            .cloned()
            .unwrap_or(Value::Null)),
        "EQ" | "NE" | "GT" | "GTE" | "LT" | "LTE" => {
            let left = arguments.first().unwrap_or(&Value::Null);
            let right = arguments.get(1).unwrap_or(&Value::Null);
            let ordering = virtual_compare_values(left, right);
            let equal = left == right || ordering == Some(Ordering::Equal);
            let result = match op {
                "EQ" => equal,
                "NE" => !equal,
                "GT" => ordering == Some(Ordering::Greater),
                "GTE" => equal || matches!(ordering, Some(Ordering::Greater | Ordering::Equal)),
                "LT" => ordering == Some(Ordering::Less),
                "LTE" => equal || matches!(ordering, Some(Ordering::Less | Ordering::Equal)),
                _ => false,
            };
            Ok(Value::Bool(result))
        }
        "AND" => Ok(Value::Bool(arguments.iter().all(expression_truthy))),
        "OR" => Ok(Value::Bool(arguments.iter().any(expression_truthy))),
        "NOT" => Ok(Value::Bool(
            !arguments.first().is_some_and(expression_truthy),
        )),
        "IS_DEFINED" => Ok(Value::Bool(
            arguments.first().is_some_and(|value| !value.is_null()),
        )),
        "IS_EMPTY" => Ok(Value::Bool(arguments.first().is_none_or(value_is_empty))),
        "IS_NOT_EMPTY" => Ok(Value::Bool(
            arguments
                .first()
                .is_some_and(|value| !value_is_empty(value)),
        )),
        other => Err(AggregateError(format!(
            "Aggregate expression uses unsupported op '{}'",
            other
        ))),
    }
}

fn expression_truthy(value: &Value) -> bool {
    match value {
        Value::Bool(value) => *value,
        Value::Number(value) => value.as_f64().is_some_and(|value| value != 0.0),
        Value::String(value) => !value.trim().is_empty(),
        Value::Array(values) => !values.is_empty(),
        Value::Object(values) => !values.is_empty(),
        Value::Null => false,
    }
}

fn value_is_empty(value: &Value) -> bool {
    match value {
        Value::Null => true,
        Value::String(value) => value.trim().is_empty(),
        Value::Array(values) => values.is_empty(),
        Value::Object(values) => values.is_empty(),
        _ => false,
    }
}

fn normalize_report_aggregate_expression(expression: &Value) -> Value {
    let Value::Object(object) = expression else {
        return expression.clone();
    };

    let mut normalized = object.clone();

    if let Some(value_type) = normalized.remove("value_type")
        && !normalized.contains_key("valueType")
    {
        normalized.insert("valueType".to_string(), value_type);
    }

    if !normalized.contains_key("valueType")
        && let Some(type_value) = normalized.get("type").and_then(Value::as_str)
        && matches!(
            normalize_expression_token(type_value).as_str(),
            "ALIAS" | "IMMEDIATE" | "REFERENCE"
        )
    {
        normalized.insert(
            "valueType".to_string(),
            Value::String(type_value.to_ascii_lowercase()),
        );
    }

    if let Some(value_type) = normalized.get_mut("valueType")
        && let Some(raw) = value_type.as_str()
    {
        *value_type = Value::String(raw.to_ascii_lowercase());
    }

    if let Some(op) = normalized.get_mut("op")
        && let Some(raw) = op.as_str()
    {
        *op = Value::String(normalize_expression_token(raw));
    }

    if let Some(fn_name) = normalized.get_mut("fn")
        && let Some(raw) = fn_name.as_str()
    {
        *fn_name = Value::String(normalize_expression_token(raw));
    }

    if let Some(arguments) = normalized.get_mut("arguments")
        && let Some(arguments) = arguments.as_array_mut()
    {
        for argument in arguments {
            *argument = normalize_report_aggregate_expression(argument);
        }
    }

    Value::Object(normalized)
}

fn normalize_expression_token(value: &str) -> String {
    let mut normalized = String::new();
    let mut previous_was_separator = true;
    let mut previous_was_lower_or_digit = false;

    for character in value.trim().chars() {
        if character == '_' || character == '-' || character == ' ' {
            if !previous_was_separator && !normalized.is_empty() {
                normalized.push('_');
            }
            previous_was_separator = true;
            previous_was_lower_or_digit = false;
            continue;
        }

        if character.is_uppercase() && previous_was_lower_or_digit && !previous_was_separator {
            normalized.push('_');
        }

        for upper in character.to_uppercase() {
            normalized.push(upper);
        }
        previous_was_separator = false;
        previous_was_lower_or_digit = character.is_lowercase() || character.is_ascii_digit();
    }

    match normalized.as_str() {
        "EQUALS" => "EQ".to_string(),
        "NOT_EQUALS" | "NOT_EQUAL" => "NE".to_string(),
        "GREATER_THAN" => "GT".to_string(),
        "GREATER_THAN_OR_EQUAL" | "GREATER_THAN_OR_EQUALS" => "GTE".to_string(),
        "LESS_THAN" => "LT".to_string(),
        "LESS_THAN_OR_EQUAL" | "LESS_THAN_OR_EQUALS" => "LTE".to_string(),
        "DEFINED" => "IS_DEFINED".to_string(),
        "EMPTY" => "IS_EMPTY".to_string(),
        "NOT_EMPTY" => "IS_NOT_EMPTY".to_string(),
        _ => normalized,
    }
}

fn f64_value(value: f64) -> Value {
    serde_json::Number::from_f64(value)
        .map(Value::Number)
        .unwrap_or(Value::Null)
}

fn option_f64_value(value: Option<f64>) -> Value {
    value
        .and_then(serde_json::Number::from_f64)
        .map(Value::Number)
        .unwrap_or(Value::Null)
}
