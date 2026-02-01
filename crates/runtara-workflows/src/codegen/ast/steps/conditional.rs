// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Conditional step emitter.
//!
//! The Conditional step evaluates conditions and branches execution.
//! Conditions are defined via the structured `condition` field using ConditionExpression.

use proc_macro2::TokenStream;
use quote::quote;

use super::super::CodegenError;
use super::super::condition_emitters::emit_condition_expression;
use super::super::context::EmitContext;
use super::super::mapping;
use super::super::steps;
use super::branching;
use super::{emit_step_debug_end, emit_step_debug_start, emit_step_span_start};
use runtara_dsl::{ConditionalStep, ExecutionGraph};

/// Emit code for a Conditional step.
pub fn emit(
    step: &ConditionalStep,
    ctx: &mut EmitContext,
    graph: &ExecutionGraph,
) -> Result<TokenStream, CodegenError> {
    let step_id = &step.id;
    let step_name = step.name.as_deref();
    let step_name_display = step_name.unwrap_or("Unnamed");
    let execution_plan = &graph.execution_plan;

    // Do all mutable operations first
    let step_var = ctx.declare_step(step_id);
    let source_var = ctx.temp_var("source");
    let condition_var = ctx.temp_var("condition_result");
    let condition_inputs_var = ctx.temp_var("condition_inputs");

    // Clone immutable references
    let steps_context = ctx.steps_context_var.clone();

    // Build the source for input mapping
    let build_source = mapping::emit_build_source(ctx);

    // Generate condition evaluation from the structured condition
    let condition_eval = emit_condition_expression(&step.condition, ctx, &source_var);

    // Serialize condition to JSON for debug events
    let condition_json = serde_json::to_string(&step.condition).ok();

    // Find the true and false branch starting steps
    let true_step_id = steps::find_next_step_for_label(step_id, "true", execution_plan);
    let false_step_id = steps::find_next_step_for_label(step_id, "false", execution_plan);

    // Find the merge point where both branches converge (if any)
    // This prevents duplicate code generation for diamond patterns
    let merge_point = branching::find_merge_point(
        true_step_id.map(|s| s.to_string()),
        false_step_id.map(|s| s.to_string()),
        graph,
    );

    // Emit code for the true branch (stopping at merge point)
    let true_branch_code = if let Some(start_step_id) = true_step_id {
        branching::emit_branch_code(start_step_id, graph, ctx, merge_point.as_deref())?
    } else {
        quote! {}
    };

    // Emit code for the false branch (stopping at merge point)
    let false_branch_code = if let Some(start_step_id) = false_step_id {
        branching::emit_branch_code(start_step_id, graph, ctx, merge_point.as_deref())?
    } else {
        quote! {}
    };

    // Emit code for the common suffix path after the merge point
    let common_suffix_code = if let Some(ref merge_step_id) = merge_point {
        branching::emit_branch_code(merge_step_id, graph, ctx, None)?
    } else {
        quote! {}
    };

    // Get the scenario inputs variable to access _loop_indices at runtime
    let scenario_inputs_var = ctx.inputs_var.clone();

    // Generate debug event emissions (Conditional doesn't create a scope)
    let debug_start = emit_step_debug_start(
        ctx,
        step_id,
        step_name,
        "Conditional",
        Some(&condition_inputs_var),
        condition_json.as_deref(),
        Some(&scenario_inputs_var),
        None,
    );
    let debug_end = emit_step_debug_end(
        ctx,
        step_id,
        step_name,
        "Conditional",
        Some(&step_var),
        Some(&scenario_inputs_var),
        None,
    );

    // Generate tracing span for OpenTelemetry
    let span_def = emit_step_span_start(step_id, step_name, "Conditional");

    // Note: Conditional does NOT use async block wrapping because:
    // 1. Condition evaluation is synchronous (no await points)
    // 2. Branch steps have their own async instrumentation
    // 3. Branches may contain Finish steps with `return Ok(...)` that must
    //    return from execute_workflow, not from an enclosing async block
    //
    // We use sync span entry (.entered()) which properly propagates to child spans
    // created by branch steps via their .instrument() calls.
    Ok(quote! {
        let #source_var = #build_source;
        let #condition_inputs_var = serde_json::json!({"condition": "evaluating"});

        // Define and enter tracing span for this step (sync pattern for control flow)
        #span_def
        let __step_span_guard = __step_span.entered();

        #debug_start

        let #condition_var: bool = #condition_eval;

        let #step_var = serde_json::json!({
            "stepId": #step_id,
            "stepName": #step_name_display,
            "stepType": "Conditional",
            "outputs": {
                "result": #condition_var
            }
        });

        #debug_end

        #steps_context.insert(#step_id.to_string(), #step_var.clone());

        // Drop the span guard before executing branches
        // Each branch step has its own instrumentation that will create child spans
        drop(__step_span_guard);

        // Execute the appropriate branch
        if #condition_var {
            #true_branch_code
        } else {
            #false_branch_code
        }

        // Execute common suffix path after merge point (if any)
        #common_suffix_code
    })
}
