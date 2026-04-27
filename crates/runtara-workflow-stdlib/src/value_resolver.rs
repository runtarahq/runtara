// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Runtime helper that walks a JSON value tree and resolves any nested
//! `{valueType: "reference", value: "<path>"}` envelopes against the
//! workflow execution context.
//!
//! Top-level input mappings are resolved by the codegen (see
//! `mapping::emit_input_mapping`), but `MappingValue::Immediate` nodes pass
//! their inner JSON through verbatim. When that inner JSON is a typed
//! structure like `ConditionExpression` it can carry nested
//! `{valueType: "reference"}` envelopes that the codegen does not visit.
//!
//! This pass runs once per capability invocation, after the input envelope
//! is built and before it is serialized to the agent. It is conservative:
//! only objects that exactly match the wire shape (`valueType` is `reference`
//! and `value` is a string) are rewritten.

use serde_json::Value;

/// Recursively replace any `{valueType: "reference", value: "<path>"}` JSON
/// objects inside `value` with the looked-up value from `source`.
///
/// `source` is the full execution context envelope, the same shape the
/// codegen builds for top-level reference resolution: it has `data`,
/// `variables`, `steps`, and `workflow.inputs` fields. Reference paths use
/// dotted notation (e.g. `"data.customer_category"`); the helper converts
/// them to JSON pointers internally.
pub fn resolve_nested_references(mut value: Value, source: &Value) -> Value {
    walk(&mut value, source);
    value
}

fn walk(value: &mut Value, source: &Value) {
    match value {
        Value::Object(map) => {
            // Detect the wire shape `{valueType: "reference", value: "<path>"}`.
            // Optional fields like `default` and `typeHint` are tolerated.
            let is_ref_envelope = matches!(
                map.get("valueType"),
                Some(Value::String(s)) if s == "reference"
            ) && matches!(map.get("value"), Some(Value::String(_)));

            if is_ref_envelope {
                let path = match map.get("value") {
                    Some(Value::String(s)) => s.clone(),
                    _ => return,
                };
                let default = map.get("default").cloned();
                let resolved =
                    resolve_path(&path, source).unwrap_or_else(|| default.unwrap_or(Value::Null));
                // Replace the entire object in-place with the resolved value.
                *value = resolved;
                // The resolved value itself may be a JSON object/array we want
                // to walk too — fall through.
                walk(value, source);
                return;
            }

            // Don't rewrite immediate envelopes — their `value` is intended to
            // be passed through verbatim.
            let is_immediate_envelope = matches!(
                map.get("valueType"),
                Some(Value::String(s)) if s == "immediate"
            );
            if is_immediate_envelope {
                return;
            }

            for (_, child) in map.iter_mut() {
                walk(child, source);
            }
        }
        Value::Array(items) => {
            for item in items.iter_mut() {
                walk(item, source);
            }
        }
        _ => {}
    }
}

fn resolve_path(path: &str, source: &Value) -> Option<Value> {
    let pointer = path_to_json_pointer(path);
    source.pointer(&pointer).cloned()
}

fn path_to_json_pointer(path: &str) -> String {
    let mut out = String::with_capacity(path.len() + 4);
    for segment in path.split('.') {
        out.push('/');
        for ch in segment.chars() {
            match ch {
                '~' => out.push_str("~0"),
                '/' => out.push_str("~1"),
                _ => out.push(ch),
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn source() -> Value {
        json!({
            "data": {"customer_category": "leather wallet brown", "id": 7},
            "variables": {"threshold": 0.3},
            "steps": {"step1": {"outputs": {"first": "alpha"}}}
        })
    }

    #[test]
    fn resolves_top_level_ref_inside_immediate_payload() {
        let value = json!({
            "op": "EQ",
            "arguments": [
                "name",
                {"valueType": "reference", "value": "data.customer_category"}
            ]
        });
        let resolved = resolve_nested_references(value, &source());
        assert_eq!(
            resolved,
            json!({
                "op": "EQ",
                "arguments": ["name", "leather wallet brown"]
            })
        );
    }

    #[test]
    fn nested_ref_inside_arguments() {
        let value = json!({
            "op": "AND",
            "arguments": [
                {"op": "EQ", "arguments": ["status", {"valueType": "reference", "value": "data.id"}]},
                {"op": "SIMILARITY_GTE", "arguments": [
                    "keywords",
                    {"valueType": "reference", "value": "data.customer_category"},
                    0.3
                ]}
            ]
        });
        let resolved = resolve_nested_references(value, &source());
        assert_eq!(
            resolved,
            json!({
                "op": "AND",
                "arguments": [
                    {"op": "EQ", "arguments": ["status", 7]},
                    {"op": "SIMILARITY_GTE", "arguments": [
                        "keywords",
                        "leather wallet brown",
                        0.3
                    ]}
                ]
            })
        );
    }

    #[test]
    fn preserves_immediate_envelopes() {
        let value = json!({
            "valueType": "immediate",
            "value": {"valueType": "reference", "value": "data.id"}
        });
        let resolved = resolve_nested_references(value.clone(), &source());
        // Immediate envelopes are pass-through; their inner value stays as-is.
        assert_eq!(resolved, value);
    }

    #[test]
    fn missing_path_falls_back_to_null() {
        let value = json!({"valueType": "reference", "value": "data.missing"});
        let resolved = resolve_nested_references(value, &source());
        assert_eq!(resolved, Value::Null);
    }

    #[test]
    fn missing_path_uses_provided_default() {
        let value = json!({
            "valueType": "reference",
            "value": "data.missing",
            "default": 42
        });
        let resolved = resolve_nested_references(value, &source());
        assert_eq!(resolved, json!(42));
    }

    #[test]
    fn unrelated_object_passes_through() {
        let value = json!({"foo": "bar", "n": 1, "list": [1, 2]});
        let resolved = resolve_nested_references(value.clone(), &source());
        assert_eq!(resolved, value);
    }
}
