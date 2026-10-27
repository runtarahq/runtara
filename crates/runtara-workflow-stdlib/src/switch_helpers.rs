// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Helper functions for Switch step output processing in generated workflows.
//!
//! Switch case matching is now handled at compile time via condition expression
//! codegen (shared with Conditional steps). Only output reference resolution
//! remains a runtime operation, and it goes through the same resolver as every
//! other mapping reference.

use serde_json::Value;

use crate::direct_json::apply_reference;

/// Process switch case output values, resolving reference-style mappings.
///
/// - Literal values (strings, numbers, bools, null): returned as-is
/// - Objects with `valueType` + `value` keys:
///   - `"reference"`: resolved against `source` exactly like any other mapping
///     reference — bracket and dotted paths, interned values, `default` and the
///     `type` hint all behave the same. A shape mismatch with no `default` is
///     an error.
///   - `"immediate"`: returns the `value` field directly
/// - Other objects/arrays: recursively processed
pub fn process_switch_output(output: &Value, source: &Value) -> Result<Value, String> {
    match output {
        Value::String(_) | Value::Number(_) | Value::Bool(_) | Value::Null => Ok(output.clone()),

        Value::Object(map) => {
            if let (Some(value_type), Some(value)) = (map.get("valueType"), map.get("value")) {
                match value_type.as_str() {
                    Some("reference") => apply_reference(map, source),
                    Some("immediate") => Ok(value.clone()),
                    _ => recurse_object(map, source),
                }
            } else {
                recurse_object(map, source)
            }
        }

        Value::Array(arr) => arr
            .iter()
            .map(|v| process_switch_output(v, source))
            .collect::<Result<Vec<_>, _>>()
            .map(Value::Array),
    }
}

/// Recursively process all values in a JSON object.
fn recurse_object(map: &serde_json::Map<String, Value>, source: &Value) -> Result<Value, String> {
    map.iter()
        .map(|(k, v)| Ok((k.clone(), process_switch_output(v, source)?)))
        .collect::<Result<serde_json::Map<String, Value>, String>>()
        .map(Value::Object)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn resolve(output: &Value, source: &Value) -> Value {
        process_switch_output(output, source).expect("switch output resolves")
    }

    #[test]
    fn test_output_literals_pass_through() {
        let source = json!({"data": {"name": "test"}});
        assert_eq!(resolve(&json!("hello"), &source), json!("hello"));
        assert_eq!(resolve(&json!(42), &source), json!(42));
        assert_eq!(resolve(&json!(true), &source), json!(true));
        assert_eq!(resolve(&json!(null), &source), json!(null));
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
        assert_eq!(resolve(&output, &source), json!("Poland"));

        let output = json!({"valueType": "reference", "value": "data.nested.value"});
        assert_eq!(resolve(&output, &source), json!(42));
    }

    #[test]
    fn test_output_reference_missing_path() {
        let source = json!({"data": {}});
        let output = json!({"valueType": "reference", "value": "data.missing"});
        assert_eq!(resolve(&output, &source), json!(null));
    }

    /// Switch output references support Python-style negative array indexing,
    /// consistent with the workflow reference resolver.
    #[test]
    fn test_output_reference_negative_index() {
        let source = json!({"data": {"items": ["a", "b", "c"]}});

        let last = json!({"valueType": "reference", "value": "data.items.-1"});
        assert_eq!(resolve(&last, &source), json!("c"));

        let first = json!({"valueType": "reference", "value": "data.items.-3"});
        assert_eq!(resolve(&first, &source), json!("a"));

        // Positive indexing unchanged.
        let positive = json!({"valueType": "reference", "value": "data.items.1"});
        assert_eq!(resolve(&positive, &source), json!("b"));

        // Out-of-range negative misses → null.
        let oob = json!({"valueType": "reference", "value": "data.items.-4"});
        assert_eq!(resolve(&oob, &source), json!(null));
    }

    /// The reference picker writes bracket form (`steps['id'].outputs…`); every
    /// spelling of the same path must resolve to the same value.
    #[test]
    fn test_output_reference_bracket_and_dotted_spellings_agree() {
        let source = json!({
            "steps": {"fetch": {"outputs": {"q": "found", "items": ["x", "y"]}}}
        });

        for path in [
            "steps.fetch.outputs.q",
            "steps['fetch'].outputs.q",
            r#"steps["fetch"].outputs.q"#,
            "steps['fetch']['outputs']['q']",
        ] {
            let output = json!({"valueType": "reference", "value": path});
            assert_eq!(resolve(&output, &source), json!("found"), "path {path}");
        }

        for path in [
            "steps.fetch.outputs.items.1",
            "steps['fetch'].outputs.items[1]",
        ] {
            let output = json!({"valueType": "reference", "value": path});
            assert_eq!(resolve(&output, &source), json!("y"), "path {path}");
        }
    }

    #[test]
    fn test_output_reference_quoted_key_containing_a_dot() {
        let source = json!({"data": {"a.b": "dotted", "a": {"b": "nested"}}});

        let quoted = json!({"valueType": "reference", "value": "data['a.b']"});
        assert_eq!(resolve(&quoted, &source), json!("dotted"));

        let nested = json!({"valueType": "reference", "value": "data.a.b"});
        assert_eq!(resolve(&nested, &source), json!("nested"));
    }

    #[test]
    fn test_output_reference_default_applies_when_absent_or_null() {
        let source = json!({"data": {"present": "here", "empty": null}});

        let absent = json!({
            "valueType": "reference", "value": "data['missing']", "default": "fallback"
        });
        assert_eq!(resolve(&absent, &source), json!("fallback"));

        let null = json!({
            "valueType": "reference", "value": "data.empty", "default": "fallback"
        });
        assert_eq!(resolve(&null, &source), json!("fallback"));

        let present = json!({
            "valueType": "reference", "value": "data.present", "default": "fallback"
        });
        assert_eq!(resolve(&present, &source), json!("here"));
    }

    #[test]
    fn test_output_reference_type_hint_coerces() {
        let source = json!({"data": {"count": "42"}});
        let output = json!({"valueType": "reference", "value": "data.count", "type": "integer"});
        assert_eq!(resolve(&output, &source), json!(42));
    }

    /// `+1` is not an index token, so it addresses a named key — and a named key
    /// into an array is a shape mismatch, which fails loudly unless a `default`
    /// is declared.
    #[test]
    fn test_output_reference_shape_mismatch_errors_without_default() {
        let source = json!({"data": {"items": ["a", "b"]}});

        let plus = json!({"valueType": "reference", "value": "data.items.+1"});
        let err = process_switch_output(&plus, &source).expect_err("+1 is a key, not an index");
        assert!(err.contains("is an array with no field '+1'"), "{err}");

        let named = json!({"valueType": "reference", "value": "data.items.first"});
        assert!(process_switch_output(&named, &source).is_err());

        let with_default = json!({
            "valueType": "reference", "value": "data.items.first", "default": "none"
        });
        assert_eq!(resolve(&with_default, &source), json!("none"));
    }

    #[test]
    fn test_output_nested_reference_error_propagates() {
        let source = json!({"data": {"items": []}});
        let output = json!({
            "label": "ok",
            "list": [{"valueType": "reference", "value": "data.items.first"}]
        });
        assert!(process_switch_output(&output, &source).is_err());
    }

    #[test]
    fn test_output_immediate_value() {
        let source = json!({});
        let output = json!({"valueType": "immediate", "value": {"result": true}});
        assert_eq!(resolve(&output, &source), json!({"result": true}));
    }

    #[test]
    fn test_output_recursive_object() {
        let source = json!({"data": {"status": "active"}});
        let output = json!({
            "label": "Status",
            "resolved": {"valueType": "reference", "value": "data.status"}
        });
        let result = resolve(&output, &source);
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
        let result = resolve(&output, &source);
        assert_eq!(result, json!([1, "literal", "plain"]));
    }
}
