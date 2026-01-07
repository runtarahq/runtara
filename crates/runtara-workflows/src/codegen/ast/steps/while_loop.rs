// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! While step emitter.
//!
//! The While step executes a subgraph repeatedly while a condition is true.
//! Each iteration produces a heartbeat to maintain instance liveness.
//! The loop has a configurable maximum iteration limit (default: 10) to prevent infinite loops.

use proc_macro2::TokenStream;
use quote::quote;

use super::super::context::EmitContext;
use super::super::mapping;
use super::super::program;
use super::conditional::emit_condition_expression;
use super::{emit_step_debug_end, emit_step_debug_start};
use runtara_dsl::WhileStep;

/// Emit code for a While step.
pub fn emit(step: &WhileStep, ctx: &mut EmitContext) -> TokenStream {
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
    let subgraph_code = program::emit_graph_as_function(&subgraph_fn_name, &step.subgraph, ctx);

    // Generate condition evaluation code
    let condition_eval = emit_condition_expression(&step.condition, ctx, &source_var);

    // Serialize condition to JSON for debug events
    let condition_json = serde_json::to_string(&step.condition).ok();

    // Clone scenario inputs var for debug events (to access _loop_indices)
    let scenario_inputs_var = inputs_var.clone();

    // Generate debug event emissions
    let debug_start = emit_step_debug_start(
        ctx,
        step_id,
        step_name,
        "While",
        Some(&loop_inputs_var),
        condition_json.as_deref(),
        Some(&scenario_inputs_var),
    );
    let debug_end = emit_step_debug_end(
        ctx,
        step_id,
        step_name,
        "While",
        Some(&step_var),
        Some(&scenario_inputs_var),
    );

    quote! {
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

                let __subgraph_inputs = ScenarioInputs {
                    data: #inputs_var.data.clone(),
                    variables: Arc::new(serde_json::Value::Object(__loop_vars)),
                };

                // Execute subgraph
                __loop_outputs = #subgraph_fn_name(Arc::new(__subgraph_inputs)).await?;

                __loop_index += 1;

                // Heartbeat after each iteration to maintain liveness
                {
                    let __sdk = sdk().lock().await;
                    let _ = __sdk.heartbeat().await;
                }

                // Check for cancellation after each iteration
                {
                    let mut __sdk = sdk().lock().await;
                    if let Err(e) = __sdk.check_cancelled().await {
                        return Err(format!("While step {} cancelled at iteration {}: {}", #step_id, __loop_index, e));
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
    }
}
