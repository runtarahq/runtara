//! Client-evaluatable row condition evaluator.
//!
//! Operates on the canonical `runtara_dsl::ConditionExpression` AST.
//! Designed for FE+BE parity: ships to the browser via WASM and runs on the
//! server too, replacing the hand-rolled evaluators in
//! `services/reports.rs:5713-5856` and `frontend/.../utils.ts:267-403`.
//!
//! Some `ConditionOperator` variants are server-only (they translate to SQL
//! against the object-store). We reject them here with a structured error
//! so callers can either: (a) skip client-side eval and round-trip to the
//! server, or (b) treat them as "condition not yet evaluable" and render
//! the row optimistically.

use runtara_dsl::{
    ConditionArgument, ConditionExpression, ConditionOperation, ConditionOperator, MappingValue,
};
use serde_json::Value;
use thiserror::Error;

// Local alias so match patterns can't collide with `SwitchMatchType` variants
// of the same name that runtara-dsl also exports.
use runtara_dsl::ConditionOperator as Op;

#[derive(Debug, Error)]
pub enum RowConditionError {
    /// The expression uses an operator that only makes sense server-side
    /// (e.g. `MATCH`, `SIMILARITY_GTE`). The caller decides whether to
    /// skip evaluation or escalate.
    #[error("operator `{0:?}` is server-only and not evaluable client-side")]
    ServerOnly(ConditionOperator),
    /// The expression uses a `MappingValue::Reference` that points at a
    /// data path the client doesn't expose. Today this is mapped to a
    /// plain "missing field" error; callers should treat it as undefined.
    #[error("reference `{0}` could not be resolved against the row")]
    UnresolvedReference(String),
    /// Wrong number of arguments for the operator.
    #[error("operator `{op:?}` requires {expected} argument(s), got {got}")]
    ArgCount {
        op: ConditionOperator,
        expected: &'static str,
        got: usize,
    },
}

/// Evaluate a `ConditionExpression` against a row. Returns `Ok(false)` for
/// expressions whose references resolve to undefined; returns
/// `Err(RowConditionError::ServerOnly(...))` for server-only operators.
pub fn evaluate_row_condition(
    expr: &ConditionExpression,
    row: &Value,
) -> Result<bool, RowConditionError> {
    let value = evaluate_expression(expr, row)?;
    Ok(truthy(&value))
}

// ---------------------------------------------------------------------------
// Internal recursion
// ---------------------------------------------------------------------------

fn evaluate_expression(
    expr: &ConditionExpression,
    row: &Value,
) -> Result<Value, RowConditionError> {
    match expr {
        ConditionExpression::Operation(op) => evaluate_operation(op, row),
        ConditionExpression::Value(mv) => Ok(resolve_mapping_value(mv, row)),
    }
}

fn evaluate_operation(op: &ConditionOperation, row: &Value) -> Result<Value, RowConditionError> {
    match &op.op {
        // Server-only operators.
        Op::Match | Op::SimilarityGte | Op::CosineDistanceLte | Op::L2DistanceLte => {
            Err(RowConditionError::ServerOnly(op.op.clone()))
        }
        // Logical.
        Op::And => {
            for arg in &op.arguments {
                let v = evaluate_argument(arg, row)?;
                if !truthy(&v) {
                    return Ok(Value::Bool(false));
                }
            }
            Ok(Value::Bool(true))
        }
        Op::Or => {
            for arg in &op.arguments {
                let v = evaluate_argument(arg, row)?;
                if truthy(&v) {
                    return Ok(Value::Bool(true));
                }
            }
            Ok(Value::Bool(false))
        }
        Op::Not => {
            require_arity(&op.op, &op.arguments, "1")?;
            let v = evaluate_argument(&op.arguments[0], row)?;
            Ok(Value::Bool(!truthy(&v)))
        }
        // Comparisons.
        Op::Eq | Op::Ne | Op::Gt | Op::Gte | Op::Lt | Op::Lte => {
            require_arity(&op.op, &op.arguments, "2")?;
            let left = evaluate_argument(&op.arguments[0], row)?;
            let right = evaluate_argument(&op.arguments[1], row)?;
            Ok(Value::Bool(compare(&op.op, &left, &right)))
        }
        // String prefix/suffix.
        Op::StartsWith => binary_string(op, row, |left, right| left.starts_with(right)),
        Op::EndsWith => binary_string(op, row, |left, right| left.ends_with(right)),
        // Array / containment.
        Op::Contains => {
            require_arity(&op.op, &op.arguments, "2")?;
            let left = evaluate_argument(&op.arguments[0], row)?;
            let right = evaluate_argument(&op.arguments[1], row)?;
            Ok(Value::Bool(contains(&left, &right)))
        }
        Op::In => {
            require_arity(&op.op, &op.arguments, "2")?;
            let needle = evaluate_argument(&op.arguments[0], row)?;
            let haystack = evaluate_argument(&op.arguments[1], row)?;
            Ok(Value::Bool(value_in(&needle, &haystack)))
        }
        Op::NotIn => {
            require_arity(&op.op, &op.arguments, "2")?;
            let needle = evaluate_argument(&op.arguments[0], row)?;
            let haystack = evaluate_argument(&op.arguments[1], row)?;
            Ok(Value::Bool(!value_in(&needle, &haystack)))
        }
        // Utility.
        Op::Length => {
            require_arity(&op.op, &op.arguments, "1")?;
            let v = evaluate_argument(&op.arguments[0], row)?;
            Ok(Value::Number(serde_json::Number::from(length(&v))))
        }
        Op::IsDefined => {
            require_arity(&op.op, &op.arguments, "1")?;
            let v = evaluate_argument(&op.arguments[0], row)?;
            Ok(Value::Bool(!v.is_null()))
        }
        Op::IsEmpty => {
            require_arity(&op.op, &op.arguments, "1")?;
            let v = evaluate_argument(&op.arguments[0], row)?;
            Ok(Value::Bool(is_empty(&v)))
        }
        Op::IsNotEmpty => {
            require_arity(&op.op, &op.arguments, "1")?;
            let v = evaluate_argument(&op.arguments[0], row)?;
            Ok(Value::Bool(!is_empty(&v)))
        }
    }
}

fn evaluate_argument(arg: &ConditionArgument, row: &Value) -> Result<Value, RowConditionError> {
    match arg {
        ConditionArgument::Expression(expr) => evaluate_expression(expr, row),
        ConditionArgument::Value(mv) => Ok(resolve_mapping_value(mv, row)),
    }
}

fn resolve_mapping_value(mv: &MappingValue, row: &Value) -> Value {
    // Serialize once to JSON, then read the shape. MappingValue is an
    // internally-tagged enum; we don't need to mirror its private layout
    // here — just rely on its serde representation.
    match serde_json::to_value(mv) {
        Ok(serialized) => resolve_mapping_value_from_json(&serialized, row),
        Err(_) => Value::Null,
    }
}

fn resolve_mapping_value_from_json(serialized: &Value, row: &Value) -> Value {
    let obj = match serialized.as_object() {
        Some(o) => o,
        None => return Value::Null,
    };
    let kind = obj.get("valueType").and_then(Value::as_str).unwrap_or("");
    match kind {
        // ImmediateValue: `{ "valueType": "immediate", "value": <literal> }`
        "immediate" => obj.get("value").cloned().unwrap_or(Value::Null),
        // ReferenceValue: `{ "valueType": "reference", "value": "dotted.path" }`
        "reference" => {
            let path = obj.get("value").and_then(Value::as_str).unwrap_or("");
            let resolved = row_value_by_path(row, path);
            if resolved.is_null() {
                obj.get("default").cloned().unwrap_or(Value::Null)
            } else {
                resolved
            }
        }
        // Composite + Template are not evaluable client-side without more
        // context (template needs the full execution scope). Caller should
        // skip client-eval and round-trip to the server for these.
        _ => Value::Null,
    }
}

/// Walk a dotted path against the row. `"customer.email"` → `row.customer.email`.
fn row_value_by_path(row: &Value, path: &str) -> Value {
    if path.is_empty() {
        return Value::Null;
    }
    let mut cursor = row;
    for segment in path.split('.') {
        match cursor {
            Value::Object(map) => match map.get(segment) {
                Some(next) => cursor = next,
                None => return Value::Null,
            },
            _ => return Value::Null,
        }
    }
    cursor.clone()
}

// ---------------------------------------------------------------------------
// Op helpers
// ---------------------------------------------------------------------------

fn require_arity(
    op: &ConditionOperator,
    args: &[ConditionArgument],
    expected: &'static str,
) -> Result<(), RowConditionError> {
    let needed = match expected {
        "1" => 1,
        "2" => 2,
        _ => return Ok(()),
    };
    if args.len() == needed {
        Ok(())
    } else {
        Err(RowConditionError::ArgCount {
            op: op.clone(),
            expected,
            got: args.len(),
        })
    }
}

fn binary_string<F: Fn(&str, &str) -> bool>(
    op: &ConditionOperation,
    row: &Value,
    cmp: F,
) -> Result<Value, RowConditionError> {
    require_arity(&op.op, &op.arguments, "2")?;
    let left = evaluate_argument(&op.arguments[0], row)?;
    let right = evaluate_argument(&op.arguments[1], row)?;
    let result = match (left.as_str(), right.as_str()) {
        (Some(l), Some(r)) => cmp(l, r),
        _ => false,
    };
    Ok(Value::Bool(result))
}

fn compare(op: &ConditionOperator, left: &Value, right: &Value) -> bool {
    match op {
        Op::Eq => values_equal(left, right),
        Op::Ne => !values_equal(left, right),
        Op::Gt | Op::Gte | Op::Lt | Op::Lte => match (numeric(left), numeric(right)) {
            (Some(l), Some(r)) => match op {
                Op::Gt => l > r,
                Op::Gte => l >= r,
                Op::Lt => l < r,
                Op::Lte => l <= r,
                _ => unreachable!(),
            },
            _ => match (left.as_str(), right.as_str()) {
                (Some(l), Some(r)) => match op {
                    Op::Gt => l > r,
                    Op::Gte => l >= r,
                    Op::Lt => l < r,
                    Op::Lte => l <= r,
                    _ => unreachable!(),
                },
                _ => false,
            },
        },
        _ => false,
    }
}

fn values_equal(left: &Value, right: &Value) -> bool {
    if let (Some(l), Some(r)) = (numeric(left), numeric(right)) {
        return l == r;
    }
    left == right
}

fn numeric(v: &Value) -> Option<f64> {
    v.as_f64().or_else(|| v.as_i64().map(|i| i as f64))
}

fn truthy(v: &Value) -> bool {
    match v {
        Value::Bool(b) => *b,
        Value::Null => false,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

fn length(v: &Value) -> u64 {
    match v {
        Value::String(s) => s.chars().count() as u64,
        Value::Array(a) => a.len() as u64,
        Value::Object(o) => o.len() as u64,
        Value::Null => 0,
        _ => 1,
    }
}

fn is_empty(v: &Value) -> bool {
    match v {
        Value::Null => true,
        Value::String(s) => s.is_empty(),
        Value::Array(a) => a.is_empty(),
        Value::Object(o) => o.is_empty(),
        _ => false,
    }
}

fn contains(left: &Value, right: &Value) -> bool {
    match (left, right) {
        (Value::String(haystack), Value::String(needle)) => haystack.contains(needle.as_str()),
        (Value::Array(arr), needle) => arr.iter().any(|v| values_equal(v, needle)),
        _ => false,
    }
}

fn value_in(needle: &Value, haystack: &Value) -> bool {
    match haystack {
        Value::Array(arr) => arr.iter().any(|v| values_equal(v, needle)),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn cond(json_str: &str) -> ConditionExpression {
        serde_json::from_str(json_str).unwrap_or_else(|e| panic!("parse cond: {e}\n{json_str}"))
    }

    #[test]
    fn eq_reference_to_immediate() {
        let expr = cond(
            r#"{
                "type": "operation",
                "op": "EQ",
                "arguments": [
                    { "valueType": "reference", "value": "status" },
                    { "valueType": "immediate", "value": "active" }
                ]
            }"#,
        );
        assert!(evaluate_row_condition(&expr, &json!({ "status": "active" })).unwrap());
        assert!(!evaluate_row_condition(&expr, &json!({ "status": "paused" })).unwrap());
    }

    #[test]
    fn gt_with_numeric_coercion() {
        let expr = cond(
            r#"{
                "type": "operation",
                "op": "GT",
                "arguments": [
                    { "valueType": "reference", "value": "total" },
                    { "valueType": "immediate", "value": 100 }
                ]
            }"#,
        );
        assert!(evaluate_row_condition(&expr, &json!({ "total": 150 })).unwrap());
        assert!(!evaluate_row_condition(&expr, &json!({ "total": 50 })).unwrap());
    }

    #[test]
    fn and_short_circuits_on_first_false() {
        let expr = cond(
            r#"{
                "type": "operation",
                "op": "AND",
                "arguments": [
                    {
                        "type": "operation",
                        "op": "EQ",
                        "arguments": [
                            { "valueType": "reference", "value": "status" },
                            { "valueType": "immediate", "value": "active" }
                        ]
                    },
                    {
                        "type": "operation",
                        "op": "GT",
                        "arguments": [
                            { "valueType": "reference", "value": "amount" },
                            { "valueType": "immediate", "value": 0 }
                        ]
                    }
                ]
            }"#,
        );
        assert!(
            evaluate_row_condition(&expr, &json!({ "status": "active", "amount": 50 })).unwrap()
        );
        assert!(
            !evaluate_row_condition(&expr, &json!({ "status": "paused", "amount": 50 })).unwrap()
        );
    }

    #[test]
    fn dotted_path_resolution() {
        let expr = cond(
            r#"{
                "type": "operation",
                "op": "EQ",
                "arguments": [
                    { "valueType": "reference", "value": "customer.tier" },
                    { "valueType": "immediate", "value": "gold" }
                ]
            }"#,
        );
        let row = json!({ "customer": { "tier": "gold" } });
        assert!(evaluate_row_condition(&expr, &row).unwrap());
    }

    #[test]
    fn in_against_array() {
        let expr = cond(
            r#"{
                "type": "operation",
                "op": "IN",
                "arguments": [
                    { "valueType": "reference", "value": "status" },
                    { "valueType": "immediate", "value": ["active", "paused"] }
                ]
            }"#,
        );
        assert!(evaluate_row_condition(&expr, &json!({ "status": "paused" })).unwrap());
        assert!(!evaluate_row_condition(&expr, &json!({ "status": "cancelled" })).unwrap());
    }

    #[test]
    fn is_defined_on_missing_path() {
        let expr = cond(
            r#"{
                "type": "operation",
                "op": "IS_DEFINED",
                "arguments": [
                    { "valueType": "reference", "value": "missing" }
                ]
            }"#,
        );
        assert!(!evaluate_row_condition(&expr, &json!({})).unwrap());
    }

    #[test]
    fn server_only_match_rejected() {
        let expr = cond(
            r#"{
                "type": "operation",
                "op": "MATCH",
                "arguments": [
                    { "valueType": "reference", "value": "content" },
                    { "valueType": "immediate", "value": "search terms" }
                ]
            }"#,
        );
        let err = evaluate_row_condition(&expr, &json!({})).unwrap_err();
        assert!(matches!(err, RowConditionError::ServerOnly(_)));
    }

    #[test]
    fn not_inverts() {
        let expr = cond(
            r#"{
                "type": "operation",
                "op": "NOT",
                "arguments": [
                    {
                        "type": "operation",
                        "op": "EQ",
                        "arguments": [
                            { "valueType": "reference", "value": "status" },
                            { "valueType": "immediate", "value": "active" }
                        ]
                    }
                ]
            }"#,
        );
        assert!(evaluate_row_condition(&expr, &json!({ "status": "paused" })).unwrap());
        assert!(!evaluate_row_condition(&expr, &json!({ "status": "active" })).unwrap());
    }

    /// Drift guard: the operators this evaluator rejects as `ServerOnly` must be
    /// exactly the ones the shared classification marks non-client-evaluable, so
    /// the hand-written server-only arm can't drift from `operator_support`.
    /// The server-only arm runs before any arity check, so an empty-argument
    /// operation is enough to observe the classification.
    #[test]
    fn server_only_arm_matches_classification() {
        use crate::operator_support::operator_support;
        use runtara_dsl::{ConditionOperation, ConditionOperator::*};

        let all = [
            And,
            Or,
            Not,
            Gt,
            Gte,
            Lt,
            Lte,
            Eq,
            Ne,
            StartsWith,
            EndsWith,
            Contains,
            In,
            NotIn,
            Length,
            IsDefined,
            IsEmpty,
            IsNotEmpty,
            SimilarityGte,
            Match,
            CosineDistanceLte,
            L2DistanceLte,
        ];
        for op in all {
            let operation = ConditionOperation {
                op: op.clone(),
                arguments: vec![],
            };
            let is_server_only = matches!(
                evaluate_operation(&operation, &json!({})),
                Err(RowConditionError::ServerOnly(_))
            );
            assert_eq!(
                is_server_only,
                !operator_support(op.clone()).client_evaluable,
                "row evaluator server-only classification of {op:?} disagrees with the client_evaluable tier"
            );
        }
    }
}
