//! Deterministic evaluator for client-safe [`ConditionExpression`] values.
//!
//! The evaluator is intentionally host-independent so the backend can call it
//! directly and the browser can call the same implementation through WASM.
//! Operators that require a server-side query engine are rejected explicitly.
//!
//! The comparison leaves ([`values_equal`], [`to_number`], [`ordering`],
//! [`is_truthy`], [`length`], [`is_empty`], [`contains`], [`value_in`]) are the
//! single definition of condition semantics: the workflow runtime, this
//! evaluator (forms, Operations answers) and `visibleWhen` / `requiredWhen`
//! checks all call them, so a condition means the same thing everywhere.
//! They follow the workflow runtime, which deployed workflows depend on:
//! numeric strings and booleans coerce to numbers, and numbers compare with
//! an `f64::EPSILON` tolerance.

use crate::{
    ConditionArgument, ConditionExpression, ConditionOperation, ConditionOperator, MappingValue,
};
use serde_json::Value;

use crate::ConditionOperator as Op;

#[derive(Debug, Clone, PartialEq)]
pub enum ConditionEvaluationError {
    ServerOnly(ConditionOperator),
    ArgCount {
        op: ConditionOperator,
        expected: &'static str,
        got: usize,
    },
}

impl std::fmt::Display for ConditionEvaluationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ServerOnly(op) => {
                write!(
                    f,
                    "operator `{op:?}` is server-only and not evaluable client-side"
                )
            }
            Self::ArgCount { op, expected, got } => write!(
                f,
                "operator `{op:?}` requires {expected} argument(s), got {got}"
            ),
        }
    }
}

impl std::error::Error for ConditionEvaluationError {}

/// Evaluate a condition against a JSON object/value context.
///
/// Missing references resolve to `null`. Reference defaults are honored.
pub fn evaluate_condition(
    expression: &ConditionExpression,
    context: &Value,
) -> Result<bool, ConditionEvaluationError> {
    Ok(is_truthy(&evaluate_expression(expression, context)?))
}

/// Whether an operator can be evaluated by this shared in-memory evaluator.
pub fn is_client_evaluable_operator(operator: &ConditionOperator) -> bool {
    !matches!(
        operator,
        Op::Match | Op::SimilarityGte | Op::CosineDistanceLte | Op::L2DistanceLte
    )
}

fn evaluate_expression(
    expression: &ConditionExpression,
    context: &Value,
) -> Result<Value, ConditionEvaluationError> {
    match expression {
        ConditionExpression::Operation(operation) => evaluate_operation(operation, context),
        ConditionExpression::Value(value) => Ok(resolve_mapping_value(value, context)),
    }
}

fn evaluate_operation(
    operation: &ConditionOperation,
    context: &Value,
) -> Result<Value, ConditionEvaluationError> {
    if !is_client_evaluable_operator(&operation.op) {
        return Err(ConditionEvaluationError::ServerOnly(operation.op.clone()));
    }

    match &operation.op {
        Op::And => {
            for argument in &operation.arguments {
                if !is_truthy(&evaluate_argument(argument, context)?) {
                    return Ok(Value::Bool(false));
                }
            }
            Ok(Value::Bool(true))
        }
        Op::Or => {
            for argument in &operation.arguments {
                if is_truthy(&evaluate_argument(argument, context)?) {
                    return Ok(Value::Bool(true));
                }
            }
            Ok(Value::Bool(false))
        }
        // NOT and LENGTH are not arity-checked at save, so a missing
        // argument keeps the runtime's answer instead of an error.
        Op::Not => match operation.arguments.first() {
            Some(argument) => Ok(Value::Bool(!is_truthy(&evaluate_argument(
                argument, context,
            )?))),
            None => Ok(Value::Bool(true)),
        },
        Op::Eq | Op::Ne | Op::Gt | Op::Gte | Op::Lt | Op::Lte => {
            require_arity(&operation.op, &operation.arguments, "2")?;
            let left = evaluate_argument(&operation.arguments[0], context)?;
            let right = evaluate_argument(&operation.arguments[1], context)?;
            Ok(Value::Bool(compare(&operation.op, &left, &right)))
        }
        Op::StartsWith => binary_string(operation, context, |left, right| left.starts_with(right)),
        Op::EndsWith => binary_string(operation, context, |left, right| left.ends_with(right)),
        Op::Contains => {
            require_arity(&operation.op, &operation.arguments, "2")?;
            let left = evaluate_argument(&operation.arguments[0], context)?;
            let right = evaluate_argument(&operation.arguments[1], context)?;
            Ok(Value::Bool(contains(&left, &right)))
        }
        Op::In | Op::NotIn => {
            require_arity(&operation.op, &operation.arguments, "2")?;
            let needle = evaluate_argument(&operation.arguments[0], context)?;
            let haystack = evaluate_argument(&operation.arguments[1], context)?;
            let found = value_in(&needle, &haystack);
            Ok(Value::Bool(if matches!(operation.op, Op::NotIn) {
                !found
            } else {
                found
            }))
        }
        Op::Length => {
            let value = match operation.arguments.first() {
                Some(argument) => evaluate_argument(argument, context)?,
                None => Value::Null,
            };
            Ok(Value::Number(serde_json::Number::from(length(&value))))
        }
        Op::IsDefined => {
            require_arity(&operation.op, &operation.arguments, "1")?;
            let value = evaluate_argument(&operation.arguments[0], context)?;
            Ok(Value::Bool(!value.is_null()))
        }
        Op::IsEmpty | Op::IsNotEmpty => {
            require_arity(&operation.op, &operation.arguments, "1")?;
            let value = evaluate_argument(&operation.arguments[0], context)?;
            let empty = is_empty(&value);
            Ok(Value::Bool(if matches!(operation.op, Op::IsNotEmpty) {
                !empty
            } else {
                empty
            }))
        }
        Op::Match | Op::SimilarityGte | Op::CosineDistanceLte | Op::L2DistanceLte => {
            unreachable!("server-only operators return before dispatch")
        }
    }
}

fn evaluate_argument(
    argument: &ConditionArgument,
    context: &Value,
) -> Result<Value, ConditionEvaluationError> {
    match argument {
        ConditionArgument::Expression(expression) => evaluate_expression(expression, context),
        ConditionArgument::Value(value) => Ok(resolve_mapping_value(value, context)),
    }
}

fn resolve_mapping_value(value: &MappingValue, context: &Value) -> Value {
    match serde_json::to_value(value) {
        Ok(serialized) => resolve_mapping_value_from_json(&serialized, context),
        Err(_) => Value::Null,
    }
}

fn resolve_mapping_value_from_json(serialized: &Value, context: &Value) -> Value {
    let Some(object) = serialized.as_object() else {
        return Value::Null;
    };
    match object
        .get("valueType")
        .and_then(Value::as_str)
        .unwrap_or("")
    {
        "immediate" => object.get("value").cloned().unwrap_or(Value::Null),
        "reference" => {
            let path = object.get("value").and_then(Value::as_str).unwrap_or("");
            let resolved = value_by_path(context, path);
            if resolved.is_null() {
                object.get("default").cloned().unwrap_or(Value::Null)
            } else {
                resolved
            }
        }
        _ => Value::Null,
    }
}

fn value_by_path(context: &Value, path: &str) -> Value {
    if path.is_empty() {
        return Value::Null;
    }
    let mut cursor = context;
    for segment in path.split('.') {
        let Value::Object(object) = cursor else {
            return Value::Null;
        };
        let Some(next) = object.get(segment) else {
            return Value::Null;
        };
        cursor = next;
    }
    cursor.clone()
}

fn require_arity(
    operator: &ConditionOperator,
    arguments: &[ConditionArgument],
    expected: &'static str,
) -> Result<(), ConditionEvaluationError> {
    let expected_count = match expected {
        "1" => 1,
        "2" => 2,
        _ => return Ok(()),
    };
    if arguments.len() == expected_count {
        Ok(())
    } else {
        Err(ConditionEvaluationError::ArgCount {
            op: operator.clone(),
            expected,
            got: arguments.len(),
        })
    }
}

fn binary_string<F: Fn(&str, &str) -> bool>(
    operation: &ConditionOperation,
    context: &Value,
    compare: F,
) -> Result<Value, ConditionEvaluationError> {
    require_arity(&operation.op, &operation.arguments, "2")?;
    let left = evaluate_argument(&operation.arguments[0], context)?;
    let right = evaluate_argument(&operation.arguments[1], context)?;
    Ok(Value::Bool(match (left.as_str(), right.as_str()) {
        (Some(left), Some(right)) => compare(left, right),
        _ => false,
    }))
}

fn compare(operator: &ConditionOperator, left: &Value, right: &Value) -> bool {
    use std::cmp::Ordering::{Equal, Greater, Less};
    match operator {
        Op::Eq => values_equal(left, right),
        Op::Ne => !values_equal(left, right),
        Op::Gt => ordering(left, right) == Some(Greater),
        Op::Gte => matches!(ordering(left, right), Some(Greater | Equal)),
        Op::Lt => ordering(left, right) == Some(Less),
        Op::Lte => matches!(ordering(left, right), Some(Less | Equal)),
        _ => false,
    }
}

/// Condition equality (`EQ`, `NE`, `CONTAINS`/`IN` membership, and
/// `visibleWhen` / `requiredWhen`). Numbers compare numerically within
/// `f64::EPSILON`, a numeric string equals the number it parses to, and
/// arrays and objects compare element by element with the same rule. Values
/// of other differing types are unequal.
pub fn values_equal(left: &Value, right: &Value) -> bool {
    match (left, right) {
        (Value::Null, Value::Null) => true,
        (Value::Bool(left), Value::Bool(right)) => left == right,
        (Value::Number(left), Value::Number(right)) => match (left.as_f64(), right.as_f64()) {
            (Some(left), Some(right)) => (left - right).abs() < f64::EPSILON,
            _ => false,
        },
        (Value::String(left), Value::String(right)) => left == right,
        (Value::Array(left), Value::Array(right)) => {
            left.len() == right.len()
                && left
                    .iter()
                    .zip(right)
                    .all(|(left, right)| values_equal(left, right))
        }
        (Value::Object(left), Value::Object(right)) => {
            left.len() == right.len()
                && left.iter().all(|(key, value)| {
                    right
                        .get(key)
                        .is_some_and(|other| values_equal(value, other))
                })
        }
        (Value::String(text), Value::Number(number))
        | (Value::Number(number), Value::String(text)) => {
            match (text.parse::<f64>(), number.as_f64()) {
                (Ok(parsed), Some(number)) => (parsed - number).abs() < f64::EPSILON,
                _ => false,
            }
        }
        _ => false,
    }
}

/// A value as a number for ordering: numbers, numeric strings, and booleans
/// (`true` = 1, `false` = 0).
pub fn to_number(value: &Value) -> Option<f64> {
    match value {
        Value::Number(number) => number.as_f64(),
        Value::String(text) => text.parse::<f64>().ok(),
        Value::Bool(flag) => Some(if *flag { 1.0 } else { 0.0 }),
        _ => None,
    }
}

/// Order two values for `GT`/`GTE`/`LT`/`LTE`: numerically when both are
/// numbers ([`to_number`]), otherwise by string comparison when both are
/// strings. Anything else has no order, so every ordering comparison of it
/// is false.
pub fn ordering(left: &Value, right: &Value) -> Option<std::cmp::Ordering> {
    match (to_number(left), to_number(right)) {
        (Some(left), Some(right)) => left.partial_cmp(&right),
        _ => match (left.as_str(), right.as_str()) {
            (Some(left), Some(right)) => Some(left.cmp(right)),
            _ => None,
        },
    }
}

/// Condition truthiness: `null`, `false`, zero, and empty strings, arrays
/// and objects are false; everything else is true.
pub fn is_truthy(value: &Value) -> bool {
    match value {
        Value::Bool(value) => *value,
        Value::Null => false,
        Value::Number(value) => value.as_f64().is_some_and(|number| number != 0.0),
        Value::String(value) => !value.is_empty(),
        Value::Array(value) => !value.is_empty(),
        Value::Object(value) => !value.is_empty(),
    }
}

/// `LENGTH`: characters of a string, items of an array, keys of an object;
/// 0 for null and 1 for any other scalar.
pub fn length(value: &Value) -> u64 {
    match value {
        Value::String(value) => value.chars().count() as u64,
        Value::Array(value) => value.len() as u64,
        Value::Object(value) => value.len() as u64,
        Value::Null => 0,
        _ => 1,
    }
}

/// `IS_EMPTY`: null and empty strings, arrays and objects. Other scalars are
/// never empty.
pub fn is_empty(value: &Value) -> bool {
    match value {
        Value::Null => true,
        Value::String(value) => value.is_empty(),
        Value::Array(value) => value.is_empty(),
        Value::Object(value) => value.is_empty(),
        _ => false,
    }
}

/// `CONTAINS`: a substring of a string, or an element of an array
/// ([`values_equal`]).
pub fn contains(haystack: &Value, needle: &Value) -> bool {
    match (haystack, needle) {
        (Value::String(haystack), Value::String(needle)) => haystack.contains(needle.as_str()),
        (Value::Array(values), needle) => values.iter().any(|value| values_equal(value, needle)),
        _ => false,
    }
}

/// `IN`: an element of an array ([`values_equal`]).
pub fn value_in(needle: &Value, haystack: &Value) -> bool {
    match haystack {
        Value::Array(values) => values.iter().any(|value| values_equal(value, needle)),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn condition(value: Value) -> ConditionExpression {
        serde_json::from_value(value).expect("valid condition fixture")
    }

    #[test]
    fn evaluates_nested_boolean_and_comparison_expressions() {
        let expression = condition(json!({
            "type": "operation",
            "op": "AND",
            "arguments": [
                {
                    "type": "operation",
                    "op": "EQ",
                    "arguments": [
                        { "valueType": "reference", "value": "auth.mode" },
                        { "valueType": "immediate", "value": "api_key" }
                    ]
                },
                {
                    "type": "operation",
                    "op": "GT",
                    "arguments": [
                        { "valueType": "reference", "value": "retries" },
                        { "valueType": "immediate", "value": 0 }
                    ]
                }
            ]
        }));

        assert!(
            evaluate_condition(
                &expression,
                &json!({ "auth": { "mode": "api_key" }, "retries": 2 })
            )
            .unwrap()
        );
        assert!(
            !evaluate_condition(
                &expression,
                &json!({ "auth": { "mode": "none" }, "retries": 2 })
            )
            .unwrap()
        );
    }

    #[test]
    fn not_provides_state_inversion_without_duplicate_rule_effects() {
        let expression = condition(json!({
            "type": "operation",
            "op": "NOT",
            "arguments": [{
                "type": "operation",
                "op": "EQ",
                "arguments": [
                    { "valueType": "reference", "value": "mode" },
                    { "valueType": "immediate", "value": "hidden" }
                ]
            }]
        }));

        assert!(evaluate_condition(&expression, &json!({ "mode": "visible" })).unwrap());
        assert!(!evaluate_condition(&expression, &json!({ "mode": "hidden" })).unwrap());
    }

    #[test]
    fn reference_defaults_are_honored() {
        let expression = condition(json!({
            "type": "operation",
            "op": "EQ",
            "arguments": [
                { "valueType": "reference", "value": "missing", "default": "fallback" },
                { "valueType": "immediate", "value": "fallback" }
            ]
        }));

        assert!(evaluate_condition(&expression, &json!({})).unwrap());
    }

    #[test]
    fn server_only_operators_are_rejected() {
        let expression = condition(json!({
            "type": "operation",
            "op": "MATCH",
            "arguments": [
                { "valueType": "reference", "value": "content" },
                { "valueType": "immediate", "value": "needle" }
            ]
        }));

        assert!(matches!(
            evaluate_condition(&expression, &json!({})),
            Err(ConditionEvaluationError::ServerOnly(Op::Match))
        ));
    }

    #[test]
    fn all_operator_classifications_are_explicit() {
        use ConditionOperator::*;

        let client = [
            And, Or, Not, Gt, Gte, Lt, Lte, Eq, Ne, StartsWith, EndsWith, Contains, In, NotIn,
            Length, IsDefined, IsEmpty, IsNotEmpty,
        ];
        for operator in client {
            assert!(is_client_evaluable_operator(&operator), "{operator:?}");
        }

        let server = [SimilarityGte, Match, CosineDistanceLte, L2DistanceLte];
        for operator in server {
            assert!(!is_client_evaluable_operator(&operator), "{operator:?}");
        }
    }
}
