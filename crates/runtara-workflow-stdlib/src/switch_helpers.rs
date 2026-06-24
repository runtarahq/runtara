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
/// Walks each `.`-separated segment as an object key or array index. Array
/// indices support Python-style negative suffix indexing (`-1` is the last
/// element), matching the workflow reference resolver. An unmatched segment
/// resolves to `Value::Null`.
fn resolve_dot_path(source: &Value, dot_path: &str) -> Value {
    let mut current = source;
    for segment in dot_path.split('.') {
        let next = match current {
            Value::Object(map) => map.get(segment),
            Value::Array(items) => array_index(segment, items.len()).and_then(|i| items.get(i)),
            _ => None,
        };
        match next {
            Some(value) => current = value,
            None => return Value::Null,
        }
    }
    current.clone()
}

/// Resolve a path segment to a concrete array index, supporting Python-style
/// negative suffix indexing (`-1` is the last element). Non-numeric segments and
/// out-of-range negatives return `None`.
fn array_index(segment: &str, len: usize) -> Option<usize> {
    let raw: i64 = segment.parse().ok()?;
    if raw >= 0 {
        usize::try_from(raw).ok()
    } else {
        len.checked_sub(usize::try_from(raw.unsigned_abs()).ok()?)
    }
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

    /// SYN-448: switch output references support Python-style negative array
    /// indexing, consistent with the workflow reference resolver.
    #[test]
    fn test_output_reference_negative_index() {
        let source = json!({"data": {"items": ["a", "b", "c"]}});

        let last = json!({"valueType": "reference", "value": "data.items.-1"});
        assert_eq!(process_switch_output(&last, &source), json!("c"));

        let first = json!({"valueType": "reference", "value": "data.items.-3"});
        assert_eq!(process_switch_output(&first, &source), json!("a"));

        // Positive indexing unchanged.
        let positive = json!({"valueType": "reference", "value": "data.items.1"});
        assert_eq!(process_switch_output(&positive, &source), json!("b"));

        // Out-of-range negative misses → null.
        let oob = json!({"valueType": "reference", "value": "data.items.-4"});
        assert_eq!(process_switch_output(&oob, &source), json!(null));
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
