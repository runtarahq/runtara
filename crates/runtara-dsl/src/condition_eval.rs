//! Deterministic evaluator for client-safe [`ConditionExpression`] values.
//!
//! The evaluator is intentionally host-independent so the backend can call it
//! directly and the browser can call the same implementation through WASM.
//! Operators that require a server-side query engine are rejected explicitly.

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
    Ok(truthy(&evaluate_expression(expression, context)?))
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
                if !truthy(&evaluate_argument(argument, context)?) {
                    return Ok(Value::Bool(false));
                }
            }
            Ok(Value::Bool(true))
        }
        Op::Or => {
            for argument in &operation.arguments {
                if truthy(&evaluate_argument(argument, context)?) {
                    return Ok(Value::Bool(true));
                }
            }
            Ok(Value::Bool(false))
        }
        Op::Not => {
            require_arity(&operation.op, &operation.arguments, "1")?;
            Ok(Value::Bool(!truthy(&evaluate_argument(
                &operation.arguments[0],
                context,
            )?)))
        }
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
            require_arity(&operation.op, &operation.arguments, "1")?;
            let value = evaluate_argument(&operation.arguments[0], context)?;
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
    match operator {
        Op::Eq => values_equal(left, right),
        Op::Ne => !values_equal(left, right),
        Op::Gt | Op::Gte | Op::Lt | Op::Lte => match (numeric(left), numeric(right)) {
            (Some(left), Some(right)) => match operator {
                Op::Gt => left > right,
                Op::Gte => left >= right,
                Op::Lt => left < right,
                Op::Lte => left <= right,
                _ => unreachable!(),
            },
            _ => match (left.as_str(), right.as_str()) {
                (Some(left), Some(right)) => match operator {
                    Op::Gt => left > right,
                    Op::Gte => left >= right,
                    Op::Lt => left < right,
                    Op::Lte => left <= right,
                    _ => unreachable!(),
                },
                _ => false,
            },
        },
        _ => false,
    }
}

fn values_equal(left: &Value, right: &Value) -> bool {
    if let (Some(left), Some(right)) = (numeric(left), numeric(right)) {
        return left == right;
    }
    left == right
}

fn numeric(value: &Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_i64().map(|integer| integer as f64))
}

fn truthy(value: &Value) -> bool {
    match value {
        Value::Bool(value) => *value,
        Value::Null => false,
        Value::Number(value) => value.as_f64().is_some_and(|number| number != 0.0),
        Value::String(value) => !value.is_empty(),
        Value::Array(value) => !value.is_empty(),
        Value::Object(value) => !value.is_empty(),
    }
}

fn length(value: &Value) -> u64 {
    match value {
        Value::String(value) => value.chars().count() as u64,
        Value::Array(value) => value.len() as u64,
        Value::Object(value) => value.len() as u64,
        Value::Null => 0,
        _ => 1,
    }
}

fn is_empty(value: &Value) -> bool {
    match value {
        Value::Null => true,
        Value::String(value) => value.is_empty(),
        Value::Array(value) => value.is_empty(),
        Value::Object(value) => value.is_empty(),
        _ => false,
    }
}

fn contains(left: &Value, right: &Value) -> bool {
    match (left, right) {
        (Value::String(haystack), Value::String(needle)) => haystack.contains(needle),
        (Value::Array(values), needle) => values.iter().any(|value| values_equal(value, needle)),
        _ => false,
    }
}

fn value_in(needle: &Value, haystack: &Value) -> bool {
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
