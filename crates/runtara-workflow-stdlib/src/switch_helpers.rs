// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Helper functions for Switch step output processing in generated workflows.
//!
//! Switch case matching is now handled at compile time via condition expression
//! codegen (shared with Conditional steps). Only output reference resolution
//! remains a runtime operation.

use serde_json::Value;

/// Process switch case output values, resolving reference-style mappings.
///
/// - Literal values (strings, numbers, bools, null): returned as-is
/// - Objects with `valueType` + `value` keys:
///   - `"reference"`: resolves dot-path against `source`
///   - `"immediate"`: returns the `value` field directly
/// - Other objects/arrays: recursively processed
pub fn process_switch_output(output: &Value, source: &Value) -> Value {
    match output {
        Value::String(_) | Value::Number(_) | Value::Bool(_) | Value::Null => output.clone(),

        Value::Object(map) => {
            if let (Some(value_type), Some(value)) = (map.get("valueType"), map.get("value")) {
                match value_type.as_str() {
                    Some("reference") => {
                        if let Some(path) = value.as_str() {
                            resolve_dot_path(source, path)
                        } else {
                            Value::Null
                        }
                    }
                    Some("immediate") => value.clone(),
                    _ => recurse_object(map, source),
                }
            } else {
                recurse_object(map, source)
            }
        }

        Value::Array(arr) => Value::Array(
            arr.iter()
                .map(|v| process_switch_output(v, source))
                .collect(),
        ),
    }
}

/// Resolve a dot-separated path against a JSON value.
///
/// Converts `"data.country"` to JSON pointer `"/data/country"` and looks it up.
fn resolve_dot_path(source: &Value, dot_path: &str) -> Value {
    let pointer = format!("/{}", dot_path.replace('.', "/"));
    source.pointer(&pointer).cloned().unwrap_or(Value::Null)
}

/// Recursively process all values in a JSON object.
fn recurse_object(map: &serde_json::Map<String, Value>, source: &Value) -> Value {
    let processed: serde_json::Map<String, Value> = map
        .iter()
        .map(|(k, v)| (k.clone(), process_switch_output(v, source)))
        .collect();
    Value::Object(processed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_output_literals_pass_through() {
        let source = json!({"data": {"name": "test"}});
        assert_eq!(
            process_switch_output(&json!("hello"), &source),
            json!("hello")
        );
        assert_eq!(process_switch_output(&json!(42), &source), json!(42));
        assert_eq!(process_switch_output(&json!(true), &source), json!(true));
        assert_eq!(process_switch_output(&json!(null), &source), json!(null));
    }

    #[test]
    fn test_output_reference_resolution() {
        let source = json!({
            "data": {
                "country": "Poland",
                "nested": {"value": 42}
            }
        });

        let output = json!({"valueType": "reference", "value": "data.country"});
        assert_eq!(process_switch_output(&output, &source), json!("Poland"));

        let output = json!({"valueType": "reference", "value": "data.nested.value"});
        assert_eq!(process_switch_output(&output, &source), json!(42));
    }

    #[test]
    fn test_output_reference_missing_path() {
        let source = json!({"data": {}});
        let output = json!({"valueType": "reference", "value": "data.missing"});
        assert_eq!(process_switch_output(&output, &source), json!(null));
    }

    #[test]
    fn test_output_immediate_value() {
        let source = json!({});
        let output = json!({"valueType": "immediate", "value": {"result": true}});
        assert_eq!(
            process_switch_output(&output, &source),
            json!({"result": true})
        );
    }

    #[test]
    fn test_output_recursive_object() {
        let source = json!({"data": {"status": "active"}});
        let output = json!({
            "label": "Status",
            "resolved": {"valueType": "reference", "value": "data.status"}
        });
        let result = process_switch_output(&output, &source);
        assert_eq!(result["label"], json!("Status"));
        assert_eq!(result["resolved"], json!("active"));
    }

    #[test]
    fn test_output_recursive_array() {
        let source = json!({"data": {"x": 1, "y": 2}});
        let output = json!([
            {"valueType": "reference", "value": "data.x"},
            {"valueType": "immediate", "value": "literal"},
            "plain"
        ]);
        let result = process_switch_output(&output, &source);
        assert_eq!(result, json!([1, "literal", "plain"]));
    }
}
