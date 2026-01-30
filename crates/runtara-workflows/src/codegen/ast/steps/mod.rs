// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Step emitters for AST-based code generation.
//!
//! Each step type has its own emitter that generates the TokenStream
//! for executing that step.

pub mod agent;
pub mod branching;
pub mod conditional;
pub mod connection;
pub mod error;
pub mod filter;
pub mod finish;
pub mod group_by;
pub mod log;
pub mod split;
pub mod start_scenario;
pub mod switch;
pub mod while_loop;

use proc_macro2::TokenStream;
use quote::quote;

use crate::codegen::ast::CodegenError;
use crate::codegen::ast::context::EmitContext;
use runtara_dsl::{ExecutionGraph, ExecutionPlanEdge, Step};

/// Trait for emitting step execution code.
pub trait StepEmitter {
    /// Emit the TokenStream for this step's execution.
    ///
    /// # Errors
    ///
    /// Returns `CodegenError` if code generation fails (e.g., missing child scenario).
    fn emit(
        &self,
        ctx: &mut EmitContext,
        graph: &ExecutionGraph,
    ) -> Result<TokenStream, CodegenError>;
}

impl StepEmitter for Step {
    fn emit(
        &self,
        ctx: &mut EmitContext,
        graph: &ExecutionGraph,
    ) -> Result<TokenStream, CodegenError> {
        match self {
            Step::Finish(s) => finish::emit(s, ctx),
            Step::Agent(s) => agent::emit(s, ctx),
            Step::Conditional(s) => conditional::emit(s, ctx, graph),
            Step::Switch(s) => switch::emit(s, ctx, graph),
            Step::Split(s) => split::emit(s, ctx),
            Step::StartScenario(s) => start_scenario::emit(s, ctx),
            Step::While(s) => while_loop::emit(s, ctx),
            Step::Log(s) => log::emit(s, ctx),
            Step::Connection(s) => connection::emit(s, ctx),
            Step::Error(s) => error::emit(s, ctx),
            Step::Filter(s) => filter::emit(s, ctx),
            Step::GroupBy(s) => group_by::emit(s, ctx),
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
        Step::Error(_) => "Error",
        Step::Filter(_) => "Filter",
        Step::GroupBy(_) => "GroupBy",
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
        Step::Error(s) => &s.id,
        Step::Filter(s) => &s.id,
        Step::GroupBy(s) => &s.id,
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
        Step::Error(s) => s.name.as_deref(),
        Step::Filter(s) => s.name.as_deref(),
        Step::GroupBy(s) => s.name.as_deref(),
    }
}

/// Maximum size in bytes for inputs/outputs in debug events before truncation.
const STEP_DEBUG_MAX_PAYLOAD_SIZE: usize = 10 * 1024; // 10KB

// ==========================================
// Scope tracking for hierarchy support
// ==========================================

/// Emit code to generate a deterministic scope ID at runtime.
///
/// The scope ID is constructed from step_id and the current loop indices,
/// producing a unique identifier for each scope instance.
///
/// Format: "sc_{step_id}[_{index}]*"
/// Examples:
///   - "sc_split-orders_0" (first Split iteration)
///   - "sc_split-orders_0_while-retry_2" (nested While iteration)
///
/// # Arguments
/// * `step_id` - The step ID creating this scope
/// * `scenario_inputs_var` - Variable holding ScenarioInputs (for extracting _loop_indices)
/// * `iteration_index_var` - Optional variable holding the current iteration index
pub fn emit_generate_scope_id(
    step_id: &str,
    scenario_inputs_var: &proc_macro2::Ident,
    iteration_index_var: Option<&str>,
) -> TokenStream {
    let index_part = if let Some(idx_var) = iteration_index_var {
        let idx_ident = proc_macro2::Ident::new(idx_var, proc_macro2::Span::call_site());
        quote! {
            format!("_{}", #idx_ident)
        }
    } else {
        quote! { String::new() }
    };

    quote! {
        {
            let parent_scope = (*#scenario_inputs_var.variables)
                .as_object()
                .and_then(|vars| vars.get("_scope_id"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());

            let base_scope_id = format!("sc_{}", #step_id);
            let index_suffix = #index_part;

            if let Some(parent) = parent_scope {
                format!("{}_{}{}", parent, #step_id, index_suffix)
            } else {
                format!("{}{}", base_scope_id, index_suffix)
            }
        }
    }
}

/// Emit scope_enter event for entering a hierarchical step (Split, While, StartScenario).
///
/// The generated code builds a scope_enter event payload and sends it via `sdk.custom_event()`.
///
/// # Arguments
/// * `ctx` - Emit context (checks debug_mode)
/// * `step_id` - Unique step identifier
/// * `step_name` - Optional human-readable step name
/// * `step_type` - Step type string ("Split", "While", "StartScenario")
/// * `scope_id_var` - Variable name holding the generated scope_id
/// * `scenario_inputs_var` - Variable holding ScenarioInputs (for extracting parent_scope_id)
/// * `iteration_index_var` - Optional variable name holding iteration index (omit for StartScenario)
pub fn emit_scope_enter_event(
    ctx: &EmitContext,
    step_id: &str,
    step_name: Option<&str>,
    step_type: &str,
    scope_id_var: &str,
    scenario_inputs_var: &proc_macro2::Ident,
    iteration_index_var: Option<&str>,
) -> TokenStream {
    if !ctx.debug_mode {
        return quote! {};
    }

    let scope_id_ident = proc_macro2::Ident::new(scope_id_var, proc_macro2::Span::call_site());

    let name_expr = step_name
        .map(|n| quote! { Some(#n.to_string()) })
        .unwrap_or(quote! { None::<String> });

    let index_expr = if let Some(idx_var) = iteration_index_var {
        let idx_ident = proc_macro2::Ident::new(idx_var, proc_macro2::Span::call_site());
        quote! { Some(#idx_ident as u32) }
    } else {
        quote! { None::<u32> }
    };

    quote! {
        {
            let __parent_scope_id = (*#scenario_inputs_var.variables)
                .as_object()
                .and_then(|vars| vars.get("_scope_id"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());

            let __payload = serde_json::json!({
                "scope_id": #scope_id_ident,
                "parent_scope_id": __parent_scope_id,
                "step_id": #step_id,
                "step_name": #name_expr,
                "step_type": #step_type,
                "index": #index_expr,
            });

            let __payload_bytes = serde_json::to_vec(&__payload).unwrap_or_default();
            let __sdk_guard = sdk().lock().await;
            let _ = __sdk_guard.custom_event("scope_enter", __payload_bytes).await;
        }
    }
}

/// Emit scope_exit event for exiting a hierarchical step scope.
///
/// # Arguments
/// * `ctx` - Emit context (checks debug_mode)
/// * `scope_id_var` - Variable name holding the scope_id
pub fn emit_scope_exit_event(ctx: &EmitContext, scope_id_var: &str) -> TokenStream {
    if !ctx.debug_mode {
        return quote! {};
    }

    let scope_id_ident = proc_macro2::Ident::new(scope_id_var, proc_macro2::Span::call_site());

    quote! {
        {
            let __payload = serde_json::json!({
                "scope_id": #scope_id_ident,
            });

            let __payload_bytes = serde_json::to_vec(&__payload).unwrap_or_default();
            let __sdk_guard = sdk().lock().await;
            let _ = __sdk_guard.custom_event("scope_exit", __payload_bytes).await;
        }
    }
}

/// Emit debug event for step execution start.
/// Captures step metadata, inputs, input mapping, loop indices, and scope_id for hierarchy tracking.
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
/// * `scenario_inputs_var` - Optional Ident of scenario inputs variable (for extracting _loop_indices and _scope_id)
/// * `override_scope_id` - Optional scope_id override for scope-creating steps (Split, While, StartScenario)
#[allow(clippy::too_many_arguments)]
pub fn emit_step_debug_start(
    ctx: &EmitContext,
    step_id: &str,
    step_name: Option<&str>,
    step_type: &str,
    inputs_var: Option<&proc_macro2::Ident>,
    input_mapping_json: Option<&str>,
    scenario_inputs_var: Option<&proc_macro2::Ident>,
    override_scope_id: Option<&str>,
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

    // Extract loop_indices from scenario inputs if available
    let loop_indices_expr = scenario_inputs_var
        .map(|v| {
            quote! {
                (*#v.variables)
                    .as_object()
                    .and_then(|vars| vars.get("_loop_indices"))
                    .cloned()
                    .unwrap_or(serde_json::Value::Array(vec![]))
            }
        })
        .unwrap_or(quote! { serde_json::Value::Array(vec![]) });

    // Use override_scope_id if provided (for scope-creating steps), otherwise extract from variables
    let scope_id_expr = if let Some(scope_id) = override_scope_id {
        quote! { Some(#scope_id.to_string()) }
    } else {
        scenario_inputs_var
            .map(|v| {
                quote! {
                    (*#v.variables)
                        .as_object()
                        .and_then(|vars| vars.get("_scope_id"))
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())
                }
            })
            .unwrap_or(quote! { None::<String> })
    };

    // Extract parent_scope_id from ScenarioInputs struct field
    let parent_scope_id_expr = scenario_inputs_var
        .map(|v| {
            quote! {
                #v.parent_scope_id.clone()
            }
        })
        .unwrap_or(quote! { None::<String> });

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

            let __loop_indices = #loop_indices_expr;
            let __scope_id: Option<String> = #scope_id_expr;
            let __parent_scope_id: Option<String> = #parent_scope_id_expr;

            let __payload = serde_json::json!({
                "step_id": #step_id,
                "step_name": #name_expr,
                "step_type": #step_type,
                "scope_id": __scope_id,
                "parent_scope_id": __parent_scope_id,
                "loop_indices": __loop_indices,
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
/// Captures step metadata, outputs, duration, loop indices, and scope_id for hierarchy tracking.
///
/// The generated code builds the payload inline and calls `sdk.custom_event()`.
///
/// # Arguments
/// * `ctx` - Emit context (checks debug_mode)
/// * `step_id` - Unique step identifier
/// * `step_name` - Optional human-readable step name
/// * `step_type` - Step type string (e.g., "Agent", "Conditional")
/// * `outputs_var` - Optional Ident of variable holding step outputs (as serde_json::Value)
/// * `scenario_inputs_var` - Optional Ident of scenario inputs variable (for extracting _loop_indices and _scope_id)
/// * `override_scope_id` - Optional scope_id override for scope-creating steps (Split, While, StartScenario)
pub fn emit_step_debug_end(
    ctx: &EmitContext,
    step_id: &str,
    step_name: Option<&str>,
    step_type: &str,
    outputs_var: Option<&proc_macro2::Ident>,
    scenario_inputs_var: Option<&proc_macro2::Ident>,
    override_scope_id: Option<&str>,
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

    // Extract loop_indices from scenario inputs if available
    let loop_indices_expr = scenario_inputs_var
        .map(|v| {
            quote! {
                (*#v.variables)
                    .as_object()
                    .and_then(|vars| vars.get("_loop_indices"))
                    .cloned()
                    .unwrap_or(serde_json::Value::Array(vec![]))
            }
        })
        .unwrap_or(quote! { serde_json::Value::Array(vec![]) });

    // Use override_scope_id if provided (for scope-creating steps), otherwise extract from variables
    let scope_id_expr = if let Some(scope_id) = override_scope_id {
        quote! { Some(#scope_id.to_string()) }
    } else {
        scenario_inputs_var
            .map(|v| {
                quote! {
                    (*#v.variables)
                        .as_object()
                        .and_then(|vars| vars.get("_scope_id"))
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())
                }
            })
            .unwrap_or(quote! { None::<String> })
    };

    // Extract parent_scope_id from ScenarioInputs struct field
    let parent_scope_id_expr = scenario_inputs_var
        .map(|v| {
            quote! {
                #v.parent_scope_id.clone()
            }
        })
        .unwrap_or(quote! { None::<String> });

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

            let __loop_indices = #loop_indices_expr;
            let __scope_id: Option<String> = #scope_id_expr;
            let __parent_scope_id: Option<String> = #parent_scope_id_expr;

            let __payload = serde_json::json!({
                "step_id": #step_id,
                "step_name": #name_expr,
                "step_type": #step_type,
                "scope_id": __scope_id,
                "parent_scope_id": __parent_scope_id,
                "loop_indices": __loop_indices,
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

// ==========================================
// Tracing span helpers for OpenTelemetry
// ==========================================

/// Emit code to create and enter a step span.
///
/// Creates a tracing span with step metadata. The span is entered synchronously
/// and must be exited by dropping the guard.
///
/// # Arguments
/// * `step_id` - Unique step identifier
/// * `step_name` - Optional human-readable step name
/// * `step_type` - Step type string (e.g., "Agent", "Conditional")
///
/// # Generated Code Pattern
/// ```rust,ignore
/// let __step_span = tracing::info_span!(
///     "step.agent",
///     step.id = "step-1",
///     step.name = "My Step",
///     step.type = "Agent",
///     otel.kind = "INTERNAL"
/// );
/// let __step_span_guard = __step_span.enter();
/// ```
pub fn emit_step_span_start(
    step_id: &str,
    step_name: Option<&str>,
    step_type: &str,
) -> TokenStream {
    let span_name = format!("step.{}", step_type.to_lowercase());
    let name_display = step_name.unwrap_or(step_id);

    quote! {
        let __step_span = tracing::info_span!(
            #span_name,
            step.id = #step_id,
            step.name = #name_display,
            step.type = #step_type,
            otel.kind = "INTERNAL"
        );
        let __step_span_guard = __step_span.enter();
    }
}

/// Emit code for agent step span with additional agent attributes.
///
/// Similar to `emit_step_span_start` but includes agent.id and capability.id.
pub fn emit_agent_span_start(
    step_id: &str,
    step_name: Option<&str>,
    agent_id: &str,
    capability_id: &str,
) -> TokenStream {
    let name_display = step_name.unwrap_or(step_id);

    quote! {
        let __step_span = tracing::info_span!(
            "step.agent",
            step.id = #step_id,
            step.name = #name_display,
            step.type = "Agent",
            agent.id = #agent_id,
            capability.id = #capability_id,
            otel.kind = "INTERNAL"
        );
        let __step_span_guard = __step_span.enter();
    }
}

/// Emit code to exit a step span.
///
/// Drops the span guard to end the span.
pub fn emit_step_span_end() -> TokenStream {
    quote! {
        drop(__step_span_guard);
    }
}

/// Emit code for iteration span (Split/While steps).
///
/// Creates a child span for each loop iteration with the iteration index.
///
/// # Arguments
/// * `step_id` - Parent step identifier
/// * `step_type` - Step type ("split" or "while")
/// * `index_var` - Variable holding the current iteration index
pub fn emit_iteration_span_start(
    step_id: &str,
    step_type: &str,
    index_var: &proc_macro2::Ident,
) -> TokenStream {
    let span_name = format!("{}.iteration", step_type.to_lowercase());

    quote! {
        let __iter_span = tracing::info_span!(
            #span_name,
            step.id = #step_id,
            iteration.index = #index_var,
            otel.kind = "INTERNAL"
        );
        let __iter_span_guard = __iter_span.enter();
    }
}

/// Emit code to exit an iteration span.
pub fn emit_iteration_span_end() -> TokenStream {
    quote! {
        drop(__iter_span_guard);
    }
}

/// Emit code for child scenario span (StartScenario step).
///
/// Creates a span for the child scenario execution.
///
/// # Arguments
/// * `parent_step_id` - The StartScenario step ID
/// * `child_scenario_id` - The child scenario ID
pub fn emit_child_scenario_span_start(
    parent_step_id: &str,
    child_scenario_id: &str,
) -> TokenStream {
    quote! {
        let __child_span = tracing::info_span!(
            "scenario.child",
            scenario.id = #child_scenario_id,
            parent_step.id = #parent_step_id,
            otel.kind = "INTERNAL"
        );
        let __child_span_guard = __child_span.enter();
    }
}

/// Emit code to exit a child scenario span.
pub fn emit_child_scenario_span_end() -> TokenStream {
    quote! {
        drop(__child_span_guard);
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

        // Stop BFS at branching steps (Conditional, routing Switch) -
        // branches handled by the step emitter itself
        if branching::is_branching_step(step) {
            continue;
        }

        // Find next steps from execution plan
        for edge in &graph.execution_plan {
            if edge.from_step == step_id {
                // Only follow "source" edges, skip "true"/"false" labels
                let label = edge.label.as_deref().unwrap_or("");
                if label != "true" && label != "false" && !visited.contains(&edge.to_step) {
                    queue.push_back(edge.to_step.clone());
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
///
/// For backward compatibility, returns the first onError edge found.
/// Use `find_on_error_edges` for full conditional edge support.
pub fn find_on_error_step<'a>(
    step_id: &str,
    execution_plan: &'a [ExecutionPlanEdge],
) -> Option<&'a str> {
    find_next_step_for_label(step_id, "onError", execution_plan)
}

/// Find all onError edges for a given step, sorted by priority (highest first).
///
/// Returns edges in evaluation order: highest priority first, then edges without
/// conditions (default fallback) last.
pub fn find_on_error_edges<'a>(
    step_id: &str,
    execution_plan: &'a [ExecutionPlanEdge],
) -> Vec<&'a ExecutionPlanEdge> {
    let mut edges: Vec<_> = execution_plan
        .iter()
        .filter(|e| e.from_step == step_id && e.label.as_deref() == Some("onError"))
        .collect();

    // Sort by priority: higher priority first, condition-less edges last
    edges.sort_by(|a, b| {
        let a_has_condition = a.condition.is_some();
        let b_has_condition = b.condition.is_some();

        // Edges with conditions come before edges without
        match (a_has_condition, b_has_condition) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => {
                // Both have or both lack conditions: sort by priority (higher first)
                let a_priority = a.priority.unwrap_or(0);
                let b_priority = b.priority.unwrap_or(0);
                b_priority.cmp(&a_priority)
            }
        }
    });

    edges
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
        let tokens = emit_step_debug_start(
            &ctx,
            "step-1",
            Some("Test Step"),
            "Agent",
            None,
            None,
            None,
            None,
        );
        assert!(
            tokens.is_empty(),
            "Should emit nothing when debug_mode is false"
        );
    }

    #[test]
    fn test_emit_step_debug_start_emits_code_in_debug_mode() {
        let ctx = make_debug_ctx();
        let tokens = emit_step_debug_start(
            &ctx,
            "step-1",
            Some("Test Step"),
            "Agent",
            None,
            None,
            None,
            None,
        );
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
        assert!(
            code.contains("loop_indices"),
            "Should include loop_indices in payload"
        );
    }

    #[test]
    fn test_emit_step_debug_start_with_inputs_var() {
        let ctx = make_debug_ctx();
        let inputs_var = proc_macro2::Ident::new("my_inputs", proc_macro2::Span::call_site());
        let tokens = emit_step_debug_start(
            &ctx,
            "step-2",
            None,
            "Conditional",
            Some(&inputs_var),
            None,
            None,
            None,
        );
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
            None,
            None,
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
    fn test_emit_step_debug_start_with_scenario_inputs() {
        let ctx = make_debug_ctx();
        let scenario_var =
            proc_macro2::Ident::new("scenario_inputs", proc_macro2::Span::call_site());
        let tokens = emit_step_debug_start(
            &ctx,
            "step-loop",
            None,
            "Agent",
            None,
            None,
            Some(&scenario_var),
            None,
        );
        let code = tokens.to_string();

        // Verify loop_indices extraction from scenario inputs
        assert!(
            code.contains("scenario_inputs"),
            "Should reference scenario inputs variable"
        );
        assert!(
            code.contains("_loop_indices"),
            "Should extract _loop_indices from variables"
        );
    }

    #[test]
    fn test_emit_step_debug_end_disabled_when_not_debug_mode() {
        let ctx = make_non_debug_ctx();
        let tokens =
            emit_step_debug_end(&ctx, "step-1", Some("Test Step"), "Agent", None, None, None);
        assert!(
            tokens.is_empty(),
            "Should emit nothing when debug_mode is false"
        );
    }

    #[test]
    fn test_emit_step_debug_end_emits_code_in_debug_mode() {
        let ctx = make_debug_ctx();
        let tokens =
            emit_step_debug_end(&ctx, "step-1", Some("Test Step"), "Agent", None, None, None);
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
        assert!(
            code.contains("loop_indices"),
            "Should include loop_indices in payload"
        );
    }

    #[test]
    fn test_emit_step_debug_end_with_outputs_var() {
        let ctx = make_debug_ctx();
        let outputs_var = proc_macro2::Ident::new("step_result", proc_macro2::Span::call_site());
        let tokens = emit_step_debug_end(
            &ctx,
            "step-4",
            None,
            "Split",
            Some(&outputs_var),
            None,
            None,
        );
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
    fn test_emit_step_debug_end_with_scenario_inputs() {
        let ctx = make_debug_ctx();
        let scenario_var =
            proc_macro2::Ident::new("scenario_inputs", proc_macro2::Span::call_site());
        let tokens = emit_step_debug_end(
            &ctx,
            "step-loop",
            None,
            "Agent",
            None,
            Some(&scenario_var),
            None,
        );
        let code = tokens.to_string();

        // Verify loop_indices extraction from scenario inputs
        assert!(
            code.contains("scenario_inputs"),
            "Should reference scenario inputs variable"
        );
        assert!(
            code.contains("_loop_indices"),
            "Should extract _loop_indices from variables"
        );
    }

    #[test]
    fn test_emit_step_debug_start_includes_timestamp() {
        let ctx = make_debug_ctx();
        let tokens = emit_step_debug_start(&ctx, "step-5", None, "Finish", None, None, None, None);
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
        let tokens = emit_step_debug_end(&ctx, "step-6", None, "Agent", None, None, None);
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
        let tokens =
            emit_step_debug_start(&ctx, "nameless-step", None, "Agent", None, None, None, None);
        let code = tokens.to_string();

        // Should still work, with None for step_name
        assert!(code.contains("nameless-step"), "Should include step_id");
        assert!(code.contains("step_name"), "Should have step_name field");
    }

    #[test]
    fn test_emit_step_debug_end_without_outputs() {
        let ctx = make_debug_ctx();
        let tokens = emit_step_debug_end(
            &ctx,
            "step-no-output",
            Some("No Output"),
            "Finish",
            None,
            None,
            None,
        );
        let code = tokens.to_string();

        // Should still work, with None for outputs
        assert!(code.contains("step-no-output"), "Should include step_id");
        assert!(code.contains("outputs"), "Should have outputs field");
    }

    #[test]
    fn test_emit_step_debug_generates_truncation_function() {
        let ctx = make_debug_ctx();
        let inputs_var = proc_macro2::Ident::new("big_data", proc_macro2::Span::call_site());
        let tokens = emit_step_debug_start(
            &ctx,
            "step",
            None,
            "Agent",
            Some(&inputs_var),
            None,
            None,
            None,
        );
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

    // Tests for _loop_indices functionality (cache key uniqueness in loops)

    /// Helper to create a minimal ExecutionGraph with just a Finish step
    fn create_minimal_graph(entry_point: &str) -> runtara_dsl::ExecutionGraph {
        use runtara_dsl::{ExecutionGraph, FinishStep, Step};
        use std::collections::HashMap;

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

    #[test]
    fn test_split_emits_loop_indices_injection() {
        use runtara_dsl::{ImmediateValue, MappingValue, SplitConfig, SplitStep};
        use std::collections::HashMap;

        let mut ctx = EmitContext::new(false);

        let split_step = SplitStep {
            id: "split-test".to_string(),
            name: Some("Test Split".to_string()),
            config: Some(SplitConfig {
                value: MappingValue::Immediate(ImmediateValue {
                    value: serde_json::json!([]),
                }),
                parallelism: None,
                sequential: None,
                dont_stop_on_failed: None,
                max_retries: None,
                retry_delay: None,
                timeout: None,
                variables: None,
            }),
            subgraph: Box::new(create_minimal_graph("finish")),
            input_schema: HashMap::new(),
            output_schema: HashMap::new(),
        };

        let tokens = split::emit(&split_step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Verify that _loop_indices injection is present
        assert!(
            code.contains("_loop_indices"),
            "Split should inject _loop_indices into variables"
        );
        assert!(
            code.contains("parent_indices"),
            "Split should preserve parent loop indices"
        );
        assert!(
            code.contains("all_indices"),
            "Split should build cumulative indices array"
        );
    }

    #[test]
    fn test_while_emits_loop_indices_injection() {
        use runtara_dsl::{
            ConditionArgument, ConditionExpression, ConditionOperation, ConditionOperator,
            ImmediateValue, MappingValue, ReferenceValue, WhileConfig, WhileStep,
        };

        let mut ctx = EmitContext::new(false);

        // Create a condition: loop.index < 5
        let condition = ConditionExpression::Operation(ConditionOperation {
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
        });

        let while_step = WhileStep {
            id: "while-test".to_string(),
            name: Some("Test While".to_string()),
            condition,
            config: Some(WhileConfig {
                max_iterations: Some(10),
                timeout: None,
            }),
            subgraph: Box::new(create_minimal_graph("finish")),
        };

        let tokens = while_loop::emit(&while_step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Verify that _loop_indices injection is present
        assert!(
            code.contains("_loop_indices"),
            "While should inject _loop_indices into variables"
        );
        assert!(
            code.contains("__parent_indices"),
            "While should preserve parent loop indices"
        );
        assert!(
            code.contains("__all_indices"),
            "While should build cumulative indices array"
        );
    }

    #[test]
    fn test_agent_emits_dynamic_cache_key() {
        use runtara_dsl::AgentStep;

        let mut ctx = EmitContext::new(false);

        let agent_step = AgentStep {
            id: "agent-test".to_string(),
            name: Some("Test Agent".to_string()),
            agent_id: "http".to_string(),
            capability_id: "request".to_string(),
            input_mapping: None,
            max_retries: None,
            retry_delay: None,
            timeout: None,
            connection_id: None,
            compensation: None,
        };

        let tokens = agent::emit(&agent_step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Verify that dynamic cache key generation is present
        assert!(
            code.contains("__durable_cache_key"),
            "Agent should generate dynamic cache key"
        );
        assert!(
            code.contains("_loop_indices"),
            "Agent should check for _loop_indices in variables"
        );
        assert!(
            code.contains("indices_suffix"),
            "Agent should build indices suffix for cache key"
        );
        // The cache key base is generated as a string literal with the pattern
        // "agent::<agent_id>::<capability_id>::<step_id>"
        assert!(
            code.contains("agent::http::request::agent-test"),
            "Agent should have correct base cache key: got code: {}",
            code
        );
    }

    #[test]
    fn test_start_scenario_emits_dynamic_cache_key() {
        use runtara_dsl::{ChildVersion, StartScenarioStep};
        use std::collections::HashMap;

        // Create a context with a child scenario
        let child_graph = create_minimal_graph("child-finish");

        let mut child_scenarios = HashMap::new();
        // Key by scenario_id::version_resolved
        child_scenarios.insert("child-scenario::1".to_string(), child_graph);

        let mut step_to_child_ref = HashMap::new();
        step_to_child_ref.insert(
            "start-scenario-test".to_string(),
            ("child-scenario".to_string(), 1),
        );

        let mut ctx = EmitContext::with_child_scenarios(
            false,
            child_scenarios,
            step_to_child_ref,
            None,
            None,
        );

        let start_scenario_step = StartScenarioStep {
            id: "start-scenario-test".to_string(),
            name: Some("Test StartScenario".to_string()),
            child_scenario_id: "child-scenario".to_string(),
            child_version: ChildVersion::Latest("latest".to_string()),
            input_mapping: None,
            max_retries: None,
            retry_delay: None,
            timeout: None,
        };

        let tokens = start_scenario::emit(&start_scenario_step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Verify that dynamic cache key generation is present
        assert!(
            code.contains("__durable_cache_key"),
            "StartScenario should generate dynamic cache key"
        );
        assert!(
            code.contains("_loop_indices"),
            "StartScenario should check for _loop_indices in variables"
        );
        assert!(
            code.contains("indices_suffix"),
            "StartScenario should build indices suffix for cache key"
        );
    }

    // ==========================================
    // Tests for loop_indices in debug events
    // ==========================================

    #[test]
    fn test_debug_event_includes_loop_indices_field() {
        let ctx = make_debug_ctx();
        let tokens = emit_step_debug_start(
            &ctx,
            "test-step",
            Some("Test"),
            "Agent",
            None,
            None,
            None,
            None,
        );
        let code = tokens.to_string();

        // Verify the payload includes loop_indices field
        assert!(
            code.contains("\"loop_indices\""),
            "Debug start event should include loop_indices field in payload"
        );
    }

    #[test]
    fn test_debug_end_event_includes_loop_indices_field() {
        let ctx = make_debug_ctx();
        let tokens =
            emit_step_debug_end(&ctx, "test-step", Some("Test"), "Agent", None, None, None);
        let code = tokens.to_string();

        // Verify the payload includes loop_indices field
        assert!(
            code.contains("\"loop_indices\""),
            "Debug end event should include loop_indices field in payload"
        );
    }

    #[test]
    fn test_debug_event_extracts_loop_indices_from_scenario_inputs() {
        let ctx = make_debug_ctx();
        let scenario_var =
            proc_macro2::Ident::new("my_scenario_inputs", proc_macro2::Span::call_site());
        let tokens = emit_step_debug_start(
            &ctx,
            "step-in-loop",
            Some("Step In Loop"),
            "Agent",
            None,
            None,
            Some(&scenario_var),
            None,
        );
        let code = tokens.to_string();

        // Verify loop_indices extraction logic is present
        assert!(
            code.contains("my_scenario_inputs"),
            "Should reference the provided scenario inputs variable"
        );
        // proc_macro2 tokenizes with spaces, so `.variables` becomes `. variables`
        assert!(
            code.contains(". variables"),
            "Should access variables from scenario inputs"
        );
        assert!(
            code.contains("\"_loop_indices\""),
            "Should look for _loop_indices key in variables"
        );
        assert!(
            code.contains("as_object"),
            "Should use as_object() to access variables map"
        );
    }

    #[test]
    fn test_debug_event_defaults_to_empty_array_without_scenario_inputs() {
        let ctx = make_debug_ctx();
        // No scenario_inputs_var provided
        let tokens = emit_step_debug_start(
            &ctx,
            "top-level-step",
            Some("Top Level"),
            "Agent",
            None,
            None,
            None,
            None,
        );
        let code = tokens.to_string();

        // When no scenario inputs var is provided, should default to empty array
        assert!(
            code.contains("serde_json :: Value :: Array (vec ! [])"),
            "Should default to empty array when no scenario inputs provided"
        );
    }

    #[test]
    fn test_debug_event_defaults_to_empty_array_when_loop_indices_missing() {
        let ctx = make_debug_ctx();
        let scenario_var =
            proc_macro2::Ident::new("scenario_inputs", proc_macro2::Span::call_site());
        let tokens = emit_step_debug_start(
            &ctx,
            "step",
            None,
            "Agent",
            None,
            None,
            Some(&scenario_var),
            None,
        );
        let code = tokens.to_string();

        // Should have fallback to empty array if _loop_indices is not present
        assert!(
            code.contains("unwrap_or (serde_json :: Value :: Array (vec ! []))"),
            "Should fallback to empty array when _loop_indices is missing"
        );
    }

    #[test]
    fn test_split_step_passes_scenario_inputs_to_debug_events() {
        use runtara_dsl::{ImmediateValue, MappingValue, SplitConfig, SplitStep};
        use std::collections::HashMap;

        // Enable debug mode to generate debug events
        let mut ctx = EmitContext::new(true);

        let split_step = SplitStep {
            id: "split-debug-test".to_string(),
            name: Some("Debug Split".to_string()),
            config: Some(SplitConfig {
                value: MappingValue::Immediate(ImmediateValue {
                    value: serde_json::json!([1, 2, 3]),
                }),
                parallelism: None,
                sequential: None,
                dont_stop_on_failed: None,
                max_retries: None,
                retry_delay: None,
                timeout: None,
                variables: None,
            }),
            subgraph: Box::new(create_minimal_graph("finish")),
            input_schema: HashMap::new(),
            output_schema: HashMap::new(),
        };

        let tokens = split::emit(&split_step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Verify debug events are emitted with loop_indices
        assert!(
            code.contains("step_debug_start"),
            "Split should emit debug start event"
        );
        assert!(
            code.contains("step_debug_end"),
            "Split should emit debug end event"
        );
        assert!(
            code.contains("\"loop_indices\""),
            "Split debug events should include loop_indices"
        );
    }

    #[test]
    fn test_agent_step_passes_scenario_inputs_to_debug_events() {
        use runtara_dsl::AgentStep;

        // Enable debug mode
        let mut ctx = EmitContext::new(true);

        let agent_step = AgentStep {
            id: "agent-debug-test".to_string(),
            name: Some("Debug Agent".to_string()),
            agent_id: "http".to_string(),
            capability_id: "request".to_string(),
            input_mapping: None,
            max_retries: None,
            retry_delay: None,
            timeout: None,
            connection_id: None,
            compensation: None,
        };

        let tokens = agent::emit(&agent_step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Verify debug events include loop_indices
        assert!(
            code.contains("step_debug_start"),
            "Agent should emit debug start event"
        );
        assert!(
            code.contains("\"loop_indices\""),
            "Agent debug events should include loop_indices"
        );
    }

    #[test]
    fn test_while_step_passes_scenario_inputs_to_debug_events() {
        use runtara_dsl::{
            ConditionArgument, ConditionExpression, ConditionOperation, ConditionOperator,
            ImmediateValue, MappingValue, ReferenceValue, WhileConfig, WhileStep,
        };

        // Enable debug mode
        let mut ctx = EmitContext::new(true);

        let condition = ConditionExpression::Operation(ConditionOperation {
            op: ConditionOperator::Lt,
            arguments: vec![
                ConditionArgument::Value(MappingValue::Reference(ReferenceValue {
                    value: "loop.index".to_string(),
                    type_hint: None,
                    default: None,
                })),
                ConditionArgument::Value(MappingValue::Immediate(ImmediateValue {
                    value: serde_json::json!(3),
                })),
            ],
        });

        let while_step = WhileStep {
            id: "while-debug-test".to_string(),
            name: Some("Debug While".to_string()),
            condition,
            config: Some(WhileConfig {
                max_iterations: Some(5),
                timeout: None,
            }),
            subgraph: Box::new(create_minimal_graph("finish")),
        };

        let tokens = while_loop::emit(&while_step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Verify debug events include loop_indices
        assert!(
            code.contains("step_debug_start"),
            "While should emit debug start event"
        );
        assert!(
            code.contains("\"loop_indices\""),
            "While debug events should include loop_indices"
        );
    }

    #[test]
    fn test_conditional_step_passes_scenario_inputs_to_debug_events() {
        use runtara_dsl::{
            ConditionArgument, ConditionExpression, ConditionOperation, ConditionOperator,
            ConditionalStep, ImmediateValue, MappingValue,
        };

        // Enable debug mode
        let mut ctx = EmitContext::new(true);

        let condition = ConditionExpression::Operation(ConditionOperation {
            op: ConditionOperator::Eq,
            arguments: vec![
                ConditionArgument::Value(MappingValue::Immediate(ImmediateValue {
                    value: serde_json::json!(true),
                })),
                ConditionArgument::Value(MappingValue::Immediate(ImmediateValue {
                    value: serde_json::json!(true),
                })),
            ],
        });

        let conditional_step = ConditionalStep {
            id: "conditional-debug-test".to_string(),
            name: Some("Debug Conditional".to_string()),
            condition,
        };

        let graph = create_minimal_graph("finish");
        let tokens = conditional::emit(&conditional_step, &mut ctx, &graph).unwrap();
        let code = tokens.to_string();

        // Verify debug events include loop_indices
        assert!(
            code.contains("step_debug_start"),
            "Conditional should emit debug start event"
        );
        assert!(
            code.contains("\"loop_indices\""),
            "Conditional debug events should include loop_indices"
        );
    }

    #[test]
    fn test_finish_step_passes_scenario_inputs_to_debug_events() {
        use runtara_dsl::FinishStep;

        // Enable debug mode
        let mut ctx = EmitContext::new(true);

        let finish_step = FinishStep {
            id: "finish-debug-test".to_string(),
            name: Some("Debug Finish".to_string()),
            input_mapping: None,
        };

        let tokens = finish::emit(&finish_step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Verify debug events include loop_indices
        assert!(
            code.contains("step_debug_start"),
            "Finish should emit debug start event"
        );
        assert!(
            code.contains("\"loop_indices\""),
            "Finish debug events should include loop_indices"
        );
    }

    #[test]
    fn test_switch_step_passes_scenario_inputs_to_debug_events() {
        use runtara_dsl::{ImmediateValue, MappingValue, SwitchConfig, SwitchStep};

        // Enable debug mode
        let mut ctx = EmitContext::new(true);

        let switch_step = SwitchStep {
            id: "switch-debug-test".to_string(),
            name: Some("Debug Switch".to_string()),
            config: Some(SwitchConfig {
                value: MappingValue::Immediate(ImmediateValue {
                    value: serde_json::json!("test"),
                }),
                cases: vec![],
                default: None,
            }),
        };

        let graph = create_minimal_graph("finish");
        let tokens = switch::emit(&switch_step, &mut ctx, &graph).unwrap();
        let code = tokens.to_string();

        // Verify debug events include loop_indices
        assert!(
            code.contains("step_debug_start"),
            "Switch should emit debug start event"
        );
        assert!(
            code.contains("\"loop_indices\""),
            "Switch debug events should include loop_indices"
        );
    }

    #[test]
    fn test_debug_events_not_emitted_when_debug_mode_disabled() {
        // Debug mode OFF
        let ctx = make_non_debug_ctx();
        let scenario_var =
            proc_macro2::Ident::new("scenario_inputs", proc_macro2::Span::call_site());

        let start_tokens = emit_step_debug_start(
            &ctx,
            "step",
            None,
            "Agent",
            None,
            None,
            Some(&scenario_var),
            None,
        );
        let end_tokens =
            emit_step_debug_end(&ctx, "step", None, "Agent", None, Some(&scenario_var), None);

        assert!(
            start_tokens.is_empty(),
            "Debug start should not emit when debug_mode is false"
        );
        assert!(
            end_tokens.is_empty(),
            "Debug end should not emit when debug_mode is false"
        );
    }

    #[test]
    fn test_loop_indices_cloned_from_variables() {
        let ctx = make_debug_ctx();
        let scenario_var = proc_macro2::Ident::new("inputs", proc_macro2::Span::call_site());
        let tokens = emit_step_debug_start(
            &ctx,
            "step",
            None,
            "Agent",
            None,
            None,
            Some(&scenario_var),
            None,
        );
        let code = tokens.to_string();

        // Verify we use .cloned() to avoid ownership issues
        // proc_macro2 tokenizes `.cloned()` as `. cloned ()`
        assert!(
            code.contains(". cloned ()"),
            "Should use .cloned() to extract loop_indices value"
        );
    }

    // ── Span helper function tests ───────────────────────────────────

    #[test]
    fn test_emit_step_span_start_basic() {
        let tokens = emit_step_span_start("step-1", Some("Test Step"), "Agent");
        let code = tokens.to_string();

        assert!(
            code.contains("tracing :: info_span !"),
            "Should create info_span"
        );
        assert!(
            code.contains("\"step.agent\""),
            "Span name should be step.<type lowercase>"
        );
        assert!(code.contains("step . id"), "Should include step.id");
        assert!(code.contains("step . name"), "Should include step.name");
        assert!(code.contains("step . type"), "Should include step.type");
        assert!(
            code.contains("otel . kind"),
            "Should include otel.kind attribute"
        );
        assert!(
            code.contains("\"INTERNAL\""),
            "otel.kind should be INTERNAL"
        );
        assert!(
            code.contains("__step_span_guard"),
            "Should create span guard"
        );
    }

    #[test]
    fn test_emit_step_span_start_without_name() {
        let tokens = emit_step_span_start("step-no-name", None, "Conditional");
        let code = tokens.to_string();

        // When no name provided, step_id is used as display name
        assert!(
            code.contains("step-no-name"),
            "Should use step_id as fallback name"
        );
        assert!(
            code.contains("\"step.conditional\""),
            "Span name should be lowercase step type"
        );
    }

    #[test]
    fn test_emit_step_span_end() {
        let tokens = emit_step_span_end();
        let code = tokens.to_string();

        assert!(
            code.contains("drop (__step_span_guard)"),
            "Should drop the span guard"
        );
    }

    #[test]
    fn test_emit_agent_span_start() {
        let tokens = emit_agent_span_start(
            "agent-step-1",
            Some("Fetch Data"),
            "http-agent",
            "fetch-json",
        );
        let code = tokens.to_string();

        assert!(
            code.contains("\"step.agent\""),
            "Span name should be step.agent"
        );
        assert!(code.contains("agent . id"), "Should include agent.id");
        assert!(
            code.contains("capability . id"),
            "Should include capability.id"
        );
        assert!(code.contains("http-agent"), "Should include agent_id value");
        assert!(
            code.contains("fetch-json"),
            "Should include capability_id value"
        );
    }

    #[test]
    fn test_emit_iteration_span_start() {
        let idx_var = proc_macro2::Ident::new("idx", proc_macro2::Span::call_site());
        let tokens = emit_iteration_span_start("split-1", "split", &idx_var);
        let code = tokens.to_string();

        assert!(
            code.contains("\"split.iteration\""),
            "Span name should be <type>.iteration"
        );
        assert!(
            code.contains("iteration . index"),
            "Should include iteration.index"
        );
        assert!(code.contains("idx"), "Should reference the index variable");
        assert!(code.contains("__iter_span"), "Should create iteration span");
        assert!(
            code.contains("__iter_span_guard"),
            "Should create iteration span guard"
        );
    }

    #[test]
    fn test_emit_iteration_span_end() {
        let tokens = emit_iteration_span_end();
        let code = tokens.to_string();

        assert!(
            code.contains("drop (__iter_span_guard)"),
            "Should drop the iteration span guard"
        );
    }

    #[test]
    fn test_emit_child_scenario_span_start() {
        let tokens = emit_child_scenario_span_start("start-scenario-1", "child-scenario-abc");
        let code = tokens.to_string();

        assert!(
            code.contains("\"scenario.child\""),
            "Span name should be scenario.child"
        );
        assert!(code.contains("scenario . id"), "Should include scenario.id");
        assert!(
            code.contains("parent_step . id"),
            "Should include parent_step.id"
        );
        assert!(
            code.contains("child-scenario-abc"),
            "Should include child scenario ID"
        );
        assert!(
            code.contains("start-scenario-1"),
            "Should include parent step ID"
        );
        assert!(code.contains("__child_span"), "Should create child span");
    }

    #[test]
    fn test_emit_child_scenario_span_end() {
        let tokens = emit_child_scenario_span_end();
        let code = tokens.to_string();

        assert!(
            code.contains("drop (__child_span_guard)"),
            "Should drop the child span guard"
        );
    }

    #[test]
    fn test_span_step_type_lowercase() {
        // Verify different step types produce correct lowercase span names
        let types_and_expected = [
            ("Agent", "step.agent"),
            ("Conditional", "step.conditional"),
            ("Switch", "step.switch"),
            ("Filter", "step.filter"),
            ("GroupBy", "step.groupby"),
            ("Finish", "step.finish"),
            ("Error", "step.error"),
            ("Log", "step.log"),
        ];

        for (step_type, expected_name) in types_and_expected {
            let tokens = emit_step_span_start("test", None, step_type);
            let code = tokens.to_string();
            assert!(
                code.contains(&format!("\"{}\"", expected_name)),
                "Step type {} should produce span name {}",
                step_type,
                expected_name
            );
        }
    }
}
