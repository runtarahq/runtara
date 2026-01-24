// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Helper functions for conditional step evaluation in generated workflows.
//!
//! These functions are used by the code generated from `ConditionalStep` in
//! `runtara-workflows` codegen. They provide JSON Value comparison, truthiness
//! checks, and numeric conversion.

use serde_json::Value;

/// Check if two JSON values are equal.
///
/// Performs type-coerced equality comparison:
/// - Numbers are compared numerically (i64 vs f64 handled)
/// - Strings are compared as strings
/// - Booleans are compared as booleans
/// - Arrays and objects use structural equality
/// - Null equals null
pub fn values_equal(left: &Value, right: &Value) -> bool {
    match (left, right) {
        // Both null
        (Value::Null, Value::Null) => true,

        // Both booleans
        (Value::Bool(l), Value::Bool(r)) => l == r,

        // Both numbers - compare as f64 for consistency
        (Value::Number(l), Value::Number(r)) => match (l.as_f64(), r.as_f64()) {
            (Some(lf), Some(rf)) => (lf - rf).abs() < f64::EPSILON,
            _ => false,
        },

        // Both strings
        (Value::String(l), Value::String(r)) => l == r,

        // Both arrays - element-wise comparison
        (Value::Array(l), Value::Array(r)) => {
            if l.len() != r.len() {
                return false;
            }
            l.iter().zip(r.iter()).all(|(a, b)| values_equal(a, b))
        }

        // Both objects - key-value comparison
        (Value::Object(l), Value::Object(r)) => {
            if l.len() != r.len() {
                return false;
            }
            l.iter()
                .all(|(k, v)| r.get(k).is_some_and(|rv| values_equal(v, rv)))
        }

        // String to number coercion (common in JSON APIs)
        (Value::String(s), Value::Number(n)) | (Value::Number(n), Value::String(s)) => {
            if let Ok(parsed) = s.parse::<f64>()
                && let Some(num) = n.as_f64()
            {
                return (parsed - num).abs() < f64::EPSILON;
            }
            false
        }

        // Different types - not equal
        _ => false,
    }
}

/// Check if a JSON value is "truthy".
///
/// Truthiness rules:
/// - `null` is falsy
/// - `false` is falsy
/// - `0` and `0.0` are falsy
/// - Empty string `""` is falsy
/// - Empty array `[]` is falsy
/// - Empty object `{}` is falsy
/// - Everything else is truthy
pub fn is_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => {
            // 0 is falsy
            if let Some(i) = n.as_i64() {
                i != 0
            } else if let Some(f) = n.as_f64() {
                f != 0.0
            } else {
                true
            }
        }
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

/// Convert a JSON value to a number (f64).
///
/// Conversion rules:
/// - Numbers are returned as f64
/// - Strings are parsed as f64
/// - Booleans: true = 1.0, false = 0.0
/// - Null, arrays, and objects return None
pub fn to_number(value: &Value) -> Option<f64> {
    match value {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.parse::<f64>().ok(),
        Value::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_values_equal_primitives() {
        assert!(values_equal(&json!(null), &json!(null)));
        assert!(values_equal(&json!(true), &json!(true)));
        assert!(!values_equal(&json!(true), &json!(false)));
        assert!(values_equal(&json!(42), &json!(42)));
        assert!(values_equal(&json!(42.0), &json!(42)));
        assert!(values_equal(&json!("hello"), &json!("hello")));
        assert!(!values_equal(&json!("hello"), &json!("world")));
    }

    #[test]
    fn test_values_equal_string_number_coercion() {
        assert!(values_equal(&json!("42"), &json!(42)));
        assert!(values_equal(&json!(42), &json!("42")));
        assert!(values_equal(&json!("3.14"), &json!(3.14)));
        assert!(!values_equal(&json!("not a number"), &json!(42)));
    }

    #[test]
    fn test_values_equal_arrays() {
        assert!(values_equal(&json!([1, 2, 3]), &json!([1, 2, 3])));
        assert!(!values_equal(&json!([1, 2, 3]), &json!([1, 2])));
        assert!(!values_equal(&json!([1, 2, 3]), &json!([1, 2, 4])));
    }

    #[test]
    fn test_values_equal_objects() {
        assert!(values_equal(&json!({"a": 1}), &json!({"a": 1})));
        assert!(!values_equal(&json!({"a": 1}), &json!({"a": 2})));
        assert!(!values_equal(&json!({"a": 1}), &json!({"b": 1})));
    }

    #[test]
    fn test_is_truthy() {
        // Falsy values
        assert!(!is_truthy(&json!(null)));
        assert!(!is_truthy(&json!(false)));
        assert!(!is_truthy(&json!(0)));
        assert!(!is_truthy(&json!(0.0)));
        assert!(!is_truthy(&json!("")));
        assert!(!is_truthy(&json!([])));
        assert!(!is_truthy(&json!({})));

        // Truthy values
        assert!(is_truthy(&json!(true)));
        assert!(is_truthy(&json!(1)));
        assert!(is_truthy(&json!(-1)));
        assert!(is_truthy(&json!(0.1)));
        assert!(is_truthy(&json!("hello")));
        assert!(is_truthy(&json!([1])));
        assert!(is_truthy(&json!({"a": 1})));
    }

    #[test]
    fn test_to_number() {
        assert_eq!(to_number(&json!(42)), Some(42.0));
        assert_eq!(to_number(&json!(3.14)), Some(3.14));
        assert_eq!(to_number(&json!("42")), Some(42.0));
        assert_eq!(to_number(&json!("3.14")), Some(3.14));
        assert_eq!(to_number(&json!(true)), Some(1.0));
        assert_eq!(to_number(&json!(false)), Some(0.0));
        assert_eq!(to_number(&json!(null)), None);
        assert_eq!(to_number(&json!("not a number")), None);
        assert_eq!(to_number(&json!([1, 2, 3])), None);
    }
}
