// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Split step emitter.
//!
//! The Split step iterates over an array, executing a subgraph for each item.
//! The Split step checkpoints its final result once all iterations complete.
//! Individual steps within the subgraph checkpoint themselves via runtara-sdk,
//! enabling recovery mid-iteration.

use proc_macro2::TokenStream;
use quote::quote;

use super::super::context::EmitContext;
use super::super::mapping;
use super::super::program;
use runtara_dsl::{MappingValue, SplitStep};

/// Emit code for a Split step.
pub fn emit(step: &SplitStep, ctx: &mut EmitContext) -> TokenStream {
    let step_id = &step.id;
    let step_name = step.name.as_deref().unwrap_or("Unnamed");
    let debug_mode = ctx.debug_mode;

    // Do all mutable operations first
    let step_var = ctx.declare_step(step_id);
    let source_var = ctx.temp_var("source");
    let split_inputs_var = ctx.temp_var("split_inputs");
    let subgraph_fn_name = ctx.temp_var(&format!(
        "{}_subgraph",
        EmitContext::sanitize_ident(step_id)
    ));

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

    let debug_log = if debug_mode {
        quote! {
            eprintln!("  -> Processing {} items", split_array.len());
        }
    } else {
        quote! {}
    };

    // Cache key for the split step's final result checkpoint
    let cache_key = format!("split::{}", step_id);

    quote! {
        let #source_var = #build_source;
        let #split_inputs_var = #inputs_code;

        // Extract split configuration
        let split_array = #split_inputs_var.get("value")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_else(|| {
                eprintln!("ERROR: Split step 'value' must be an array. Got: {}",
                    #split_inputs_var.get("value").unwrap_or(&serde_json::Value::Null));
                vec![]
            });

        #debug_log

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

        // Try to load completed split result from checkpoint via SDK
        let #step_var: serde_json::Value = {
            let __sdk = sdk().lock().await;

            match __sdk.get_checkpoint(#cache_key).await {
                Ok(Some(cached_bytes)) => {
                    // Found cached result - deserialize and return
                    drop(__sdk);
                    match serde_json::from_slice::<serde_json::Value>(&cached_bytes) {
                        Ok(cached_value) => cached_value,
                        Err(e) => {
                            return Err(format!("Split step {} failed to deserialize cached result: {}", #step_id, e));
                        }
                    }
                }
                Ok(None) | Err(_) => {
                    drop(__sdk);

                    // Execute subgraph for each item
                    // Inner steps will checkpoint themselves - on recovery they replay from their checkpoints
                    let mut results: Vec<serde_json::Value> = Vec::with_capacity(split_array.len());
                    let mut errors: Vec<serde_json::Value> = Vec::new();
                    let variables_base = (*#inputs_var.variables).clone();

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
                                return Err(format!("Split step {} cancelled at iteration {}: {}", #step_id, idx, e));
                            }
                        }
                    }

                    let step_result = if dont_stop_on_failed {
                        serde_json::json!({
                            "stepId": #step_id,
                            "stepName": #step_name,
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
                            "stepId": #step_id,
                            "stepName": #step_name,
                            "stepType": "Split",
                            "outputs": &results
                        })
                    };

                    // Save checkpoint for the complete split result
                    let result_bytes = serde_json::to_vec(&step_result)
                        .map_err(|e| format!("Split step {} failed to serialize result: {}", #step_id, e))?;

                    let __sdk = sdk().lock().await;
                    if let Err(e) = __sdk.checkpoint(#cache_key, &result_bytes).await {
                        eprintln!("WARN: Split step {} checkpoint save failed: {}", #step_id, e);
                    }

                    step_result
                }
            }
        };

        #steps_context.insert(#step_id.to_string(), #step_var.clone());
    }
}
