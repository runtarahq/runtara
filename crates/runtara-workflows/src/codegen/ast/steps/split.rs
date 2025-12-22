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

    // Generate debug event emissions
    let debug_start = emit_step_debug_start(
        ctx,
        step_id,
        step_name,
        "Split",
        Some(&split_inputs_var),
        config_json.as_deref(),
    );
    let debug_end = emit_step_debug_end(ctx, step_id, step_name, "Split", Some(&step_var));

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
                // Inject iteration index as _index (0-based)
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
