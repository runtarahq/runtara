// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Step emitters for AST-based code generation.
//!
//! Each step type has its own emitter that generates the TokenStream
//! for executing that step.

pub mod agent;
pub mod conditional;
pub mod connection;
pub mod finish;
pub mod log;
pub mod split;
pub mod start_scenario;
pub mod switch;
pub mod while_loop;

use proc_macro2::TokenStream;
use quote::quote;

use crate::codegen::ast::context::EmitContext;
use runtara_dsl::{ExecutionGraph, ExecutionPlanEdge, Step};

/// Trait for emitting step execution code.
pub trait StepEmitter {
    /// Emit the TokenStream for this step's execution.
    fn emit(&self, ctx: &mut EmitContext, graph: &ExecutionGraph) -> TokenStream;
}

impl StepEmitter for Step {
    fn emit(&self, ctx: &mut EmitContext, graph: &ExecutionGraph) -> TokenStream {
        match self {
            Step::Finish(s) => finish::emit(s, ctx),
            Step::Agent(s) => agent::emit(s, ctx),
            Step::Conditional(s) => conditional::emit(s, ctx, graph),
            Step::Switch(s) => switch::emit(s, ctx),
            Step::Split(s) => split::emit(s, ctx),
            Step::StartScenario(s) => start_scenario::emit(s, ctx),
            Step::While(s) => while_loop::emit(s, ctx),
            Step::Log(s) => log::emit(s, ctx),
            Step::Connection(s) => connection::emit(s, ctx),
        }
    }
}

/// Get the step type string for a Step.
pub fn step_type_str(step: &Step) -> &'static str {
    match step {
        Step::Finish(_) => "Finish",
        Step::Agent(_) => "Agent",
        Step::Conditional(_) => "Conditional",
        Step::Switch(_) => "Switch",
        Step::Split(_) => "Split",
        Step::StartScenario(_) => "StartScenario",
        Step::While(_) => "While",
        Step::Log(_) => "Log",
        Step::Connection(_) => "Connection",
    }
}

/// Get the step ID from a Step.
pub fn step_id(step: &Step) -> &str {
    match step {
        Step::Finish(s) => &s.id,
        Step::Agent(s) => &s.id,
        Step::Conditional(s) => &s.id,
        Step::Switch(s) => &s.id,
        Step::Split(s) => &s.id,
        Step::StartScenario(s) => &s.id,
        Step::While(s) => &s.id,
        Step::Log(s) => &s.id,
        Step::Connection(s) => &s.id,
    }
}

/// Get the step name from a Step.
pub fn step_name(step: &Step) -> Option<&str> {
    match step {
        Step::Finish(s) => s.name.as_deref(),
        Step::Agent(s) => s.name.as_deref(),
        Step::Conditional(s) => s.name.as_deref(),
        Step::Switch(s) => s.name.as_deref(),
        Step::Split(s) => s.name.as_deref(),
        Step::StartScenario(s) => s.name.as_deref(),
        Step::While(s) => s.name.as_deref(),
        Step::Log(s) => s.name.as_deref(),
        Step::Connection(s) => s.name.as_deref(),
    }
}

/// Maximum size in bytes for inputs/outputs in debug events before truncation.
const STEP_DEBUG_MAX_PAYLOAD_SIZE: usize = 10 * 1024; // 10KB

/// Emit debug event for step execution start.
/// Captures step metadata, inputs, and input mapping.
///
/// The generated code builds the payload inline and calls `sdk.custom_event()`.
///
/// # Arguments
/// * `ctx` - Emit context (checks debug_mode)
/// * `step_id` - Unique step identifier
/// * `step_name` - Optional human-readable step name
/// * `step_type` - Step type string (e.g., "Agent", "Conditional")
/// * `inputs_var` - Optional Ident of variable holding step inputs (as serde_json::Value)
/// * `input_mapping_json` - Optional static JSON string of input mapping DSL
pub fn emit_step_debug_start(
    ctx: &EmitContext,
    step_id: &str,
    step_name: Option<&str>,
    step_type: &str,
    inputs_var: Option<&proc_macro2::Ident>,
    input_mapping_json: Option<&str>,
) -> TokenStream {
    if !ctx.debug_mode {
        return quote! {};
    }

    let max_size = STEP_DEBUG_MAX_PAYLOAD_SIZE;

    let name_expr = step_name
        .map(|n| quote! { Some(#n.to_string()) })
        .unwrap_or(quote! { None::<String> });

    let inputs_expr = inputs_var
        .map(|v| {
            quote! {
                Some(__truncate_json_value(&#v, #max_size))
            }
        })
        .unwrap_or(quote! { None::<serde_json::Value> });

    let mapping_expr = input_mapping_json
        .map(|json| quote! {
            Some(serde_json::from_str::<serde_json::Value>(#json).unwrap_or(serde_json::Value::Null))
        })
        .unwrap_or(quote! { None::<serde_json::Value> });

    quote! {
        let __step_start_time = std::time::Instant::now();
        {
            // Truncate helper function
            fn __truncate_json_value(value: &serde_json::Value, max_size: usize) -> serde_json::Value {
                let serialized = serde_json::to_string(value).unwrap_or_default();
                if serialized.len() <= max_size {
                    value.clone()
                } else {
                    let truncated = &serialized[..max_size.saturating_sub(20)];
                    serde_json::json!({
                        "_truncated": true,
                        "_original_size": serialized.len(),
                        "_preview": truncated
                    })
                }
            }

            let __payload = serde_json::json!({
                "step_id": #step_id,
                "step_name": #name_expr,
                "step_type": #step_type,
                "timestamp_ms": std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as i64)
                    .unwrap_or(0),
                "inputs": #inputs_expr,
                "input_mapping": #mapping_expr,
            });

            let __payload_bytes = serde_json::to_vec(&__payload).unwrap_or_default();
            let __sdk_guard = sdk().lock().await;
            let _ = __sdk_guard.custom_event("step_debug_start", __payload_bytes).await;
        }
    }
}

/// Emit debug event for step execution end.
/// Captures step metadata, outputs, and duration.
///
/// The generated code builds the payload inline and calls `sdk.custom_event()`.
///
/// # Arguments
/// * `ctx` - Emit context (checks debug_mode)
/// * `step_id` - Unique step identifier
/// * `step_name` - Optional human-readable step name
/// * `step_type` - Step type string (e.g., "Agent", "Conditional")
/// * `outputs_var` - Optional Ident of variable holding step outputs (as serde_json::Value)
pub fn emit_step_debug_end(
    ctx: &EmitContext,
    step_id: &str,
    step_name: Option<&str>,
    step_type: &str,
    outputs_var: Option<&proc_macro2::Ident>,
) -> TokenStream {
    if !ctx.debug_mode {
        return quote! {};
    }

    let max_size = STEP_DEBUG_MAX_PAYLOAD_SIZE;

    let name_expr = step_name
        .map(|n| quote! { Some(#n.to_string()) })
        .unwrap_or(quote! { None::<String> });

    let outputs_expr = outputs_var
        .map(|v| {
            quote! {
                Some(__truncate_json_value(&#v, #max_size))
            }
        })
        .unwrap_or(quote! { None::<serde_json::Value> });

    quote! {
        {
            let __duration_ms = __step_start_time.elapsed().as_millis() as u64;

            // Truncate helper function
            fn __truncate_json_value(value: &serde_json::Value, max_size: usize) -> serde_json::Value {
                let serialized = serde_json::to_string(value).unwrap_or_default();
                if serialized.len() <= max_size {
                    value.clone()
                } else {
                    let truncated = &serialized[..max_size.saturating_sub(20)];
                    serde_json::json!({
                        "_truncated": true,
                        "_original_size": serialized.len(),
                        "_preview": truncated
                    })
                }
            }

            let __payload = serde_json::json!({
                "step_id": #step_id,
                "step_name": #name_expr,
                "step_type": #step_type,
                "timestamp_ms": std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as i64)
                    .unwrap_or(0),
                "duration_ms": __duration_ms,
                "outputs": #outputs_expr,
            });

            let __payload_bytes = serde_json::to_vec(&__payload).unwrap_or_default();
            let __sdk_guard = sdk().lock().await;
            let _ = __sdk_guard.custom_event("step_debug_end", __payload_bytes).await;
        }
    }
}

/// Emit code to set the current step (for error reporting).
pub fn emit_set_current_step(step_id: &str) -> TokenStream {
    quote! {
        ctx.set_current_step(#step_id);
    }
}

/// Build execution order using BFS traversal from entry point.
/// Stops at Conditional steps (branches handled separately).
pub fn build_execution_order(graph: &ExecutionGraph) -> Vec<String> {
    use std::collections::{HashSet, VecDeque};

    let mut order = Vec::new();
    let mut visited = HashSet::new();
    let mut queue = VecDeque::new();

    queue.push_back(graph.entry_point.clone());

    while let Some(step_id) = queue.pop_front() {
        if visited.contains(&step_id) {
            continue;
        }
        visited.insert(step_id.clone());
        order.push(step_id.clone());

        // Get the step to check its type
        let step = match graph.steps.get(&step_id) {
            Some(s) => s,
            None => continue,
        };

        // Stop BFS at Conditional steps - branches handled by the step itself
        if matches!(step, Step::Conditional(_)) {
            continue;
        }

        // Find next steps from execution plan
        for edge in &graph.execution_plan {
            if edge.from_step == step_id {
                // Only follow "source" edges, skip "true"/"false" labels
                let label = edge.label.as_deref().unwrap_or("");
                if label != "true" && label != "false" {
                    if !visited.contains(&edge.to_step) {
                        queue.push_back(edge.to_step.clone());
                    }
                }
            }
        }
    }

    order
}

/// Find the next step for a given label (e.g., "true", "false", or default).
pub fn find_next_step_for_label<'a>(
    step_id: &str,
    label: &str,
    execution_plan: &'a [ExecutionPlanEdge],
) -> Option<&'a str> {
    for edge in execution_plan {
        if edge.from_step == step_id {
            let edge_label = edge.label.as_deref().unwrap_or("");
            if edge_label == label {
                return Some(&edge.to_step);
            }
        }
    }
    None
}

/// Find the onError handler step for a given step.
pub fn find_on_error_step<'a>(
    step_id: &str,
    execution_plan: &'a [ExecutionPlanEdge],
) -> Option<&'a str> {
    find_next_step_for_label(step_id, "onError", execution_plan)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_debug_ctx() -> EmitContext {
        EmitContext::new(true) // debug_mode = true
    }

    fn make_non_debug_ctx() -> EmitContext {
        EmitContext::new(false) // debug_mode = false
    }

    #[test]
    fn test_emit_step_debug_start_disabled_when_not_debug_mode() {
        let ctx = make_non_debug_ctx();
        let tokens = emit_step_debug_start(&ctx, "step-1", Some("Test Step"), "Agent", None, None);
        assert!(
            tokens.is_empty(),
            "Should emit nothing when debug_mode is false"
        );
    }

    #[test]
    fn test_emit_step_debug_start_emits_code_in_debug_mode() {
        let ctx = make_debug_ctx();
        let tokens = emit_step_debug_start(&ctx, "step-1", Some("Test Step"), "Agent", None, None);
        let code = tokens.to_string();

        // Verify key elements are present in generated code
        assert!(
            code.contains("__step_start_time"),
            "Should declare start time"
        );
        assert!(
            code.contains("step_debug_start"),
            "Should use correct subtype"
        );
        assert!(code.contains("custom_event"), "Should call custom_event");
        assert!(code.contains("step-1"), "Should include step_id");
        assert!(code.contains("Test Step"), "Should include step_name");
        assert!(code.contains("Agent"), "Should include step_type");
    }

    #[test]
    fn test_emit_step_debug_start_with_inputs_var() {
        let ctx = make_debug_ctx();
        let inputs_var = proc_macro2::Ident::new("my_inputs", proc_macro2::Span::call_site());
        let tokens =
            emit_step_debug_start(&ctx, "step-2", None, "Conditional", Some(&inputs_var), None);
        let code = tokens.to_string();

        assert!(
            code.contains("my_inputs"),
            "Should reference the inputs variable"
        );
        assert!(
            code.contains("__truncate_json_value"),
            "Should include truncation helper"
        );
    }

    #[test]
    fn test_emit_step_debug_start_with_input_mapping() {
        let ctx = make_debug_ctx();
        let mapping_json = r#"{"field": {"type": "reference", "value": "data.x"}}"#;
        let tokens = emit_step_debug_start(
            &ctx,
            "step-3",
            Some("Map Step"),
            "Agent",
            None,
            Some(mapping_json),
        );
        let code = tokens.to_string();

        assert!(
            code.contains("input_mapping"),
            "Should include input_mapping in payload"
        );
        assert!(
            code.contains("serde_json :: from_str"),
            "Should parse mapping JSON"
        );
    }

    #[test]
    fn test_emit_step_debug_end_disabled_when_not_debug_mode() {
        let ctx = make_non_debug_ctx();
        let tokens = emit_step_debug_end(&ctx, "step-1", Some("Test Step"), "Agent", None);
        assert!(
            tokens.is_empty(),
            "Should emit nothing when debug_mode is false"
        );
    }

    #[test]
    fn test_emit_step_debug_end_emits_code_in_debug_mode() {
        let ctx = make_debug_ctx();
        let tokens = emit_step_debug_end(&ctx, "step-1", Some("Test Step"), "Agent", None);
        let code = tokens.to_string();

        // Verify key elements are present in generated code
        assert!(code.contains("__duration_ms"), "Should calculate duration");
        assert!(
            code.contains("step_debug_end"),
            "Should use correct subtype"
        );
        assert!(code.contains("custom_event"), "Should call custom_event");
        assert!(code.contains("step-1"), "Should include step_id");
        assert!(
            code.contains("duration_ms"),
            "Should include duration in payload"
        );
    }

    #[test]
    fn test_emit_step_debug_end_with_outputs_var() {
        let ctx = make_debug_ctx();
        let outputs_var = proc_macro2::Ident::new("step_result", proc_macro2::Span::call_site());
        let tokens = emit_step_debug_end(&ctx, "step-4", None, "Split", Some(&outputs_var));
        let code = tokens.to_string();

        assert!(
            code.contains("step_result"),
            "Should reference the outputs variable"
        );
        assert!(
            code.contains("__truncate_json_value"),
            "Should include truncation helper"
        );
    }

    #[test]
    fn test_emit_step_debug_start_includes_timestamp() {
        let ctx = make_debug_ctx();
        let tokens = emit_step_debug_start(&ctx, "step-5", None, "Finish", None, None);
        let code = tokens.to_string();

        assert!(code.contains("timestamp_ms"), "Should include timestamp");
        assert!(
            code.contains("SystemTime :: now"),
            "Should use current time"
        );
    }

    #[test]
    fn test_emit_step_debug_end_includes_timestamp_and_duration() {
        let ctx = make_debug_ctx();
        let tokens = emit_step_debug_end(&ctx, "step-6", None, "Agent", None);
        let code = tokens.to_string();

        assert!(code.contains("timestamp_ms"), "Should include timestamp");
        assert!(code.contains("duration_ms"), "Should include duration");
        assert!(
            code.contains("__step_start_time . elapsed"),
            "Should calculate elapsed time"
        );
    }

    #[test]
    fn test_truncation_constant() {
        assert_eq!(
            STEP_DEBUG_MAX_PAYLOAD_SIZE,
            10 * 1024,
            "Max payload size should be 10KB"
        );
    }

    #[test]
    fn test_emit_step_debug_start_without_name() {
        let ctx = make_debug_ctx();
        let tokens = emit_step_debug_start(&ctx, "nameless-step", None, "Agent", None, None);
        let code = tokens.to_string();

        // Should still work, with None for step_name
        assert!(code.contains("nameless-step"), "Should include step_id");
        assert!(code.contains("step_name"), "Should have step_name field");
    }

    #[test]
    fn test_emit_step_debug_end_without_outputs() {
        let ctx = make_debug_ctx();
        let tokens = emit_step_debug_end(&ctx, "step-no-output", Some("No Output"), "Finish", None);
        let code = tokens.to_string();

        // Should still work, with None for outputs
        assert!(code.contains("step-no-output"), "Should include step_id");
        assert!(code.contains("outputs"), "Should have outputs field");
    }

    #[test]
    fn test_emit_step_debug_generates_truncation_function() {
        let ctx = make_debug_ctx();
        let inputs_var = proc_macro2::Ident::new("big_data", proc_macro2::Span::call_site());
        let tokens = emit_step_debug_start(&ctx, "step", None, "Agent", Some(&inputs_var), None);
        let code = tokens.to_string();

        // Verify truncation function is generated
        assert!(
            code.contains("fn __truncate_json_value"),
            "Should define truncation function"
        );
        assert!(code.contains("_truncated"), "Should mark truncated values");
        assert!(
            code.contains("_original_size"),
            "Should include original size"
        );
    }
}
