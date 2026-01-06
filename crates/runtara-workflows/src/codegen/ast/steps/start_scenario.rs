// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! StartScenario step emitter.
//!
//! The StartScenario step executes a nested child scenario.
//! When a child scenario's ExecutionGraph is available in the EmitContext,
//! it will be recursively emitted and embedded into the parent scenario.
//! The entire child scenario result uses #[durable] macro for checkpoint-based recovery.

use proc_macro2::TokenStream;
use quote::quote;

use super::super::context::EmitContext;
use super::super::mapping;
use super::super::program;
use super::{emit_step_debug_end, emit_step_debug_start};
use runtara_dsl::{ExecutionGraph, StartScenarioStep};

/// Emit code for a StartScenario step.
///
/// If the child scenario's ExecutionGraph is available in the EmitContext,
/// it will be recursively emitted as an embedded function. Otherwise,
/// a placeholder/warning is generated.
pub fn emit(step: &StartScenarioStep, ctx: &mut EmitContext) -> TokenStream {
    let step_id = &step.id;
    let step_name = step.name.as_deref();
    let step_name_display = step_name.unwrap_or("Unnamed");
    let child_scenario_id = &step.child_scenario_id;

    // Get retry configuration with defaults
    let max_retries = step.max_retries.unwrap_or(3);
    let retry_delay = step.retry_delay.unwrap_or(1000);

    // Check if we have the child scenario's graph
    if let Some(child_graph) = ctx.get_child_scenario(step_id).cloned() {
        // We have the child graph - emit embedded version
        emit_with_embedded_child(step, &child_graph, ctx, max_retries, retry_delay)
    } else {
        // No child graph available - emit placeholder
        emit_placeholder(
            step_id,
            step_name,
            step_name_display,
            child_scenario_id,
            ctx,
        )
    }
}

/// Emit a StartScenario step with an embedded child scenario.
fn emit_with_embedded_child(
    step: &StartScenarioStep,
    child_graph: &ExecutionGraph,
    ctx: &mut EmitContext,
    max_retries: u32,
    retry_delay: u64,
) -> TokenStream {
    let step_id = &step.id;
    let step_name = step.name.as_deref();
    let step_name_display = step_name.unwrap_or("Unnamed");
    let child_scenario_id = &step.child_scenario_id;

    // Do all mutable operations first
    let step_var = ctx.declare_step(step_id);
    let source_var = ctx.temp_var("source");
    let child_inputs_var = ctx.temp_var("child_inputs");
    let child_fn_name = ctx.temp_var(&format!(
        "execute_child_{}",
        EmitContext::sanitize_ident(step_id)
    ));
    let durable_fn_name =
        ctx.temp_var(&format!("{}_durable", EmitContext::sanitize_ident(step_id)));

    // Clone immutable references
    let steps_context = ctx.steps_context_var.clone();

    // Build the source for input mapping
    let build_source = mapping::emit_build_source(ctx);

    // Serialize input mapping to JSON for debug events
    let input_mapping_json = step.input_mapping.as_ref().and_then(|m| {
        if m.is_empty() {
            None
        } else {
            serde_json::to_string(m).ok()
        }
    });

    // Generate input mapping for child scenario
    let inputs_code = if let Some(ref input_mapping) = step.input_mapping {
        if !input_mapping.is_empty() {
            let mapping_code = mapping::emit_input_mapping(input_mapping, ctx, &source_var);
            quote! { #mapping_code }
        } else {
            quote! { serde_json::Value::Object(serde_json::Map::new()) }
        }
    } else {
        quote! { serde_json::Value::Object(serde_json::Map::new()) }
    };

    // Generate the embedded child scenario function using shared recursive emitter
    let child_fn_code = program::emit_graph_as_function(&child_fn_name, child_graph, ctx);

    // Generate debug event emissions
    let debug_start = emit_step_debug_start(
        ctx,
        step_id,
        step_name,
        "StartScenario",
        Some(&child_inputs_var),
        input_mapping_json.as_deref(),
    );
    let debug_end = emit_step_debug_end(ctx, step_id, step_name, "StartScenario", Some(&step_var));

    // Static base for cache key - will be combined with loop indices at runtime
    let cache_key_base = format!("start_scenario::{}", step_id);

    // Get the scenario inputs variable to access _loop_indices at runtime
    let scenario_inputs_var = ctx.inputs_var.clone();

    let max_retries_lit = max_retries;
    let retry_delay_lit = retry_delay;

    quote! {
        // Define the embedded child scenario function
        #child_fn_code

        let #source_var = #build_source;
        let #child_inputs_var = #inputs_code;

        #debug_start

        // Build cache key dynamically, including loop indices if inside Split/While
        let __durable_cache_key = {
            let base = #cache_key_base;
            let indices_suffix = (*#scenario_inputs_var.variables)
                .as_object()
                .and_then(|vars| vars.get("_loop_indices"))
                .and_then(|v| v.as_array())
                .filter(|arr| !arr.is_empty())
                .map(|arr| {
                    let indices: Vec<String> = arr.iter()
                        .map(|v| v.to_string())
                        .collect();
                    format!("::[{}]", indices.join(","))
                })
                .unwrap_or_default();
            format!("{}{}", base, indices_suffix)
        };

        // Define the durable child scenario execution function
        #[durable(max_retries = #max_retries_lit, delay = #retry_delay_lit)]
        async fn #durable_fn_name(
            cache_key: &str,
            child_inputs: serde_json::Value,
            child_scenario_id: &str,
            step_id: &str,
            step_name: &str,
        ) -> std::result::Result<serde_json::Value, String> {
            // Prepare child scenario inputs
            // All mapped inputs become child's data (myParam1 -> data.myParam1)
            // Child variables are always isolated - never inherited from parent
            let child_scenario_inputs = ScenarioInputs {
                data: Arc::new(child_inputs),
                variables: Arc::new(serde_json::Value::Object(serde_json::Map::new())),
            };

            // Execute child scenario
            let child_result = #child_fn_name(Arc::new(child_scenario_inputs)).await
                .map_err(|e| format!("Child scenario {} failed: {}", child_scenario_id, e))?;

            let result = serde_json::json!({
                "stepId": step_id,
                "stepName": step_name,
                "stepType": "StartScenario",
                "childScenarioId": child_scenario_id,
                "outputs": child_result
            });

            Ok(result)
        }

        // Execute the durable child scenario function
        let #step_var = #durable_fn_name(
            &__durable_cache_key,
            #child_inputs_var.clone(),
            #child_scenario_id,
            #step_id,
            #step_name_display,
        ).await?;

        #debug_end

        #steps_context.insert(#step_id.to_string(), #step_var.clone());

        // Check for cancellation after child scenario completes
        {
            let mut __sdk = sdk().lock().await;
            if let Err(e) = __sdk.check_cancelled().await {
                return Err(format!("StartScenario step {} cancelled: {}", #step_id, e));
            }
        }
    }
}

/// Emit a placeholder for when child scenario is not available.
fn emit_placeholder(
    step_id: &str,
    step_name: Option<&str>,
    step_name_display: &str,
    child_scenario_id: &str,
    ctx: &mut EmitContext,
) -> TokenStream {
    // Do all mutable operations first
    let step_var = ctx.declare_step(step_id);
    let source_var = ctx.temp_var("source");
    let placeholder_inputs_var = ctx.temp_var("placeholder_inputs");

    // Clone immutable references
    let steps_context = ctx.steps_context_var.clone();

    // Build the source for input mapping
    let build_source = mapping::emit_build_source(ctx);

    // Generate debug event emissions
    let debug_start = emit_step_debug_start(
        ctx,
        step_id,
        step_name,
        "StartScenario",
        Some(&placeholder_inputs_var),
        None,
    );
    let debug_end = emit_step_debug_end(ctx, step_id, step_name, "StartScenario", Some(&step_var));

    quote! {
        let #source_var = #build_source;
        let #placeholder_inputs_var = serde_json::json!({"childScenarioId": #child_scenario_id, "placeholder": true});

        #debug_start

        // Placeholder: child scenario not available at compile time
        let child_result = {
            eprintln!("WARNING: Child scenario {} not embedded - returning empty result", #child_scenario_id);
            serde_json::json!({
                "warning": format!("Child scenario {} was not available at compile time", #child_scenario_id)
            })
        };

        let #step_var = serde_json::json!({
            "stepId": #step_id,
            "stepName": #step_name_display,
            "stepType": "StartScenario",
            "childScenarioId": #child_scenario_id,
            "outputs": child_result
        });

        #debug_end

        #steps_context.insert(#step_id.to_string(), #step_var.clone());

        // Check for cancellation after step completes
        {
            let mut __sdk = sdk().lock().await;
            if let Err(e) = __sdk.check_cancelled().await {
                return Err(format!("StartScenario step {} cancelled: {}", #step_id, e));
            }
        }
    }
}
