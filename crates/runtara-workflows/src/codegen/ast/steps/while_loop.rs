// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! While step emitter.
//!
//! The While step executes a subgraph repeatedly while a condition is true.
//! Each iteration produces a heartbeat to maintain instance liveness.
//! The loop has a configurable maximum iteration limit (default: 10) to prevent infinite loops.

use proc_macro2::TokenStream;
use quote::quote;

use super::super::CodegenError;
use super::super::context::EmitContext;
use super::super::mapping;
use super::super::program;
use super::conditional::emit_condition_expression;
use super::{emit_step_debug_end, emit_step_debug_start};
use runtara_dsl::WhileStep;

/// Emit code for a While step.
pub fn emit(step: &WhileStep, ctx: &mut EmitContext) -> Result<TokenStream, CodegenError> {
    #![allow(clippy::too_many_lines)]
    let step_id = &step.id;
    let step_name = step.name.as_deref();
    let step_name_display = step_name.unwrap_or("Unnamed");

    // Get config with defaults
    let max_iterations = step
        .config
        .as_ref()
        .and_then(|c| c.max_iterations)
        .unwrap_or(10);

    // Do all mutable operations first
    let step_var = ctx.declare_step(step_id);
    let source_var = ctx.temp_var("source");
    let loop_inputs_var = ctx.temp_var("loop_inputs");
    let subgraph_fn_name = ctx.temp_var(&format!(
        "{}_subgraph",
        EmitContext::sanitize_ident(step_id)
    ));

    // Clone immutable references
    let steps_context = ctx.steps_context_var.clone();
    let inputs_var = ctx.inputs_var.clone();

    // Build the source for input mapping
    let build_source = mapping::emit_build_source(ctx);

    // Generate the subgraph function using shared recursive emitter
    let subgraph_code = program::emit_graph_as_function(&subgraph_fn_name, &step.subgraph, ctx)?;

    // Generate condition evaluation code
    let condition_eval = emit_condition_expression(&step.condition, ctx, &source_var);

    // Serialize condition to JSON for debug events
    let condition_json = serde_json::to_string(&step.condition).ok();

    // Clone scenario inputs var for debug events (to access _loop_indices)
    let scenario_inputs_var = inputs_var.clone();

    // While creates a scope - use sc_{step_id} as its scope_id
    let while_scope_id = format!("sc_{}", step_id);

    // Generate debug event emissions with the While's own scope_id
    let debug_start = emit_step_debug_start(
        ctx,
        step_id,
        step_name,
        "While",
        Some(&loop_inputs_var),
        condition_json.as_deref(),
        Some(&scenario_inputs_var),
        Some(&while_scope_id),
    );
    let debug_end = emit_step_debug_end(
        ctx,
        step_id,
        step_name,
        "While",
        Some(&step_var),
        Some(&scenario_inputs_var),
        Some(&while_scope_id),
    );

    Ok(quote! {
        let #source_var = #build_source;
        let #loop_inputs_var = serde_json::json!({"maxIterations": #max_iterations});

        #debug_start

        // Define the subgraph function
        #subgraph_code

        // While loop execution
        let #step_var = {
            let mut __loop_index: u32 = 0;
            let mut __loop_outputs: serde_json::Value = serde_json::Value::Null;
            let __max_iterations: u32 = #max_iterations;

            loop {
                // Check max iterations limit
                if __loop_index >= __max_iterations {
                    eprintln!("While step '{}' reached max iterations limit ({})", #step_id, __max_iterations);
                    break;
                }

                // Build source with loop context for condition evaluation
                let mut __loop_source = #source_var.clone();
                if let serde_json::Value::Object(ref mut map) = __loop_source {
                    let mut loop_ctx = serde_json::Map::new();
                    loop_ctx.insert("index".to_string(), serde_json::json!(__loop_index));
                    loop_ctx.insert("outputs".to_string(), __loop_outputs.clone());
                    map.insert("loop".to_string(), serde_json::Value::Object(loop_ctx));
                }
                let #source_var = __loop_source;

                // Evaluate condition
                let __condition_result: bool = #condition_eval;

                if !__condition_result {
                    break;
                }

                // Prepare subgraph inputs with loop context
                let mut __loop_vars = match (*#inputs_var.variables).clone() {
                    serde_json::Value::Object(m) => m,
                    _ => serde_json::Map::new(),
                };

                // Build cumulative loop indices array for cache key uniqueness in nested loops
                let __parent_indices = __loop_vars.get("_loop_indices")
                    .and_then(|v| v.as_array())
                    .cloned()
                    .unwrap_or_default();
                let mut __all_indices = __parent_indices;
                __all_indices.push(serde_json::json!(__loop_index));
                __loop_vars.insert("_loop_indices".to_string(), serde_json::json!(__all_indices));

                // Inject iteration index as _index for backward compatibility
                __loop_vars.insert("_index".to_string(), serde_json::json!(__loop_index));

                // Include previous iteration outputs in variables
                if !__loop_outputs.is_null() {
                    __loop_vars.insert("_previousOutputs".to_string(), __loop_outputs.clone());
                }

                // Generate scope ID for this iteration
                let __iteration_scope_id = {
                    let parent_scope = __loop_vars.get("_scope_id")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());

                    if let Some(parent) = parent_scope {
                        format!("{}_{}_{}", parent, #step_id, __loop_index)
                    } else {
                        format!("sc_{}_{}", #step_id, __loop_index)
                    }
                };

                // Inject _scope_id into subgraph variables (iteration-specific for cache key uniqueness)
                __loop_vars.insert("_scope_id".to_string(), serde_json::json!(__iteration_scope_id.clone()));

                // Inner steps use the While's scope (sc_{step_id}) as their parent, NOT the iteration scope.
                // This ensures all iterations share the same parent scope for hierarchy queries,
                // while still having unique scope_ids for cache key differentiation.
                let __while_scope_id = format!("sc_{}", #step_id);
                let __subgraph_inputs = ScenarioInputs {
                    data: #inputs_var.data.clone(),
                    variables: Arc::new(serde_json::Value::Object(__loop_vars)),
                    parent_scope_id: Some(__while_scope_id),
                };

                // Check for cancellation before executing subgraph
                if runtara_sdk::is_cancelled() {
                    return Err(format!("While step {} cancelled before iteration {}", #step_id, __loop_index));
                }

                // Execute subgraph with cancellation support
                __loop_outputs = match runtara_sdk::with_cancellation(
                    #subgraph_fn_name(Arc::new(__subgraph_inputs))
                ).await {
                    Ok(result) => result?,
                    Err(cancel_err) => {
                        return Err(format!("While step {} cancelled at iteration {}: {}", #step_id, __loop_index, cancel_err));
                    }
                };

                __loop_index += 1;

                // Heartbeat after each iteration to maintain liveness
                {
                    let __sdk = sdk().lock().await;
                    let _ = __sdk.heartbeat().await;
                }

                // Also check for cancellation or pause after each iteration (belt and suspenders)
                {
                    let mut __sdk = sdk().lock().await;
                    if let Err(e) = __sdk.check_signals().await {
                        return Err(format!("While step {} at iteration {}: {}", #step_id, __loop_index, e));
                    }
                }
            }

            serde_json::json!({
                "stepId": #step_id,
                "stepName": #step_name_display,
                "stepType": "While",
                "iterations": __loop_index,
                "outputs": __loop_outputs
            })
        };

        #debug_end

        #steps_context.insert(#step_id.to_string(), #step_var.clone());
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtara_dsl::{
        ConditionArgument, ConditionExpression, ConditionOperation, ConditionOperator,
        ExecutionGraph, FinishStep, ImmediateValue, MappingValue, ReferenceValue, Step,
        WhileConfig,
    };
    use std::collections::HashMap;

    /// Helper to create a minimal ExecutionGraph with just a Finish step
    fn create_minimal_graph(entry_point: &str) -> ExecutionGraph {
        let mut steps = HashMap::new();
        steps.insert(
            entry_point.to_string(),
            Step::Finish(FinishStep {
                id: entry_point.to_string(),
                name: Some("Finish".to_string()),
                input_mapping: None,
            }),
        );

        ExecutionGraph {
            name: None,
            description: None,
            entry_point: entry_point.to_string(),
            steps,
            execution_plan: vec![],
            variables: HashMap::new(),
            input_schema: HashMap::new(),
            output_schema: HashMap::new(),
            notes: None,
            nodes: None,
            edges: None,
        }
    }

    /// Helper to create a simple while step with loop.index < N condition
    fn create_while_step(step_id: &str, max_iterations: Option<u32>, limit: i64) -> WhileStep {
        let condition = ConditionExpression::Operation(ConditionOperation {
            op: ConditionOperator::Lt,
            arguments: vec![
                ConditionArgument::Value(MappingValue::Reference(ReferenceValue {
                    value: "loop.index".to_string(),
                    type_hint: None,
                    default: None,
                })),
                ConditionArgument::Value(MappingValue::Immediate(ImmediateValue {
                    value: serde_json::json!(limit),
                })),
            ],
        });

        WhileStep {
            id: step_id.to_string(),
            name: Some("Test While".to_string()),
            condition,
            config: max_iterations.map(|m| WhileConfig {
                max_iterations: Some(m),
                timeout: None,
            }),
            subgraph: Box::new(create_minimal_graph("finish")),
        }
    }

    #[test]
    fn test_emit_basic_while_structure() {
        let mut ctx = EmitContext::new(false);
        let while_step = create_while_step("while-1", None, 5);

        let tokens = emit(&while_step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Verify basic structure
        assert!(code.contains("while-1"), "Should contain step ID");
        assert!(
            code.contains("__loop_index"),
            "Should have loop index variable"
        );
        assert!(
            code.contains("__max_iterations"),
            "Should have max iterations check"
        );
        assert!(code.contains("__loop_outputs"), "Should track loop outputs");
    }

    #[test]
    fn test_emit_while_default_max_iterations() {
        let mut ctx = EmitContext::new(false);
        // No config = default max_iterations of 10
        let while_step = WhileStep {
            id: "while-default".to_string(),
            name: None,
            condition: ConditionExpression::Value(MappingValue::Immediate(ImmediateValue {
                value: serde_json::json!(true),
            })),
            config: None,
            subgraph: Box::new(create_minimal_graph("finish")),
        };

        let tokens = emit(&while_step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Default max_iterations is 10
        assert!(
            code.contains("10u32") || code.contains("10 u32") || code.contains(": u32 = 10"),
            "Should use default max_iterations of 10"
        );
    }

    #[test]
    fn test_emit_while_custom_max_iterations() {
        let mut ctx = EmitContext::new(false);
        let while_step = create_while_step("while-custom", Some(25), 100);

        let tokens = emit(&while_step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Should use custom max_iterations
        assert!(
            code.contains("25u32") || code.contains("25 u32") || code.contains(": u32 = 25"),
            "Should use custom max_iterations of 25"
        );
    }

    #[test]
    fn test_emit_while_loop_context_injection() {
        let mut ctx = EmitContext::new(false);
        let while_step = create_while_step("while-ctx", Some(5), 3);

        let tokens = emit(&while_step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Verify loop context is injected
        assert!(
            code.contains("\"index\""),
            "Should inject index into loop context"
        );
        assert!(
            code.contains("\"outputs\""),
            "Should inject outputs into loop context"
        );
        assert!(
            code.contains("\"loop\""),
            "Should create loop context object"
        );
    }

    #[test]
    fn test_emit_while_loop_indices_for_cache_key() {
        let mut ctx = EmitContext::new(false);
        let while_step = create_while_step("while-indices", Some(5), 3);

        let tokens = emit(&while_step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Verify _loop_indices is tracked for nested loops
        assert!(
            code.contains("_loop_indices"),
            "Should track _loop_indices for cache key uniqueness"
        );
        assert!(
            code.contains("__parent_indices"),
            "Should preserve parent loop indices"
        );
        assert!(
            code.contains("__all_indices"),
            "Should build cumulative indices"
        );
    }

    #[test]
    fn test_emit_while_backward_compat_index() {
        let mut ctx = EmitContext::new(false);
        let while_step = create_while_step("while-compat", Some(5), 3);

        let tokens = emit(&while_step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Verify _index is injected for backward compatibility
        assert!(
            code.contains("\"_index\""),
            "Should inject _index for backward compatibility"
        );
    }

    #[test]
    fn test_emit_while_previous_outputs() {
        let mut ctx = EmitContext::new(false);
        let while_step = create_while_step("while-prev", Some(5), 3);

        let tokens = emit(&while_step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Verify previous outputs are passed to next iteration
        assert!(
            code.contains("_previousOutputs"),
            "Should pass _previousOutputs to subgraph"
        );
    }

    #[test]
    fn test_emit_while_heartbeat() {
        let mut ctx = EmitContext::new(false);
        let while_step = create_while_step("while-hb", Some(5), 3);

        let tokens = emit(&while_step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Verify heartbeat is sent after each iteration
        assert!(
            code.contains("heartbeat"),
            "Should emit heartbeat after each iteration"
        );
    }

    #[test]
    fn test_emit_while_signal_check() {
        let mut ctx = EmitContext::new(false);
        let while_step = create_while_step("while-cancel", Some(5), 3);

        let tokens = emit(&while_step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Verify signals (cancel/pause) are checked
        assert!(
            code.contains("check_signals"),
            "Should check for signals (cancel/pause) after each iteration"
        );
    }

    #[test]
    fn test_emit_while_subgraph_function() {
        let mut ctx = EmitContext::new(false);
        let while_step = create_while_step("while-subgraph", Some(5), 3);

        let tokens = emit(&while_step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Verify subgraph function is generated
        assert!(
            code.contains("_subgraph"),
            "Should generate subgraph function"
        );
        assert!(
            code.contains("ScenarioInputs"),
            "Should use ScenarioInputs for subgraph"
        );
    }

    #[test]
    fn test_emit_while_output_structure() {
        let mut ctx = EmitContext::new(false);
        let while_step = create_while_step("while-output", Some(5), 3);

        let tokens = emit(&while_step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Verify output structure
        assert!(
            code.contains("\"stepId\""),
            "Should include stepId in output"
        );
        assert!(
            code.contains("\"stepType\""),
            "Should include stepType in output"
        );
        assert!(code.contains("\"While\""), "Should have stepType = While");
        assert!(
            code.contains("\"iterations\""),
            "Should include iterations count"
        );
        assert!(
            code.contains("\"outputs\""),
            "Should include outputs in result"
        );
    }

    #[test]
    fn test_emit_while_stores_in_steps_context() {
        let mut ctx = EmitContext::new(false);
        let while_step = create_while_step("while-store", Some(5), 3);

        let tokens = emit(&while_step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Verify result is stored in steps_context
        assert!(
            code.contains("steps_context . insert"),
            "Should store result in steps_context"
        );
    }

    #[test]
    fn test_emit_while_debug_mode_enabled() {
        let mut ctx = EmitContext::new(true); // debug mode ON
        let while_step = create_while_step("while-debug", Some(5), 3);

        let tokens = emit(&while_step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Verify debug events are emitted
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
    fn test_emit_while_debug_mode_disabled() {
        let mut ctx = EmitContext::new(false); // debug mode OFF
        let while_step = create_while_step("while-no-debug", Some(5), 3);

        let tokens = emit(&while_step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Debug events should not be present (or minimal)
        // The debug functions return empty tokens when debug_mode is false
        // So we just verify the core loop logic is present
        assert!(code.contains("loop {"), "Should have loop structure");
    }

    #[test]
    fn test_emit_while_with_unnamed_step() {
        let mut ctx = EmitContext::new(false);
        let while_step = WhileStep {
            id: "while-unnamed".to_string(),
            name: None, // No name
            condition: ConditionExpression::Value(MappingValue::Immediate(ImmediateValue {
                value: serde_json::json!(true),
            })),
            config: Some(WhileConfig {
                max_iterations: Some(3),
                timeout: None,
            }),
            subgraph: Box::new(create_minimal_graph("finish")),
        };

        let tokens = emit(&while_step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Should use "Unnamed" as display name
        assert!(
            code.contains("\"Unnamed\""),
            "Should use 'Unnamed' for unnamed steps"
        );
    }

    #[test]
    fn test_emit_while_condition_evaluation() {
        let mut ctx = EmitContext::new(false);

        // Complex condition: loop.index < 5 AND loop.outputs != null
        let condition = ConditionExpression::Operation(ConditionOperation {
            op: ConditionOperator::And,
            arguments: vec![
                ConditionArgument::Expression(Box::new(ConditionExpression::Operation(
                    ConditionOperation {
                        op: ConditionOperator::Lt,
                        arguments: vec![
                            ConditionArgument::Value(MappingValue::Reference(ReferenceValue {
                                value: "loop.index".to_string(),
                                type_hint: None,
                                default: None,
                            })),
                            ConditionArgument::Value(MappingValue::Immediate(ImmediateValue {
                                value: serde_json::json!(5),
                            })),
                        ],
                    },
                ))),
                ConditionArgument::Expression(Box::new(ConditionExpression::Operation(
                    ConditionOperation {
                        op: ConditionOperator::IsDefined,
                        arguments: vec![ConditionArgument::Value(MappingValue::Reference(
                            ReferenceValue {
                                value: "loop.outputs".to_string(),
                                type_hint: None,
                                default: None,
                            },
                        ))],
                    },
                ))),
            ],
        });

        let while_step = WhileStep {
            id: "while-complex".to_string(),
            name: Some("Complex Condition".to_string()),
            condition,
            config: Some(WhileConfig {
                max_iterations: Some(10),
                timeout: None,
            }),
            subgraph: Box::new(create_minimal_graph("finish")),
        };

        let tokens = emit(&while_step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Verify condition is evaluated
        assert!(
            code.contains("__condition_result"),
            "Should store condition result"
        );
        assert!(
            code.contains("if ! __condition_result"),
            "Should break on false condition"
        );
    }
}
