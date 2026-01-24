// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Type coercion utilities for agent inputs.
//!
//! This module provides automatic type coercion between JSON values and expected
//! Rust types. It bridges the gap between loosely-typed external data (e.g., APIs
//! returning numbers as strings) and strongly-typed agent input structs.
//!
//! # Supported Coercions
//!
//! | From | To | Example |
//! |------|-----|---------|
//! | String | f64/f32 | `"1840"` → `1840.0` |
//! | String | i64/i32/etc | `"42"` → `42` |
//! | String | u64/u32/etc | `"100"` → `100` |
//! | String | bool | `"true"`, `"1"`, `"yes"` → `true` |
//! | Number | String | `42` → `"42"` |
//! | Bool | String | `true` → `"true"` |
//! | Number | bool | `1` → `true`, `0` → `false` |

use crate::agent_meta::InputTypeMeta;
use serde_json::{Number, Value};

/// Coerce a JSON value to match the expected type based on the type name.
///
/// Returns the original value if coercion is not possible or not needed.
///
/// # Arguments
///
/// * `value` - The JSON value to coerce
/// * `type_name` - The expected Rust type name (e.g., "f64", "String", "bool")
///
/// # Examples
///
/// ```
/// use serde_json::json;
/// use runtara_dsl::coercion::coerce_to_type;
///
/// // String to f64
/// let result = coerce_to_type(json!("1840"), "f64");
/// assert_eq!(result, json!(1840.0));
///
/// // Number to String
/// let result = coerce_to_type(json!(42), "String");
/// assert_eq!(result, json!("42"));
/// ```
pub fn coerce_to_type(value: Value, type_name: &str) -> Value {
    // Handle Option<T> by extracting inner type
    let inner_type = extract_inner_type(type_name, "Option");
    let target_type = inner_type.unwrap_or(type_name);

    // Handle null values for Option types
    if value.is_null() {
        return value;
    }

    match (target_type, &value) {
        // ========================================
        // String → Numeric coercions
        // ========================================

        // String → f64/f32 (floating point)
        ("f64" | "f32", Value::String(s)) => s
            .trim()
            .parse::<f64>()
            .ok()
            .and_then(Number::from_f64)
            .map(Value::Number)
            .unwrap_or(value),

        // String → signed integers
        ("i64" | "i32" | "i16" | "i8" | "isize", Value::String(s)) => s
            .trim()
            .parse::<i64>()
            .ok()
            .map(|i| Value::Number(Number::from(i)))
            .unwrap_or(value),

        // String → unsigned integers
        ("u64" | "u32" | "u16" | "u8" | "usize", Value::String(s)) => s
            .trim()
            .parse::<u64>()
            .ok()
            .map(|u| Value::Number(Number::from(u)))
            .unwrap_or(value),

        // ========================================
        // Numeric → String coercion
        // ========================================
        ("String" | "str", Value::Number(n)) => Value::String(n.to_string()),

        // ========================================
        // Bool → String coercion
        // ========================================
        ("String" | "str", Value::Bool(b)) => Value::String(b.to_string()),

        // ========================================
        // String → Bool coercion
        // ========================================
        ("bool", Value::String(s)) => {
            let s_lower = s.trim().to_lowercase();
            Value::Bool(s_lower == "true" || s_lower == "1" || s_lower == "yes")
        }

        // ========================================
        // Number → Bool coercion (non-zero = true)
        // ========================================
        ("bool", Value::Number(n)) => {
            let is_true = n
                .as_i64()
                .map(|i| i != 0)
                .unwrap_or_else(|| n.as_f64().map(|f| f != 0.0).unwrap_or(false));
            Value::Bool(is_true)
        }

        // ========================================
        // No coercion needed or possible
        // ========================================
        _ => value,
    }
}

/// Coerce all fields in a JSON object based on InputTypeMeta.
///
/// This function iterates over all fields defined in the metadata and applies
/// type coercion where needed. Fields not in the metadata are left unchanged.
///
/// # Arguments
///
/// * `input` - The JSON object to coerce (typically agent input)
/// * `meta` - Metadata describing the expected input structure
///
/// # Examples
///
/// ```ignore
/// use serde_json::json;
/// use runtara_dsl::coercion::coerce_input;
///
/// let input = json!({
///     "weight": "1840",
///     "name": "Product"
/// });
///
/// // Assuming meta describes weight as f64
/// let coerced = coerce_input(input, &meta);
/// // coerced.weight is now 1840.0 (number, not string)
/// ```
pub fn coerce_input(mut input: Value, meta: &InputTypeMeta) -> Value {
    if let Value::Object(ref mut map) = input {
        for field in meta.fields {
            if let Some(value) = map.remove(field.name) {
                let coerced = coerce_field_value(value, field.type_name);
                map.insert(field.name.to_string(), coerced);
            }
        }
    }
    input
}

/// Coerce a field value, handling arrays and nested types.
fn coerce_field_value(value: Value, type_name: &str) -> Value {
    // Handle Vec<T> by coercing each element
    if let Some(inner_type) = extract_inner_type(type_name, "Vec") {
        if let Value::Array(arr) = value {
            return Value::Array(
                arr.into_iter()
                    .map(|v| coerce_to_type(v, inner_type))
                    .collect(),
            );
        }
        return value;
    }

    coerce_to_type(value, type_name)
}

/// Extract the inner type from a generic type like Option<T> or Vec<T>.
///
/// Returns None if the type doesn't match the wrapper pattern.
fn extract_inner_type<'a>(type_name: &'a str, wrapper: &str) -> Option<&'a str> {
    let prefix = format!("{}<", wrapper);
    if type_name.starts_with(&prefix) && type_name.ends_with('>') {
        Some(&type_name[prefix.len()..type_name.len() - 1])
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ========================================
    // String → Number coercions
    // ========================================

    #[test]
    fn test_string_to_f64() {
        assert_eq!(coerce_to_type(json!("1840"), "f64"), json!(1840.0));
        assert_eq!(coerce_to_type(json!("3.14159"), "f64"), json!(3.14159));
        assert_eq!(coerce_to_type(json!("-42.5"), "f64"), json!(-42.5));
        assert_eq!(coerce_to_type(json!("0"), "f64"), json!(0.0));
    }

    #[test]
    fn test_string_to_f64_with_whitespace() {
        assert_eq!(coerce_to_type(json!("  1840  "), "f64"), json!(1840.0));
        assert_eq!(coerce_to_type(json!("\t3.14\n"), "f64"), json!(3.14));
    }

    #[test]
    fn test_string_to_f32() {
        assert_eq!(coerce_to_type(json!("1840"), "f32"), json!(1840.0));
    }

    #[test]
    fn test_string_to_i64() {
        assert_eq!(coerce_to_type(json!("42"), "i64"), json!(42));
        assert_eq!(coerce_to_type(json!("-100"), "i64"), json!(-100));
        assert_eq!(coerce_to_type(json!("0"), "i64"), json!(0));
    }

    #[test]
    fn test_string_to_i32() {
        assert_eq!(coerce_to_type(json!("42"), "i32"), json!(42));
        assert_eq!(coerce_to_type(json!("-100"), "i32"), json!(-100));
    }

    #[test]
    fn test_string_to_u64() {
        assert_eq!(coerce_to_type(json!("42"), "u64"), json!(42));
        assert_eq!(coerce_to_type(json!("0"), "u64"), json!(0));
    }

    #[test]
    fn test_string_to_u32() {
        assert_eq!(coerce_to_type(json!("42"), "u32"), json!(42));
    }

    #[test]
    fn test_invalid_string_to_number_returns_original() {
        let invalid = json!("not a number");
        assert_eq!(coerce_to_type(invalid.clone(), "f64"), invalid);
        assert_eq!(coerce_to_type(json!("abc"), "i64"), json!("abc"));
    }

    #[test]
    fn test_negative_string_to_unsigned_returns_original() {
        // Negative numbers can't be parsed as unsigned
        assert_eq!(coerce_to_type(json!("-42"), "u64"), json!("-42"));
    }

    // ========================================
    // Number → String coercions
    // ========================================

    #[test]
    fn test_number_to_string() {
        assert_eq!(coerce_to_type(json!(42), "String"), json!("42"));
        assert_eq!(coerce_to_type(json!(3.14), "String"), json!("3.14"));
        assert_eq!(coerce_to_type(json!(-100), "String"), json!("-100"));
    }

    // ========================================
    // Bool → String coercions
    // ========================================

    #[test]
    fn test_bool_to_string() {
        assert_eq!(coerce_to_type(json!(true), "String"), json!("true"));
        assert_eq!(coerce_to_type(json!(false), "String"), json!("false"));
    }

    // ========================================
    // String → Bool coercions
    // ========================================

    #[test]
    fn test_string_to_bool_true() {
        assert_eq!(coerce_to_type(json!("true"), "bool"), json!(true));
        assert_eq!(coerce_to_type(json!("TRUE"), "bool"), json!(true));
        assert_eq!(coerce_to_type(json!("True"), "bool"), json!(true));
        assert_eq!(coerce_to_type(json!("1"), "bool"), json!(true));
        assert_eq!(coerce_to_type(json!("yes"), "bool"), json!(true));
        assert_eq!(coerce_to_type(json!("YES"), "bool"), json!(true));
    }

    #[test]
    fn test_string_to_bool_false() {
        assert_eq!(coerce_to_type(json!("false"), "bool"), json!(false));
        assert_eq!(coerce_to_type(json!("0"), "bool"), json!(false));
        assert_eq!(coerce_to_type(json!("no"), "bool"), json!(false));
        assert_eq!(coerce_to_type(json!(""), "bool"), json!(false));
        assert_eq!(coerce_to_type(json!("random"), "bool"), json!(false));
    }

    #[test]
    fn test_string_to_bool_with_whitespace() {
        assert_eq!(coerce_to_type(json!("  true  "), "bool"), json!(true));
        assert_eq!(coerce_to_type(json!("\n1\t"), "bool"), json!(true));
    }

    // ========================================
    // Number → Bool coercions
    // ========================================

    #[test]
    fn test_number_to_bool() {
        assert_eq!(coerce_to_type(json!(1), "bool"), json!(true));
        assert_eq!(coerce_to_type(json!(0), "bool"), json!(false));
        assert_eq!(coerce_to_type(json!(-1), "bool"), json!(true));
        assert_eq!(coerce_to_type(json!(42), "bool"), json!(true));
    }

    #[test]
    fn test_float_to_bool() {
        assert_eq!(coerce_to_type(json!(1.0), "bool"), json!(true));
        assert_eq!(coerce_to_type(json!(0.0), "bool"), json!(false));
        assert_eq!(coerce_to_type(json!(0.1), "bool"), json!(true));
    }

    // ========================================
    // No coercion needed
    // ========================================

    #[test]
    fn test_no_coercion_same_type() {
        // Already correct type - no change
        assert_eq!(coerce_to_type(json!(42.0), "f64"), json!(42.0));
        assert_eq!(coerce_to_type(json!(42), "i64"), json!(42));
        assert_eq!(coerce_to_type(json!("hello"), "String"), json!("hello"));
        assert_eq!(coerce_to_type(json!(true), "bool"), json!(true));
    }

    #[test]
    fn test_no_coercion_unknown_type() {
        // Unknown types pass through unchanged
        assert_eq!(coerce_to_type(json!("test"), "UnknownType"), json!("test"));
    }

    #[test]
    fn test_null_passthrough() {
        assert_eq!(coerce_to_type(Value::Null, "f64"), Value::Null);
        assert_eq!(coerce_to_type(Value::Null, "String"), Value::Null);
    }

    // ========================================
    // Option<T> handling
    // ========================================

    #[test]
    fn test_option_type_coercion() {
        assert_eq!(coerce_to_type(json!("42"), "Option<f64>"), json!(42.0));
        assert_eq!(coerce_to_type(json!("true"), "Option<bool>"), json!(true));
        assert_eq!(coerce_to_type(Value::Null, "Option<f64>"), Value::Null);
    }

    // ========================================
    // Vec<T> handling
    // ========================================

    #[test]
    fn test_vec_coercion() {
        let input = json!(["1", "2", "3"]);
        let expected = json!([1, 2, 3]);
        assert_eq!(coerce_field_value(input, "Vec<i64>"), expected);
    }

    #[test]
    fn test_vec_f64_coercion() {
        let input = json!(["1.5", "2.5", "3.5"]);
        let expected = json!([1.5, 2.5, 3.5]);
        assert_eq!(coerce_field_value(input, "Vec<f64>"), expected);
    }

    #[test]
    fn test_vec_bool_coercion() {
        let input = json!(["true", "false", "1", "0"]);
        let expected = json!([true, false, true, false]);
        assert_eq!(coerce_field_value(input, "Vec<bool>"), expected);
    }

    // ========================================
    // extract_inner_type tests
    // ========================================

    #[test]
    fn test_extract_inner_type_option() {
        assert_eq!(extract_inner_type("Option<f64>", "Option"), Some("f64"));
        assert_eq!(
            extract_inner_type("Option<String>", "Option"),
            Some("String")
        );
        assert_eq!(extract_inner_type("f64", "Option"), None);
    }

    #[test]
    fn test_extract_inner_type_vec() {
        assert_eq!(extract_inner_type("Vec<i32>", "Vec"), Some("i32"));
        assert_eq!(extract_inner_type("Vec<String>", "Vec"), Some("String"));
        assert_eq!(extract_inner_type("String", "Vec"), None);
    }

    // ========================================
    // Edge cases
    // ========================================

    #[test]
    fn test_empty_string_to_number() {
        // Empty string cannot be parsed as number
        assert_eq!(coerce_to_type(json!(""), "f64"), json!(""));
        assert_eq!(coerce_to_type(json!(""), "i64"), json!(""));
    }

    #[test]
    fn test_scientific_notation() {
        assert_eq!(coerce_to_type(json!("1e10"), "f64"), json!(1e10));
        assert_eq!(coerce_to_type(json!("1.5e-3"), "f64"), json!(0.0015));
    }

    #[test]
    fn test_object_passthrough() {
        let obj = json!({"nested": "value"});
        assert_eq!(coerce_to_type(obj.clone(), "f64"), obj);
    }

    #[test]
    fn test_array_passthrough_for_non_vec() {
        let arr = json!([1, 2, 3]);
        assert_eq!(coerce_to_type(arr.clone(), "f64"), arr);
    }
}
