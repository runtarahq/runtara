// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Step emitters for AST-based code generation.
//!
//! Each step type has its own emitter that generates the TokenStream
//! for executing that step.

pub mod agent;
pub mod conditional;
pub mod finish;
pub mod split;
pub mod start_scenario;
pub mod switch;

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
    }
}

/// Emit debug logging for step execution start.
pub fn emit_step_debug_start(
    ctx: &EmitContext,
    step_id: &str,
    step_name: Option<&str>,
    step_type: &str,
) -> TokenStream {
    if ctx.debug_mode {
        let name_display = step_name.unwrap_or("Unnamed");
        let runtime_ctx = &ctx.runtime_ctx_var;
        quote! {
            #runtime_ctx.step_started(#step_id, #name_display, #step_type);
        }
    } else {
        quote! {}
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
