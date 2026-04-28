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
//! is built and before it is serialized to the agent.
//!
//! Two contexts, two behaviors:
//! - **Outside an immediate envelope** (top-level fields, composites): a
//!   resolved `{valueType: "reference", ...}` is replaced with the bare
//!   looked-up value. Agents whose input fields take primitive Rust types
//!   (e.g. `String`, `i32`) deserialize cleanly from the bare form.
//! - **Inside an immediate envelope** (e.g. nested in a `ConditionExpression`
//!   argument): a resolved reference is rewritten to
//!   `{valueType: "immediate", value: <resolved>}`. Agents that expect typed
//!   wire shapes there (e.g. `ConditionArgument` = `Expression | MappingValue`)
//!   then deserialize the resolved arg as `MappingValue::Immediate`. The
//!   `condition_expr_to_json` path on the agent side already extracts
//!   `Immediate.value`, so the bare value reaches the SQL layer unchanged.

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
    walk(&mut value, source, false);
    value
}

/// Strip top-level `{valueType: "immediate", value: X}` envelopes from each
/// field of an input object so they can be deserialized into the agent's
/// typed input struct (which expects the inner shape, not the wrapper).
///
/// Workflow `inputMapping` requires every value to be a `MappingValue`
/// (`{valueType, value}`), but agent input fields are typed against the
/// inner Rust type — e.g. `condition: Option<ConditionExpression>` rather
/// than `MappingValue<ConditionExpression>`. To bridge the two, the codegen
/// runs this helper after `resolve_nested_references`.
///
/// Only the immediate envelope at the *outermost* level of each field is
/// stripped. Nested wrappers inside the value are preserved so things like
/// `ConditionArgument::Value(MappingValue::Immediate)` continue to match.
pub fn unwrap_top_level_immediate_envelopes(mut value: Value) -> Value {
    if let Value::Object(map) = &mut value {
        for child in map.values_mut() {
            if let Some(inner) = take_immediate_inner(child) {
                *child = inner;
            }
        }
    }
    value
}

/// If `value` is exactly `{valueType: "immediate", value: X}` (and no other
/// recognised envelope keys we'd want to keep, like `default`), return `X`.
fn take_immediate_inner(value: &mut Value) -> Option<Value> {
    let Value::Object(map) = value else {
        return None;
    };
    let is_immediate = matches!(
        map.get("valueType"),
        Some(Value::String(s)) if s == "immediate"
    );
    if !is_immediate {
        return None;
    }
    map.remove("value")
}

fn walk(value: &mut Value, source: &Value, inside_immediate: bool) {
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

                if inside_immediate {
                    // Preserve `MappingValue` shape so structures like
                    // `ConditionArgument` can still deserialize. The agent
                    // side then sees `MappingValue::Immediate(resolved)`.
                    let mut wrapped = serde_json::Map::with_capacity(2);
                    wrapped.insert("valueType".to_string(), Value::String("immediate".into()));
                    wrapped.insert("value".to_string(), resolved);
                    *value = Value::Object(wrapped);
                    // Recurse with the same flag so nested refs inside
                    // a complex resolved value (object/array) keep wrapping.
                    if let Value::Object(m) = value
                        && let Some(inner) = m.get_mut("value")
                    {
                        walk(inner, source, inside_immediate);
                    }
                } else {
                    // Top-level / outside-immediate: bare-replace as before.
                    *value = resolved;
                    walk(value, source, inside_immediate);
                }
                return;
            }

            let is_immediate_envelope = matches!(
                map.get("valueType"),
                Some(Value::String(s)) if s == "immediate"
            );

            // Descend into the immediate's `value` so nested refs get resolved.
            // The flag flips on so any refs inside are wrapped, preserving the
            // `MappingValue::Immediate` shape that downstream typed deserialisers
            // (e.g. `ConditionArgument`) expect.
            if is_immediate_envelope {
                if let Some(inner) = map.get_mut("value") {
                    walk(inner, source, true);
                }
                return;
            }

            for (_, child) in map.iter_mut() {
                walk(child, source, inside_immediate);
            }
        }
        Value::Array(items) => {
            for item in items.iter_mut() {
                walk(item, source, inside_immediate);
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
    fn ref_inside_immediate_is_wrapped() {
        // Inside an immediate envelope, a resolved reference is rewritten as
        // `{valueType: "immediate", value: <resolved>}` so structures like
        // `ConditionArgument::Value(MappingValue::Immediate)` keep matching.
        let value = json!({
            "valueType": "immediate",
            "value": {"valueType": "reference", "value": "data.id"}
        });
        let resolved = resolve_nested_references(value, &source());
        assert_eq!(
            resolved,
            json!({
                "valueType": "immediate",
                "value": {"valueType": "immediate", "value": 7}
            })
        );
    }

    #[test]
    fn condition_with_nested_refs_inside_immediate_envelope() {
        // The user-reported failing shape: a condition expression wrapped in
        // an immediate envelope, whose argument list contains nested refs.
        // After resolution, every ref-inside-the-immediate is wrapped as
        // a MappingValue::Immediate so ConditionArgument deserialises cleanly.
        let value = json!({
            "valueType": "immediate",
            "value": {
                "type": "operation",
                "op": "COSINE_DISTANCE_LTE",
                "arguments": [
                    {"valueType": "reference", "value": "data.customer_category"},
                    {"valueType": "reference", "value": "steps.step1.outputs.first"},
                    {"valueType": "immediate", "value": 0.6}
                ]
            }
        });
        let resolved = resolve_nested_references(value, &source());
        assert_eq!(
            resolved,
            json!({
                "valueType": "immediate",
                "value": {
                    "type": "operation",
                    "op": "COSINE_DISTANCE_LTE",
                    "arguments": [
                        {"valueType": "immediate", "value": "leather wallet brown"},
                        {"valueType": "immediate", "value": "alpha"},
                        {"valueType": "immediate", "value": 0.6}
                    ]
                }
            })
        );
    }

    #[test]
    fn unwrap_top_level_strips_immediate_envelope() {
        let input = json!({
            "condition": {
                "valueType": "immediate",
                "value": {"type": "operation", "op": "EQ", "arguments": []}
            },
            "limit": 100,
            "name": {"valueType": "immediate", "value": "Foo"}
        });
        let unwrapped = unwrap_top_level_immediate_envelopes(input);
        assert_eq!(
            unwrapped,
            json!({
                "condition": {"type": "operation", "op": "EQ", "arguments": []},
                "limit": 100,
                "name": "Foo"
            })
        );
    }

    #[test]
    fn unwrap_top_level_leaves_non_immediate_alone() {
        let input = json!({
            "x": {"foo": "bar"},
            "y": [1, 2, 3]
        });
        assert_eq!(unwrap_top_level_immediate_envelopes(input.clone()), input);
    }

    #[test]
    fn unwrap_top_level_ignores_nested_immediates() {
        // Only the outermost wrapper of each field is stripped — nested
        // immediates inside (e.g. inside ConditionExpression arguments)
        // must survive so they can match MappingValue at the agent boundary.
        let input = json!({
            "condition": {
                "valueType": "immediate",
                "value": {
                    "arguments": [
                        {"valueType": "immediate", "value": 0.6}
                    ]
                }
            }
        });
        assert_eq!(
            unwrap_top_level_immediate_envelopes(input),
            json!({
                "condition": {
                    "arguments": [
                        {"valueType": "immediate", "value": 0.6}
                    ]
                }
            })
        );
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
