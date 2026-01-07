// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Finish step emitter.
//!
//! The Finish step defines the scenario outputs and returns from the workflow.

use proc_macro2::TokenStream;
use quote::quote;

use super::super::context::EmitContext;
use super::super::mapping;
use super::{emit_step_debug_end, emit_step_debug_start};
use runtara_dsl::FinishStep;

/// Emit code for a Finish step.
///
/// The Finish step computes its outputs and immediately returns from the
/// workflow function. This is necessary to support multiple Finish steps
/// in different branches (e.g., after a Conditional step).
pub fn emit(step: &FinishStep, ctx: &mut EmitContext) -> TokenStream {
    let step_id = &step.id;
    let step_name = step.name.as_deref();
    let step_name_display = step_name.unwrap_or("Finish");

    // Do all mutable operations first
    let step_var = ctx.declare_step(step_id);
    let source_var = ctx.temp_var("source");
    let outputs_var = ctx.temp_var("finish_outputs");
    let finish_inputs_var = ctx.temp_var("finish_inputs");

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

    // Generate output mapping if present
    let outputs = if let Some(ref input_mapping) = step.input_mapping {
        if !input_mapping.is_empty() {
            let mapping_code = mapping::emit_input_mapping(input_mapping, ctx, &source_var);
            quote! { #mapping_code }
        } else {
            quote! { serde_json::Value::Object(serde_json::Map::new()) }
        }
    } else {
        quote! { serde_json::Value::Object(serde_json::Map::new()) }
    };

    // Get the scenario inputs variable to access _loop_indices at runtime
    let scenario_inputs_var = ctx.inputs_var.clone();

    // Generate debug event emissions
    let debug_start = emit_step_debug_start(
        ctx,
        step_id,
        step_name,
        "Finish",
        Some(&finish_inputs_var),
        input_mapping_json.as_deref(),
        Some(&scenario_inputs_var),
    );
    let debug_end = emit_step_debug_end(
        ctx,
        step_id,
        step_name,
        "Finish",
        Some(&step_var),
        Some(&scenario_inputs_var),
    );

    // The Finish step immediately returns from the workflow function.
    // This allows multiple Finish steps in different branches to work correctly.
    quote! {
        let #source_var = #build_source;
        let #finish_inputs_var = serde_json::json!({"finishing": true});

        #debug_start

        let #outputs_var = #outputs;

        // Extract just the "outputs" field if it exists, otherwise use the whole value
        let #outputs_var = #outputs_var.get("outputs").cloned().unwrap_or(#outputs_var);

        let #step_var = serde_json::json!({
            "stepId": #step_id,
            "stepName": #step_name_display,
            "stepType": "Finish",
            "outputs": &#outputs_var
        });

        #debug_end

        #steps_context.insert(#step_id.to_string(), #step_var.clone());

        // Return immediately with the outputs
        return Ok(#outputs_var);
    }
}
