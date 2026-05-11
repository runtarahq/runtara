// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Runtime helper that walks a JSON value tree and resolves any nested
//! `{valueType: "reference", value: "<path>"}` envelopes against the
//! workflow execution context.
//!
//! By the time this pass runs, codegen has already stripped the outer
//! `MappingValue` envelope from each top-level inputMapping field
//! (`emit_immediate_value` emits the inner JSON verbatim). What remains
//! are references buried inside those inlined immediate payloads — for
//! example `ConditionExpression` arguments. Those positions are typed at
//! the agent boundary as `ConditionArgument` (untagged: `Expression |
//! MappingValue`) which only matches *wrapped* values, so resolving a
//! reference there to a bare scalar would crash deserialization with
//! `INPUT_DESERIALIZATION_ERROR: data did not match any variant of
//! untagged enum ConditionArgument`.
//!
//! The fix is to always rewrite a resolved reference as
//! `{valueType: "immediate", value: <resolved>}`. The wrapper preserves
//! the `MappingValue` shape that nested typed deserialisers expect, and
//! the per-field `unwrap_top_level_immediate_envelopes` pass strips
//! exactly one wrapper at the agent input boundary so primitive-typed
//! fields (`String`, `i32`, etc.) still see their bare value.

use serde_json::Value;

/// Recursively replace any `{valueType: "reference", value: "<path>"}` JSON
/// objects inside `value` with `{valueType: "immediate", value: <resolved>}`,
/// where `<resolved>` is the looked-up value from `source`.
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

/// Strip top-level `{valueType: "immediate", value: X}` envelopes from each
/// field of an input object so they can be deserialized into the agent's
/// typed input struct (which expects the inner shape, not the wrapper).
///
/// Pairs with [`resolve_nested_references`]: the resolver wraps every
/// resolved reference as a `MappingValue::Immediate`; this pass strips
/// exactly one wrapper per field so primitive-typed agent inputs (`String`,
/// `i32`, etc.) still see their bare value while nested wrappers inside
/// typed structures (e.g. `ConditionArgument::Value(MappingValue::Immediate)`)
/// survive intact.
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

/// If `value` is exactly `{valueType: "immediate", value: X}`, return `X`.
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
                // Always wrap as MappingValue::Immediate so untagged
                // deserialisers like ConditionArgument keep matching.
                // unwrap_top_level_immediate_envelopes peels the wrapper
                // back off for top-level primitive-typed agent inputs.
                let mut wrapped = serde_json::Map::with_capacity(2);
                wrapped.insert("valueType".to_string(), Value::String("immediate".into()));
                wrapped.insert("value".to_string(), resolved);
                *value = Value::Object(wrapped);
                // Recurse into the freshly resolved payload so any refs
                // inside complex values (object/array) get rewritten too.
                if let Value::Object(m) = value
                    && let Some(inner) = m.get_mut("value")
                {
                    walk(inner, source);
                }
                return;
            }

            let is_immediate_envelope = matches!(
                map.get("valueType"),
                Some(Value::String(s)) if s == "immediate"
            );

            // Skip the immediate envelope itself (its inner payload is
            // intended to pass through verbatim) but still walk the inner
            // value to resolve any references the user nested inside.
            if is_immediate_envelope {
                if let Some(inner) = map.get_mut("value") {
                    walk(inner, source);
                }
                return;
            }

            // Row-level object-model score expressions use `fn` calls whose
            // arguments may mix Object Model column refs and workflow refs:
            //   {fn:"COSINE_DISTANCE", arguments:[
            //      {valueType:"reference", value:"embedding"},
            //      {valueType:"reference", value:"data.query_embedding"}
            //   ]}
            //
            // Unqualified refs are column names and must stay as references
            // for the object-store expression validator. Qualified workflow
            // refs (`data.*`, `steps.*`, etc.) should still resolve.
            let fn_call = map.get("fn").and_then(|v| v.as_str()).map(str::to_owned);
            if fn_call.is_some()
                && let Some(args) = map.get_mut("arguments").and_then(|v| v.as_array_mut())
            {
                for arg in args.iter_mut() {
                    if is_unqualified_reference_envelope(arg) {
                        continue;
                    }
                    walk(arg, source);
                }
                return;
            }

            // Object-model condition payloads use the same MappingValue
            // envelope for two different concepts:
            // - argument 0 of field-based operations names an Object Model
            //   column and must stay as a MappingValue::Reference so the
            //   agent can turn it into a field name;
            // - later arguments are runtime values and should be resolved.
            //
            // Without this positional rule, `{valueType:"reference",
            // value:"category_leaf_id"}` gets looked up as a workflow path and
            // becomes null before the object-model agent can interpret it.
            let condition_op = map.get("op").and_then(|v| v.as_str()).map(str::to_owned);
            if let Some(op) = condition_op.as_deref()
                && let Some(args) = map.get_mut("arguments").and_then(|v| v.as_array_mut())
            {
                for (index, arg) in args.iter_mut().enumerate() {
                    if index == 0 && is_field_argument_operator(op) && is_reference_envelope(arg) {
                        continue;
                    }
                    walk(arg, source);
                }
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

fn is_reference_envelope(value: &Value) -> bool {
    matches!(
        value.get("valueType"),
        Some(Value::String(s)) if s == "reference"
    ) && matches!(value.get("value"), Some(Value::String(_)))
}

fn is_unqualified_reference_envelope(value: &Value) -> bool {
    let Some(path) = value.get("value").and_then(|v| v.as_str()) else {
        return false;
    };
    is_reference_envelope(value) && !is_qualified_workflow_path(path)
}

fn is_qualified_workflow_path(path: &str) -> bool {
    matches!(
        path.split('.').next(),
        Some("data" | "variables" | "workflow" | "steps" | "loop")
    )
}

fn is_field_argument_operator(op: &str) -> bool {
    matches!(
        op.to_ascii_uppercase().as_str(),
        "EQ" | "NE"
            | "GT"
            | "GTE"
            | "LT"
            | "LTE"
            | "STARTS_WITH"
            | "ENDS_WITH"
            | "CONTAINS"
            | "IN"
            | "NOT_IN"
            | "IS_DEFINED"
            | "IS_EMPTY"
            | "IS_NOT_EMPTY"
            | "SIMILARITY_GTE"
            | "MATCH"
            | "COSINE_DISTANCE_LTE"
            | "L2_DISTANCE_LTE"
    )
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
    fn resolves_ref_inline_inside_payload() {
        // After codegen unwraps the outer MappingValue::Immediate, the
        // resolver sees the inner JSON directly. Resolved refs are wrapped
        // as MappingValue::Immediate so untagged ConditionArgument keeps
        // matching at the agent boundary.
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
                "arguments": [
                    "name",
                    {"valueType": "immediate", "value": "leather wallet brown"}
                ]
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
                    {"op": "EQ", "arguments": [
                        "status",
                        {"valueType": "immediate", "value": 7}
                    ]},
                    {"op": "SIMILARITY_GTE", "arguments": [
                        "keywords",
                        {"valueType": "immediate", "value": "leather wallet brown"},
                        0.3
                    ]}
                ]
            })
        );
    }

    #[test]
    fn ref_inside_immediate_is_wrapped() {
        // An immediate envelope's inner value is descended into; nested refs
        // are rewritten as `{valueType: "immediate", value: <resolved>}` so
        // structures like ConditionArgument::Value(MappingValue::Immediate)
        // keep matching.
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
    fn condition_post_codegen_shape() {
        // Reproduce the exact shape the resolver sees in production: codegen
        // has already unwrapped the outer MappingValue::Immediate, so the
        // condition arrives as `{type, op, arguments}` (no `valueType` at the
        // top level). The user-reported failure (`INPUT_DESERIALIZATION_ERROR:
        // ConditionArgument`) was caused by the inner refs resolving to bare
        // null. Field-position references now remain references so the
        // object-model agent can interpret them as column names; value
        // arguments are still resolved and wrapped as immediates.
        let value = json!({
            "type": "operation",
            "op": "COSINE_DISTANCE_LTE",
            "arguments": [
                {"valueType": "reference", "value": "embedding"},
                {"valueType": "reference", "value": "steps.step1.outputs.first"},
                {"valueType": "immediate", "value": 0.6}
            ]
        });
        let resolved = resolve_nested_references(value, &source());
        assert_eq!(
            resolved,
            json!({
                "type": "operation",
                "op": "COSINE_DISTANCE_LTE",
                "arguments": [
                    {"valueType": "reference", "value": "embedding"},
                    {"valueType": "immediate", "value": "alpha"},
                    {"valueType": "immediate", "value": 0.6}
                ]
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
    fn agent_pipeline_end_to_end() {
        // Mirrors the runtime pipeline emitted by agent.rs codegen, AFTER
        // codegen has already unwrapped the outer MappingValue envelope from
        // each top-level field. The remaining job is:
        //   1. resolve_nested_references — wraps every nested ref as
        //      MappingValue::Immediate.
        //   2. unwrap_top_level_immediate_envelopes — strips a single envelope
        //      from each top-level field so primitive-typed agent inputs see
        //      the bare value.
        // This pins the exact wire shape the agent's typed deserialiser sees.
        let inputs = json!({
            "schema_name": "Embedding",
            "limit": 50,
            "condition": {
                "type": "operation",
                "op": "COSINE_DISTANCE_LTE",
                "arguments": [
                    {"valueType": "reference", "value": "embedding"},
                    {"valueType": "reference", "value": "steps.step1.outputs.first"},
                    {"valueType": "immediate", "value": 0.6}
                ]
            }
        });
        let resolved = resolve_nested_references(inputs, &source());
        let final_inputs = unwrap_top_level_immediate_envelopes(resolved);
        assert_eq!(
            final_inputs,
            json!({
                "schema_name": "Embedding",
                "limit": 50,
                "condition": {
                    "type": "operation",
                    "op": "COSINE_DISTANCE_LTE",
                    "arguments": [
                        // Field-position references are column names, not
                        // workflow paths, so they intentionally remain refs.
                        {"valueType": "reference", "value": "embedding"},
                        {"valueType": "immediate", "value": "alpha"},
                        {"valueType": "immediate", "value": 0.6}
                    ]
                }
            })
        );
    }

    #[test]
    fn score_expression_preserves_column_ref_and_resolves_query_ref() {
        let inputs = json!({
            "schema_name": "UnspscNode",
            "score_expression": {
                "alias": "trgm_sim",
                "expression": {
                    "fn": "SIMILARITY",
                    "arguments": [
                        {"valueType": "reference", "value": "commodity_title"},
                        {"valueType": "reference", "value": "data.customer_category"}
                    ]
                }
            }
        });

        let resolved = resolve_nested_references(inputs, &source());
        let final_inputs = unwrap_top_level_immediate_envelopes(resolved);

        assert_eq!(
            final_inputs,
            json!({
                "schema_name": "UnspscNode",
                "score_expression": {
                    "alias": "trgm_sim",
                    "expression": {
                        "fn": "SIMILARITY",
                        "arguments": [
                            {"valueType": "reference", "value": "commodity_title"},
                            {"valueType": "immediate", "value": "leather wallet brown"}
                        ]
                    }
                }
            })
        );
    }

    #[test]
    fn score_expression_preserves_vector_column_ref() {
        let inputs = json!({
            "schema_name": "UnspscNode",
            "score_expression": {
                "alias": "vec_dist",
                "expression": {
                    "fn": "COSINE_DISTANCE",
                    "arguments": [
                        {"valueType": "reference", "value": "embedding"},
                        {"valueType": "reference", "value": "data.query_embedding"}
                    ]
                }
            }
        });
        let source = json!({
            "data": {"query_embedding": [1.0, 0.0, 0.0, 0.0]},
            "variables": {},
            "steps": {}
        });

        let resolved = resolve_nested_references(inputs, &source);
        let final_inputs = unwrap_top_level_immediate_envelopes(resolved);

        assert_eq!(
            final_inputs["score_expression"]["expression"]["arguments"],
            json!([
                {"valueType": "reference", "value": "embedding"},
                {"valueType": "immediate", "value": [1.0, 0.0, 0.0, 0.0]}
            ])
        );
    }

    #[test]
    fn immediate_score_expression_resolves_step_vector_ref() {
        let inputs = json!({
            "score_expression": {
                "valueType": "immediate",
                "value": {
                    "alias": "vec_dist",
                    "expression": {
                        "fn": "COSINE_DISTANCE",
                        "arguments": [
                            {"valueType": "reference", "value": "embedding"},
                            {"valueType": "reference", "value": "steps.embed.outputs.embeddings.0"}
                        ]
                    }
                }
            }
        });
        let source = json!({
            "data": {},
            "variables": {},
            "steps": {
                "embed": {
                    "outputs": {
                        "embeddings": [[1.0, 0.0, 0.0, 0.0]]
                    }
                }
            }
        });

        let resolved = resolve_nested_references(inputs, &source);
        let final_inputs = unwrap_top_level_immediate_envelopes(resolved);

        assert_eq!(
            final_inputs["score_expression"]["expression"]["arguments"],
            json!([
                {"valueType": "reference", "value": "embedding"},
                {"valueType": "immediate", "value": [1.0, 0.0, 0.0, 0.0]}
            ])
        );
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
        // Refs always wrap as MappingValue::Immediate; null payload reflects
        // the missing path.
        assert_eq!(resolved, json!({"valueType": "immediate", "value": null}));
    }

    #[test]
    fn missing_path_uses_provided_default() {
        let value = json!({
            "valueType": "reference",
            "value": "data.missing",
            "default": 42
        });
        let resolved = resolve_nested_references(value, &source());
        assert_eq!(resolved, json!({"valueType": "immediate", "value": 42}));
    }

    #[test]
    fn unrelated_object_passes_through() {
        let value = json!({"foo": "bar", "n": 1, "list": [1, 2]});
        let resolved = resolve_nested_references(value.clone(), &source());
        assert_eq!(resolved, value);
    }
}
