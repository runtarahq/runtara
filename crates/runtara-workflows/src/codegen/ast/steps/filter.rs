// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Filter step emitter.
//!
//! The Filter step filters an array using a condition expression.
//! For each item in the array, the condition is evaluated with `item.*`
//! references resolving to the current element.

use proc_macro2::TokenStream;
use quote::quote;

use super::super::CodegenError;
use super::super::condition_emitters::emit_condition_expression;
use super::super::context::EmitContext;
use super::super::mapping;
use super::{emit_step_debug_end, emit_step_debug_start};
use runtara_dsl::FilterStep;

/// Emit code for a Filter step.
pub fn emit(step: &FilterStep, ctx: &mut EmitContext) -> Result<TokenStream, CodegenError> {
    let step_id = &step.id;
    let step_name = step.name.as_deref();
    let step_name_display = step_name.unwrap_or("Unnamed");

    // Do all mutable operations first
    let step_var = ctx.declare_step(step_id);
    let source_var = ctx.temp_var("source");
    let filter_input_var = ctx.temp_var("filter_input");
    let filter_array_var = ctx.temp_var("filter_array");
    let filter_results_var = ctx.temp_var("filter_results");
    let item_var = ctx.temp_var("item");
    let item_source_var = ctx.temp_var("item_source");
    let matches_var = ctx.temp_var("matches");

    // Clone immutable references
    let steps_context = ctx.steps_context_var.clone();

    // Build the source for input mapping
    let build_source = mapping::emit_build_source(ctx);

    // Emit code to resolve the array value
    let array_value_code = mapping::emit_mapping_value(&step.config.value, ctx, &source_var);

    // Emit condition evaluation - this will use item_source_var which has `item` injected
    let condition_eval = emit_condition_expression(&step.config.condition, ctx, &item_source_var);

    // Serialize condition to JSON for debug events
    let condition_json = serde_json::to_string(&step.config.condition).ok();

    // Get the scenario inputs variable to access _loop_indices at runtime
    let scenario_inputs_var = ctx.inputs_var.clone();

    // Generate debug event emissions
    let debug_start = emit_step_debug_start(
        ctx,
        step_id,
        step_name,
        "Filter",
        Some(&filter_input_var),
        condition_json.as_deref(),
        Some(&scenario_inputs_var),
        None,
    );
    let debug_end = emit_step_debug_end(
        ctx,
        step_id,
        step_name,
        "Filter",
        Some(&step_var),
        Some(&scenario_inputs_var),
        None,
    );

    Ok(quote! {
        let #source_var = #build_source;
        let #filter_input_var = #array_value_code;

        // Convert input to array, tolerating null/non-array
        let #filter_array_var: Vec<serde_json::Value> = match #filter_input_var.as_array() {
            Some(arr) => arr.clone(),
            None => vec![],
        };

        #debug_start

        // Filter the array
        let mut #filter_results_var: Vec<serde_json::Value> = Vec::new();
        for #item_var in #filter_array_var {
            // Build source with "item" key pointing to current element
            let mut #item_source_var = #source_var.clone();
            if let Some(obj) = #item_source_var.as_object_mut() {
                obj.insert("item".to_string(), #item_var.clone());
            }

            // Evaluate condition with item-aware source
            let #matches_var: bool = #condition_eval;
            if #matches_var {
                #filter_results_var.push(#item_var);
            }
        }

        let __filter_count = #filter_results_var.len();

        let #step_var = serde_json::json!({
            "stepId": #step_id,
            "stepName": #step_name_display,
            "stepType": "Filter",
            "outputs": {
                "items": #filter_results_var,
                "count": __filter_count
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
    use runtara_dsl::{
        ConditionArgument, ConditionExpression, ConditionOperation, ConditionOperator,
        FilterConfig, ImmediateValue, MappingValue, ReferenceValue,
    };

    /// Helper to create a filter step with a simple equality condition.
    fn create_filter_step(
        step_id: &str,
        array_path: &str,
        item_field: &str,
        value: &str,
    ) -> FilterStep {
        FilterStep {
            id: step_id.to_string(),
            name: Some("Test Filter".to_string()),
            config: FilterConfig {
                value: MappingValue::Reference(ReferenceValue {
                    value: array_path.to_string(),
                    type_hint: None,
                    default: None,
                }),
                condition: ConditionExpression::Operation(ConditionOperation {
                    op: ConditionOperator::Eq,
                    arguments: vec![
                        ConditionArgument::Value(MappingValue::Reference(ReferenceValue {
                            value: format!("item.{}", item_field),
                            type_hint: None,
                            default: None,
                        })),
                        ConditionArgument::Value(MappingValue::Immediate(ImmediateValue {
                            value: serde_json::json!(value),
                        })),
                    ],
                }),
            },
        }
    }

    #[test]
    fn test_emit_filter_basic_structure() {
        let mut ctx = EmitContext::new(false);
        let step = create_filter_step(
            "filter-test",
            "steps.get-data.outputs.items",
            "status",
            "active",
        );

        let tokens = emit(&step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Verify basic structure
        assert!(
            code.contains("filter_array"),
            "Should have filter_array variable"
        );
        assert!(
            code.contains("filter_results"),
            "Should have filter_results variable"
        );
        assert!(code.contains("as_array"), "Should check if input is array");
    }

    #[test]
    fn test_emit_filter_tolerates_null() {
        let mut ctx = EmitContext::new(false);
        let step = create_filter_step("filter-null", "steps.missing.outputs", "field", "value");

        let tokens = emit(&step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Should have fallback to empty vec for non-array input
        assert!(
            code.contains("None => vec ! []"),
            "Should fallback to empty vec for null/non-array"
        );
    }

    #[test]
    fn test_emit_filter_injects_item_into_source() {
        let mut ctx = EmitContext::new(false);
        let step = create_filter_step("filter-item", "data.items", "name", "test");

        let tokens = emit(&step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Should inject item into source
        assert!(
            code.contains("\"item\""),
            "Should inject 'item' key into source"
        );
        assert!(
            code.contains("as_object_mut"),
            "Should modify source object"
        );
    }

    #[test]
    fn test_emit_filter_output_structure() {
        let mut ctx = EmitContext::new(false);
        let step = create_filter_step("filter-output", "data.items", "active", "true");

        let tokens = emit(&step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Verify output JSON structure
        assert!(code.contains("\"stepId\""), "Should include stepId");
        assert!(code.contains("\"stepType\""), "Should include stepType");
        assert!(code.contains("\"Filter\""), "Should have stepType = Filter");
        assert!(code.contains("\"outputs\""), "Should include outputs");
        assert!(
            code.contains("\"items\""),
            "Should include items in outputs"
        );
        assert!(
            code.contains("\"count\""),
            "Should include count in outputs"
        );
    }

    #[test]
    fn test_emit_filter_stores_in_steps_context() {
        let mut ctx = EmitContext::new(false);
        let step = create_filter_step("filter-store", "data.items", "field", "value");

        let tokens = emit(&step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Verify result is stored in steps_context
        assert!(
            code.contains("steps_context . insert"),
            "Should store result in steps_context"
        );
    }

    #[test]
    fn test_emit_filter_with_debug_mode() {
        let mut ctx = EmitContext::new(true); // debug mode enabled
        let step = create_filter_step("filter-debug", "data.items", "status", "active");

        let tokens = emit(&step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Should have debug events
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
    fn test_emit_filter_without_debug_mode() {
        let mut ctx = EmitContext::new(false); // debug mode disabled
        let step = create_filter_step("filter-no-debug", "data.items", "status", "active");

        let tokens = emit(&step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Should NOT have debug events
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
    fn test_emit_filter_with_complex_condition() {
        let mut ctx = EmitContext::new(false);

        // Create a filter with AND condition: item.status == "active" AND item.age > 18
        let step = FilterStep {
            id: "filter-complex".to_string(),
            name: Some("Complex Filter".to_string()),
            config: FilterConfig {
                value: MappingValue::Reference(ReferenceValue {
                    value: "data.users".to_string(),
                    type_hint: None,
                    default: None,
                }),
                condition: ConditionExpression::Operation(ConditionOperation {
                    op: ConditionOperator::And,
                    arguments: vec![
                        ConditionArgument::Expression(Box::new(ConditionExpression::Operation(
                            ConditionOperation {
                                op: ConditionOperator::Eq,
                                arguments: vec![
                                    ConditionArgument::Value(MappingValue::Reference(
                                        ReferenceValue {
                                            value: "item.status".to_string(),
                                            type_hint: None,
                                            default: None,
                                        },
                                    )),
                                    ConditionArgument::Value(MappingValue::Immediate(
                                        ImmediateValue {
                                            value: serde_json::json!("active"),
                                        },
                                    )),
                                ],
                            },
                        ))),
                        ConditionArgument::Expression(Box::new(ConditionExpression::Operation(
                            ConditionOperation {
                                op: ConditionOperator::Gt,
                                arguments: vec![
                                    ConditionArgument::Value(MappingValue::Reference(
                                        ReferenceValue {
                                            value: "item.age".to_string(),
                                            type_hint: None,
                                            default: None,
                                        },
                                    )),
                                    ConditionArgument::Value(MappingValue::Immediate(
                                        ImmediateValue {
                                            value: serde_json::json!(18),
                                        },
                                    )),
                                ],
                            },
                        ))),
                    ],
                }),
            },
        };

        let tokens = emit(&step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Should have AND logic
        assert!(
            code.contains("&&"),
            "Should have AND operator for complex condition"
        );
    }

    #[test]
    fn test_emit_filter_with_immediate_array() {
        let mut ctx = EmitContext::new(false);

        // Create a filter with an immediate array value
        let step = FilterStep {
            id: "filter-immediate".to_string(),
            name: Some("Immediate Array Filter".to_string()),
            config: FilterConfig {
                value: MappingValue::Immediate(ImmediateValue {
                    value: serde_json::json!([
                        {"status": "active", "name": "Alice"},
                        {"status": "inactive", "name": "Bob"}
                    ]),
                }),
                condition: ConditionExpression::Operation(ConditionOperation {
                    op: ConditionOperator::Eq,
                    arguments: vec![
                        ConditionArgument::Value(MappingValue::Reference(ReferenceValue {
                            value: "item.status".to_string(),
                            type_hint: None,
                            default: None,
                        })),
                        ConditionArgument::Value(MappingValue::Immediate(ImmediateValue {
                            value: serde_json::json!("active"),
                        })),
                    ],
                }),
            },
        };

        let tokens = emit(&step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Should handle immediate array
        assert!(
            code.contains("filter_array"),
            "Should have filter loop even with immediate array"
        );
    }
}
