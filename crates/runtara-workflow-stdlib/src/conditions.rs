// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Helper functions for conditional step evaluation in generated workflows.
//!
//! These functions are used by the code generated from `ConditionalStep` in
//! `runtara-workflows` codegen. They provide JSON Value comparison, truthiness
//! checks, and numeric conversion.

// One definition of condition semantics, shared with the form layer and
// `visibleWhen` / `requiredWhen` checks (see `runtara_dsl::condition_eval`).
pub use runtara_dsl::condition_eval::{is_truthy, to_number, values_equal};

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
        assert!(values_equal(&json!("2.75"), &json!(2.75)));
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
        assert_eq!(to_number(&json!(2.75)), Some(2.75));
        assert_eq!(to_number(&json!("42")), Some(42.0));
        assert_eq!(to_number(&json!("2.75")), Some(2.75));
        assert_eq!(to_number(&json!(true)), Some(1.0));
        assert_eq!(to_number(&json!(false)), Some(0.0));
        assert_eq!(to_number(&json!(null)), None);
        assert_eq!(to_number(&json!("not a number")), None);
        assert_eq!(to_number(&json!([1, 2, 3])), None);
    }
}
