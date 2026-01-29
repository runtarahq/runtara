// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! GroupBy step emitter.
//!
//! The GroupBy step groups array items by a specified key property path.
//! Returns: groups (map of key -> items), counts (map of key -> count), total_groups.

use proc_macro2::TokenStream;
use quote::quote;

use super::super::CodegenError;
use super::super::context::EmitContext;
use super::super::mapping;
use super::{emit_step_debug_end, emit_step_debug_start};
use runtara_dsl::GroupByStep;

/// Emit code for a GroupBy step.
pub fn emit(step: &GroupByStep, ctx: &mut EmitContext) -> Result<TokenStream, CodegenError> {
    let step_id = &step.id;
    let step_name = step.name.as_deref();
    let step_name_display = step_name.unwrap_or("Unnamed");
    let key_path = &step.config.key;

    // Declare variables
    let step_var = ctx.declare_step(step_id);
    let source_var = ctx.temp_var("source");
    let group_input_var = ctx.temp_var("group_input");
    let group_array_var = ctx.temp_var("group_array");
    let groups_map_var = ctx.temp_var("groups_map");
    let counts_map_var = ctx.temp_var("counts_map");
    let item_var = ctx.temp_var("item");
    let key_value_var = ctx.temp_var("key_value");
    let key_str_var = ctx.temp_var("key_str");

    // Clone immutable references
    let steps_context = ctx.steps_context_var.clone();

    // Build the source for input mapping
    let build_source = mapping::emit_build_source(ctx);

    // Emit code to resolve the array value
    let array_value_code = mapping::emit_mapping_value(&step.config.value, ctx, &source_var);

    // Get scenario inputs for debug events
    let scenario_inputs_var = ctx.inputs_var.clone();

    // Convert key path to JSON pointer format for nested access
    let key_pointer = mapping::path_to_json_pointer(key_path);

    // Generate debug events
    let debug_start = emit_step_debug_start(
        ctx,
        step_id,
        step_name,
        "GroupBy",
        Some(&group_input_var),
        None,
        Some(&scenario_inputs_var),
        None,
    );
    let debug_end = emit_step_debug_end(
        ctx,
        step_id,
        step_name,
        "GroupBy",
        Some(&step_var),
        Some(&scenario_inputs_var),
        None,
    );

    // Generate expected keys initialization code
    let expected_keys_init = if let Some(ref keys) = step.config.expected_keys {
        let key_insertions = keys.iter().map(|key| {
            quote! {
                #groups_map_var.entry(#key.to_string()).or_default();
                #counts_map_var.entry(#key.to_string()).or_insert(0);
            }
        });
        quote! { #(#key_insertions)* }
    } else {
        quote! {}
    };

    Ok(quote! {
        let #source_var = #build_source;
        let #group_input_var = #array_value_code;

        // Convert input to array, tolerating null/non-array
        let #group_array_var: Vec<serde_json::Value> = match #group_input_var.as_array() {
            Some(arr) => arr.clone(),
            None => vec![],
        };

        #debug_start

        // Group the array items
        let mut #groups_map_var: std::collections::HashMap<String, Vec<serde_json::Value>> = std::collections::HashMap::new();
        let mut #counts_map_var: std::collections::HashMap<String, usize> = std::collections::HashMap::new();

        // Pre-initialize expected keys with empty values
        #expected_keys_init

        for #item_var in #group_array_var {
            // Extract key value using JSON pointer
            let #key_value_var = #item_var.pointer(#key_pointer).cloned().unwrap_or(serde_json::Value::Null);

            // Convert key to string representation
            let #key_str_var: String = match &#key_value_var {
                serde_json::Value::String(s) => s.clone(),
                serde_json::Value::Number(n) => n.to_string(),
                serde_json::Value::Bool(b) => b.to_string(),
                serde_json::Value::Null => "_null".to_string(),
                // For arrays/objects, serialize to JSON string
                other => serde_json::to_string(other).unwrap_or_else(|_| "_invalid".to_string()),
            };

            // Add to groups
            #groups_map_var.entry(#key_str_var.clone()).or_default().push(#item_var);
            *#counts_map_var.entry(#key_str_var).or_insert(0) += 1;
        }

        let __total_groups = #groups_map_var.len();

        // Convert groups HashMap to JSON Value (object)
        let __groups_json: serde_json::Value = {
            let mut map = serde_json::Map::new();
            for (key, items) in #groups_map_var {
                map.insert(key, serde_json::Value::Array(items));
            }
            serde_json::Value::Object(map)
        };

        // Convert counts HashMap to JSON Value (object)
        let __counts_json: serde_json::Value = {
            let mut map = serde_json::Map::new();
            for (key, count) in #counts_map_var {
                map.insert(key, serde_json::Value::Number(serde_json::Number::from(count)));
            }
            serde_json::Value::Object(map)
        };

        let #step_var = serde_json::json!({
            "stepId": #step_id,
            "stepName": #step_name_display,
            "stepType": "GroupBy",
            "outputs": {
                "groups": __groups_json,
                "counts": __counts_json,
                "total_groups": __total_groups
            }
        });

        #debug_end

        #steps_context.insert(#step_id.to_string(), #step_var.clone());
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codegen::ast::context::EmitContext;
    use runtara_dsl::{GroupByConfig, ImmediateValue, MappingValue, ReferenceValue};

    fn create_group_by_step(step_id: &str, array_path: &str, key_path: &str) -> GroupByStep {
        GroupByStep {
            id: step_id.to_string(),
            name: Some("Test GroupBy".to_string()),
            config: GroupByConfig {
                value: MappingValue::Reference(ReferenceValue {
                    value: array_path.to_string(),
                    type_hint: None,
                    default: None,
                }),
                key: key_path.to_string(),
                expected_keys: None,
            },
        }
    }

    #[test]
    fn test_emit_group_by_basic_structure() {
        let mut ctx = EmitContext::new(false);
        let step = create_group_by_step("group-test", "steps.get-data.outputs.items", "status");

        let tokens = emit(&step, &mut ctx).unwrap();
        let code = tokens.to_string();

        assert!(
            code.contains("groups_map"),
            "Should have groups_map variable"
        );
        assert!(
            code.contains("counts_map"),
            "Should have counts_map variable"
        );
        assert!(code.contains("as_array"), "Should check if input is array");
    }

    #[test]
    fn test_emit_group_by_handles_nested_key() {
        let mut ctx = EmitContext::new(false);
        let step = create_group_by_step("group-nested", "data.items", "user.role");

        let tokens = emit(&step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Should convert nested path to JSON pointer
        assert!(
            code.contains("/user/role"),
            "Should have JSON pointer for nested path"
        );
    }

    #[test]
    fn test_emit_group_by_output_structure() {
        let mut ctx = EmitContext::new(false);
        let step = create_group_by_step("group-output", "data.items", "category");

        let tokens = emit(&step, &mut ctx).unwrap();
        let code = tokens.to_string();

        assert!(
            code.contains("\"groups\""),
            "Should include groups in outputs"
        );
        assert!(
            code.contains("\"counts\""),
            "Should include counts in outputs"
        );
        assert!(
            code.contains("\"total_groups\""),
            "Should include total_groups in outputs"
        );
    }

    #[test]
    fn test_emit_group_by_handles_null_keys() {
        let mut ctx = EmitContext::new(false);
        let step = create_group_by_step("group-null", "data.items", "optional_field");

        let tokens = emit(&step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Should handle null keys by grouping under "_null"
        assert!(code.contains("_null"), "Should handle null keys");
    }

    #[test]
    fn test_emit_group_by_with_debug_mode() {
        let mut ctx = EmitContext::new(true);
        let step = create_group_by_step("group-debug", "data.items", "status");

        let tokens = emit(&step, &mut ctx).unwrap();
        let code = tokens.to_string();

        assert!(
            code.contains("step_debug_start"),
            "Should emit debug start event"
        );
        assert!(
            code.contains("step_debug_end"),
            "Should emit debug end event"
        );
    }

    #[test]
    fn test_emit_group_by_without_debug_mode() {
        let mut ctx = EmitContext::new(false);
        let step = create_group_by_step("group-no-debug", "data.items", "status");

        let tokens = emit(&step, &mut ctx).unwrap();
        let code = tokens.to_string();

        assert!(
            !code.contains("step_debug_start"),
            "Should not emit debug start event"
        );
        assert!(
            !code.contains("step_debug_end"),
            "Should not emit debug end event"
        );
    }

    #[test]
    fn test_emit_group_by_with_immediate_array() {
        let mut ctx = EmitContext::new(false);
        let step = GroupByStep {
            id: "group-immediate".to_string(),
            name: Some("Immediate GroupBy".to_string()),
            config: GroupByConfig {
                value: MappingValue::Immediate(ImmediateValue {
                    value: serde_json::json!([
                        {"status": "active", "name": "Alice"},
                        {"status": "inactive", "name": "Bob"},
                        {"status": "active", "name": "Charlie"}
                    ]),
                }),
                key: "status".to_string(),
                expected_keys: None,
            },
        };

        let tokens = emit(&step, &mut ctx).unwrap();
        let code = tokens.to_string();

        assert!(
            code.contains("groups_map"),
            "Should work with immediate arrays"
        );
    }

    #[test]
    fn test_emit_group_by_stores_in_steps_context() {
        let mut ctx = EmitContext::new(false);
        let step = create_group_by_step("group-store", "data.items", "field");

        let tokens = emit(&step, &mut ctx).unwrap();
        let code = tokens.to_string();

        assert!(
            code.contains("steps_context . insert"),
            "Should store result in steps_context"
        );
    }

    #[test]
    fn test_emit_group_by_with_expected_keys() {
        let mut ctx = EmitContext::new(false);
        let step = GroupByStep {
            id: "group-expected".to_string(),
            name: Some("Expected Keys GroupBy".to_string()),
            config: GroupByConfig {
                value: MappingValue::Reference(ReferenceValue {
                    value: "data.items".to_string(),
                    type_hint: None,
                    default: None,
                }),
                key: "action".to_string(),
                expected_keys: Some(vec![
                    "created".to_string(),
                    "updated".to_string(),
                    "failed".to_string(),
                ]),
            },
        };

        let tokens = emit(&step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Should pre-initialize expected keys before the grouping loop
        assert!(
            code.contains("\"created\""),
            "Should initialize 'created' key"
        );
        assert!(
            code.contains("\"updated\""),
            "Should initialize 'updated' key"
        );
        assert!(
            code.contains("\"failed\""),
            "Should initialize 'failed' key"
        );
        assert!(
            code.contains("or_default"),
            "Should use or_default for groups initialization"
        );
        assert!(
            code.contains("or_insert (0)"),
            "Should use or_insert(0) for counts initialization"
        );
    }
}
