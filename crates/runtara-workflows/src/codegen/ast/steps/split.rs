// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Split step emitter.
//!
//! The Split step iterates over an array, executing a subgraph for each item.
//! The Split step uses #[durable] macro to checkpoint its final result.
//! Individual steps within the subgraph checkpoint themselves via runtara-sdk,
//! enabling recovery mid-iteration.

use proc_macro2::TokenStream;
use quote::quote;

use super::super::context::EmitContext;
use super::super::mapping;
use super::super::program;
use super::{emit_step_debug_end, emit_step_debug_start};
use runtara_dsl::{MappingValue, SplitStep};

/// Emit code for a Split step.
pub fn emit(step: &SplitStep, ctx: &mut EmitContext) -> TokenStream {
    let step_id = &step.id;
    let step_name = step.name.as_deref();
    let step_name_display = step_name.unwrap_or("Unnamed");

    // Get retry configuration with defaults (0 retries for Split by default)
    let max_retries = step
        .config
        .as_ref()
        .and_then(|c| c.max_retries)
        .unwrap_or(0);
    let retry_delay = step
        .config
        .as_ref()
        .and_then(|c| c.retry_delay)
        .unwrap_or(1000);

    // Do all mutable operations first
    let step_var = ctx.declare_step(step_id);
    let source_var = ctx.temp_var("source");
    let split_inputs_var = ctx.temp_var("split_inputs");
    let subgraph_fn_name = ctx.temp_var(&format!(
        "{}_subgraph",
        EmitContext::sanitize_ident(step_id)
    ));
    let durable_fn_name =
        ctx.temp_var(&format!("{}_durable", EmitContext::sanitize_ident(step_id)));

    // Clone immutable references
    let steps_context = ctx.steps_context_var.clone();
    let inputs_var = ctx.inputs_var.clone();

    // Build the source for input mapping
    let build_source = mapping::emit_build_source(ctx);

    // Build inputs from the typed SplitConfig
    let inputs_code = if let Some(ref config) = step.config {
        // Emit mapping code for the value field
        let value_mapping: std::collections::HashMap<String, MappingValue> =
            [("value".to_string(), config.value.clone())]
                .into_iter()
                .collect();

        let mapping_code = mapping::emit_input_mapping(&value_mapping, ctx, &source_var);

        // Generate code to resolve variables mapping if present
        let variables_code = if let Some(ref vars) = config.variables {
            let vars_mapping_code = mapping::emit_input_mapping(vars, ctx, &source_var);
            quote! {
                if let serde_json::Value::Object(ref mut map) = inputs {
                    map.insert("variables".to_string(), #vars_mapping_code);
                }
            }
        } else {
            quote! {}
        };

        // Emit the parallelism, sequential, and dontStopOnFailed as immediate values
        let parallelism = config.parallelism.unwrap_or(0);
        let sequential = config.sequential.unwrap_or(false);
        let dont_stop_on_failed = config.dont_stop_on_failed.unwrap_or(false);

        quote! {
            {
                let mut inputs = #mapping_code;
                if let serde_json::Value::Object(ref mut map) = inputs {
                    map.insert("parallelism".to_string(), serde_json::json!(#parallelism));
                    map.insert("sequential".to_string(), serde_json::json!(#sequential));
                    map.insert("dontStopOnFailed".to_string(), serde_json::json!(#dont_stop_on_failed));
                }
                #variables_code
                inputs
            }
        }
    } else {
        quote! { serde_json::Value::Object(serde_json::Map::new()) }
    };

    // Generate the subgraph function using shared recursive emitter
    let subgraph_code = program::emit_graph_as_function(&subgraph_fn_name, &step.subgraph, ctx);

    // Serialize config to JSON for debug events
    let config_json = step
        .config
        .as_ref()
        .and_then(|c| serde_json::to_string(c).ok());

    // Clone scenario inputs var for debug events (to access _loop_indices)
    let scenario_inputs_var = inputs_var.clone();

    // Generate debug event emissions
    let debug_start = emit_step_debug_start(
        ctx,
        step_id,
        step_name,
        "Split",
        Some(&split_inputs_var),
        config_json.as_deref(),
        Some(&scenario_inputs_var),
    );
    let debug_end = emit_step_debug_end(
        ctx,
        step_id,
        step_name,
        "Split",
        Some(&step_var),
        Some(&scenario_inputs_var),
    );

    // Cache key for the split step's final result checkpoint
    let cache_key = format!("split::{}", step_id);

    // Generate the durable function with configurable retry settings
    let max_retries_lit = max_retries;
    let retry_delay_lit = retry_delay;

    quote! {
        let #source_var = #build_source;
        let #split_inputs_var = #inputs_code;

        #debug_start

        // Extract split configuration
        let split_array = #split_inputs_var.get("value")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_else(|| {
                eprintln!("ERROR: Split step 'value' must be an array. Got: {}",
                    #split_inputs_var.get("value").unwrap_or(&serde_json::Value::Null));
                vec![]
            });

        let _parallelism = #split_inputs_var.get("parallelism")
            .and_then(|v| v.as_i64())
            .unwrap_or(0) as usize;
        let _sequential = #split_inputs_var.get("sequential")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let dont_stop_on_failed = #split_inputs_var.get("dontStopOnFailed")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        // Extract extra variables from input mapping (for passing config to subgraphs)
        let extra_variables = #split_inputs_var.get("variables")
            .cloned()
            .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));

        // Define the subgraph function
        #subgraph_code

        // Define the durable split execution function
        #[durable(max_retries = #max_retries_lit, delay = #retry_delay_lit)]
        async fn #durable_fn_name(
            cache_key: &str,
            split_array: Vec<serde_json::Value>,
            variables_base: serde_json::Value,
            extra_variables: serde_json::Value,
            dont_stop_on_failed: bool,
            step_id: &str,
            step_name: &str,
        ) -> std::result::Result<serde_json::Value, String> {
            let mut results: Vec<serde_json::Value> = Vec::with_capacity(split_array.len());
            let mut errors: Vec<serde_json::Value> = Vec::new();

            for (idx, item) in split_array.iter().enumerate() {
                // Each iteration: data = current item, variables = inherited + extra from mapping + _index
                let mut merged_vars = match &variables_base {
                    serde_json::Value::Object(base) => base.clone(),
                    _ => serde_json::Map::new(),
                };
                if let serde_json::Value::Object(extra) = &extra_variables {
                    for (k, v) in extra {
                        merged_vars.insert(k.clone(), v.clone());
                    }
                }

                // Build cumulative loop indices array for cache key uniqueness in nested loops
                let parent_indices = merged_vars.get("_loop_indices")
                    .and_then(|v| v.as_array())
                    .cloned()
                    .unwrap_or_default();
                let mut all_indices = parent_indices;
                all_indices.push(serde_json::json!(idx));
                merged_vars.insert("_loop_indices".to_string(), serde_json::json!(all_indices));

                // Inject iteration index as _index (0-based) for backward compatibility
                merged_vars.insert("_index".to_string(), serde_json::json!(idx));

                let subgraph_inputs = ScenarioInputs {
                    data: Arc::new(item.clone()),
                    variables: Arc::new(serde_json::Value::Object(merged_vars)),
                };

                match #subgraph_fn_name(Arc::new(subgraph_inputs)).await {
                    Ok(result) => results.push(result),
                    Err(e) => {
                        if dont_stop_on_failed {
                            errors.push(serde_json::json!({"error": e, "index": idx}));
                        } else {
                            eprintln!("ERROR in split iteration {}: {}", idx, e);
                            return Err(format!("Split step failed at index {}: {}", idx, e));
                        }
                    }
                }

                // Check for cancellation after each iteration
                {
                    let mut __sdk = sdk().lock().await;
                    if let Err(e) = __sdk.check_cancelled().await {
                        return Err(format!("Split step {} cancelled at iteration {}: {}", step_id, idx, e));
                    }
                }
            }

            let step_result = if dont_stop_on_failed {
                serde_json::json!({
                    "stepId": step_id,
                    "stepName": step_name,
                    "stepType": "Split",
                    "data": {
                        "success": &results,
                        "error": &errors,
                        "aborted": [],
                        "unknown": [],
                        "skipped": []
                    },
                    "stats": {
                        "success": results.len(),
                        "error": errors.len(),
                        "aborted": 0,
                        "unknown": 0,
                        "skipped": 0,
                        "total": split_array.len()
                    },
                    "outputs": &results
                })
            } else {
                serde_json::json!({
                    "stepId": step_id,
                    "stepName": step_name,
                    "stepType": "Split",
                    "outputs": &results
                })
            };

            Ok(step_result)
        }

        // Execute the durable split function
        let variables_base = (*#inputs_var.variables).clone();
        let #step_var = #durable_fn_name(
            #cache_key,
            split_array,
            variables_base,
            extra_variables,
            dont_stop_on_failed,
            #step_id,
            #step_name_display,
        ).await?;

        #debug_end

        #steps_context.insert(#step_id.to_string(), #step_var.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtara_dsl::{
        ExecutionGraph, FinishStep, ImmediateValue, MappingValue, ReferenceValue, SplitConfig, Step,
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

    /// Helper to create a split step with array reference
    fn create_split_step(step_id: &str, array_ref: &str) -> SplitStep {
        SplitStep {
            id: step_id.to_string(),
            name: Some("Test Split".to_string()),
            config: Some(SplitConfig {
                value: MappingValue::Reference(ReferenceValue {
                    value: array_ref.to_string(),
                    type_hint: None,
                    default: None,
                }),
                variables: None,
                parallelism: None,
                sequential: None,
                dont_stop_on_failed: None,
                max_retries: None,
                retry_delay: None,
                timeout: None,
            }),
            subgraph: Box::new(create_minimal_graph("finish")),
            input_schema: HashMap::new(),
            output_schema: HashMap::new(),
        }
    }

    #[test]
    fn test_emit_basic_split_structure() {
        let mut ctx = EmitContext::new(false);
        let split_step = create_split_step("split-1", "data.items");

        let tokens = emit(&split_step, &mut ctx);
        let code = tokens.to_string();

        // Verify basic structure
        assert!(code.contains("split-1"), "Should contain step ID");
        assert!(code.contains("split_array"), "Should extract split array");
        assert!(code.contains("_durable"), "Should have durable function");
    }

    #[test]
    fn test_emit_split_default_config() {
        let mut ctx = EmitContext::new(false);
        let split_step = SplitStep {
            id: "split-default".to_string(),
            name: None,
            config: None, // No config
            subgraph: Box::new(create_minimal_graph("finish")),
            input_schema: HashMap::new(),
            output_schema: HashMap::new(),
        };

        let tokens = emit(&split_step, &mut ctx);
        let code = tokens.to_string();

        // Should have empty object as inputs when no config
        assert!(
            code.contains("serde_json :: Value :: Object"),
            "Should create empty object when no config"
        );
    }

    #[test]
    fn test_emit_split_parallelism_config() {
        let mut ctx = EmitContext::new(false);
        let split_step = SplitStep {
            id: "split-parallel".to_string(),
            name: Some("Parallel Split".to_string()),
            config: Some(SplitConfig {
                value: MappingValue::Reference(ReferenceValue {
                    value: "data.items".to_string(),
                    type_hint: None,
                    default: None,
                }),
                variables: None,
                parallelism: Some(4),
                sequential: Some(false),
                dont_stop_on_failed: Some(true),
                max_retries: None,
                retry_delay: None,
                timeout: None,
            }),
            subgraph: Box::new(create_minimal_graph("finish")),
            input_schema: HashMap::new(),
            output_schema: HashMap::new(),
        };

        let tokens = emit(&split_step, &mut ctx);
        let code = tokens.to_string();

        // Verify parallelism config is included
        assert!(
            code.contains("\"parallelism\""),
            "Should include parallelism config"
        );
        assert!(
            code.contains("\"sequential\""),
            "Should include sequential config"
        );
        assert!(
            code.contains("\"dontStopOnFailed\""),
            "Should include dontStopOnFailed config"
        );
    }

    #[test]
    fn test_emit_split_variables_mapping() {
        let mut ctx = EmitContext::new(false);

        let mut variables = HashMap::new();
        variables.insert(
            "customVar".to_string(),
            MappingValue::Immediate(ImmediateValue {
                value: serde_json::json!("test-value"),
            }),
        );

        let split_step = SplitStep {
            id: "split-vars".to_string(),
            name: Some("Split with Variables".to_string()),
            config: Some(SplitConfig {
                value: MappingValue::Reference(ReferenceValue {
                    value: "data.items".to_string(),
                    type_hint: None,
                    default: None,
                }),
                variables: Some(variables),
                parallelism: None,
                sequential: None,
                dont_stop_on_failed: None,
                max_retries: None,
                retry_delay: None,
                timeout: None,
            }),
            subgraph: Box::new(create_minimal_graph("finish")),
            input_schema: HashMap::new(),
            output_schema: HashMap::new(),
        };

        let tokens = emit(&split_step, &mut ctx);
        let code = tokens.to_string();

        // Verify variables mapping is included
        assert!(
            code.contains("\"variables\""),
            "Should include variables mapping"
        );
    }

    #[test]
    fn test_emit_split_retry_config() {
        let mut ctx = EmitContext::new(false);
        let split_step = SplitStep {
            id: "split-retry".to_string(),
            name: Some("Retry Split".to_string()),
            config: Some(SplitConfig {
                value: MappingValue::Reference(ReferenceValue {
                    value: "data.items".to_string(),
                    type_hint: None,
                    default: None,
                }),
                variables: None,
                parallelism: None,
                sequential: None,
                dont_stop_on_failed: None,
                max_retries: Some(5),
                retry_delay: Some(2000),
                timeout: None,
            }),
            subgraph: Box::new(create_minimal_graph("finish")),
            input_schema: HashMap::new(),
            output_schema: HashMap::new(),
        };

        let tokens = emit(&split_step, &mut ctx);
        let code = tokens.to_string();

        // Verify retry config in durable macro
        assert!(
            code.contains("max_retries = 5"),
            "Should include custom max_retries"
        );
        assert!(
            code.contains("delay = 2000"),
            "Should include custom retry delay"
        );
    }

    #[test]
    fn test_emit_split_durable_function() {
        let mut ctx = EmitContext::new(false);
        let split_step = create_split_step("split-durable", "data.items");

        let tokens = emit(&split_step, &mut ctx);
        let code = tokens.to_string();

        // Verify durable function is generated
        // Token stream formats attributes as "# [durable" with spaces
        assert!(
            code.contains("# [durable") || code.contains("#[durable"),
            "Should have durable macro"
        );
        assert!(code.contains("async fn"), "Should be async function");
        assert!(
            code.contains("cache_key"),
            "Should have cache_key parameter"
        );
    }

    #[test]
    fn test_emit_split_loop_indices() {
        let mut ctx = EmitContext::new(false);
        let split_step = create_split_step("split-indices", "data.items");

        let tokens = emit(&split_step, &mut ctx);
        let code = tokens.to_string();

        // Verify _loop_indices is tracked for nested loops
        assert!(
            code.contains("_loop_indices"),
            "Should track _loop_indices for cache key uniqueness"
        );
        assert!(
            code.contains("_index"),
            "Should inject _index for backward compatibility"
        );
    }

    #[test]
    fn test_emit_split_subgraph_function() {
        let mut ctx = EmitContext::new(false);
        let split_step = create_split_step("split-subgraph", "data.items");

        let tokens = emit(&split_step, &mut ctx);
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
    fn test_emit_split_error_handling() {
        let mut ctx = EmitContext::new(false);
        let split_step = SplitStep {
            id: "split-errors".to_string(),
            name: Some("Error Handling Split".to_string()),
            config: Some(SplitConfig {
                value: MappingValue::Reference(ReferenceValue {
                    value: "data.items".to_string(),
                    type_hint: None,
                    default: None,
                }),
                variables: None,
                parallelism: None,
                sequential: None,
                dont_stop_on_failed: Some(true),
                max_retries: None,
                retry_delay: None,
                timeout: None,
            }),
            subgraph: Box::new(create_minimal_graph("finish")),
            input_schema: HashMap::new(),
            output_schema: HashMap::new(),
        };

        let tokens = emit(&split_step, &mut ctx);
        let code = tokens.to_string();

        // Verify error handling structure
        assert!(
            code.contains("dont_stop_on_failed"),
            "Should check dont_stop_on_failed flag"
        );
        assert!(code.contains("errors"), "Should track errors array");
    }

    #[test]
    fn test_emit_split_output_structure() {
        let mut ctx = EmitContext::new(false);
        let split_step = create_split_step("split-output", "data.items");

        let tokens = emit(&split_step, &mut ctx);
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
        assert!(code.contains("\"Split\""), "Should have stepType = Split");
        assert!(
            code.contains("\"outputs\""),
            "Should include outputs in result"
        );
    }

    #[test]
    fn test_emit_split_cancellation_check() {
        let mut ctx = EmitContext::new(false);
        let split_step = create_split_step("split-cancel", "data.items");

        let tokens = emit(&split_step, &mut ctx);
        let code = tokens.to_string();

        // Verify cancellation is checked
        assert!(
            code.contains("check_cancelled"),
            "Should check for cancellation after each iteration"
        );
    }

    #[test]
    fn test_emit_split_stores_in_steps_context() {
        let mut ctx = EmitContext::new(false);
        let split_step = create_split_step("split-store", "data.items");

        let tokens = emit(&split_step, &mut ctx);
        let code = tokens.to_string();

        // Verify result is stored in steps_context
        assert!(
            code.contains("steps_context . insert"),
            "Should store result in steps_context"
        );
    }

    #[test]
    fn test_emit_split_debug_mode_enabled() {
        let mut ctx = EmitContext::new(true); // debug mode ON
        let split_step = create_split_step("split-debug", "data.items");

        let tokens = emit(&split_step, &mut ctx);
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
    fn test_emit_split_with_immediate_array() {
        let mut ctx = EmitContext::new(false);
        let split_step = SplitStep {
            id: "split-immediate".to_string(),
            name: Some("Immediate Array Split".to_string()),
            config: Some(SplitConfig {
                value: MappingValue::Immediate(ImmediateValue {
                    value: serde_json::json!([1, 2, 3, 4, 5]),
                }),
                variables: None,
                parallelism: None,
                sequential: None,
                dont_stop_on_failed: None,
                max_retries: None,
                retry_delay: None,
                timeout: None,
            }),
            subgraph: Box::new(create_minimal_graph("finish")),
            input_schema: HashMap::new(),
            output_schema: HashMap::new(),
        };

        let tokens = emit(&split_step, &mut ctx);
        let code = tokens.to_string();

        // Verify immediate value is used
        assert!(
            code.contains("1") && code.contains("2") && code.contains("3"),
            "Should include immediate array values"
        );
    }

    #[test]
    fn test_emit_split_cache_key() {
        let mut ctx = EmitContext::new(false);
        let split_step = create_split_step("my-split-step", "data.items");

        let tokens = emit(&split_step, &mut ctx);
        let code = tokens.to_string();

        // Verify cache key format
        assert!(
            code.contains("split::my-split-step"),
            "Should have cache key with split:: prefix and step ID"
        );
    }

    #[test]
    fn test_emit_split_with_unnamed_step() {
        let mut ctx = EmitContext::new(false);
        let split_step = SplitStep {
            id: "split-unnamed".to_string(),
            name: None, // No name
            config: Some(SplitConfig {
                value: MappingValue::Reference(ReferenceValue {
                    value: "data.items".to_string(),
                    type_hint: None,
                    default: None,
                }),
                variables: None,
                parallelism: None,
                sequential: None,
                dont_stop_on_failed: None,
                max_retries: None,
                retry_delay: None,
                timeout: None,
            }),
            subgraph: Box::new(create_minimal_graph("finish")),
            input_schema: HashMap::new(),
            output_schema: HashMap::new(),
        };

        let tokens = emit(&split_step, &mut ctx);
        let code = tokens.to_string();

        // Should use "Unnamed" as display name
        assert!(
            code.contains("\"Unnamed\""),
            "Should use 'Unnamed' for unnamed steps"
        );
    }
}
