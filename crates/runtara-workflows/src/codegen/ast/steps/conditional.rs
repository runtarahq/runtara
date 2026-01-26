// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Conditional step emitter.
//!
//! The Conditional step evaluates conditions and branches execution.
//! Conditions are defined via the structured `condition` field using ConditionExpression.

use proc_macro2::TokenStream;
use quote::quote;
use std::collections::HashSet;

use super::super::CodegenError;
use super::super::context::EmitContext;
use super::super::mapping::{self, emit_mapping_value};
use super::super::steps;
use super::{StepEmitter, emit_step_debug_end, emit_step_debug_start};
use runtara_dsl::{
    ConditionArgument, ConditionExpression, ConditionOperation, ConditionOperator, ConditionalStep,
    ExecutionGraph, Step,
};

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
    let merge_point = find_merge_point(
        true_step_id.map(|s| s.to_string()),
        false_step_id.map(|s| s.to_string()),
        graph,
    );

    // Emit code for the true branch (stopping at merge point)
    let true_branch_code = if let Some(start_step_id) = true_step_id {
        emit_branch_code(start_step_id, graph, ctx, merge_point.as_deref())?
    } else {
        quote! {}
    };

    // Emit code for the false branch (stopping at merge point)
    let false_branch_code = if let Some(start_step_id) = false_step_id {
        emit_branch_code(start_step_id, graph, ctx, merge_point.as_deref())?
    } else {
        quote! {}
    };

    // Emit code for the common suffix path after the merge point
    let common_suffix_code = if let Some(ref merge_step_id) = merge_point {
        emit_branch_code(merge_step_id, graph, ctx, None)?
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

    Ok(quote! {
        let #source_var = #build_source;
        let #condition_inputs_var = serde_json::json!({"condition": "evaluating"});

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

/// Find the merge point where two branches converge (diamond pattern detection).
///
/// This function traces both branches from a conditional and finds the first step
/// that is reachable from both paths. This is the "merge point" where the diamond
/// pattern converges.
///
/// Returns `None` if:
/// - Either branch is None (one-sided conditional)
/// - Branches don't converge (both end in Finish steps or different paths)
/// - Branches are the same step (no branching)
fn find_merge_point(
    true_start: Option<String>,
    false_start: Option<String>,
    graph: &ExecutionGraph,
) -> Option<String> {
    let true_start = true_start?;
    let false_start = false_start?;

    // Collect all steps reachable from the true branch
    let true_reachable = collect_reachable_steps(&true_start, graph);

    // Collect all steps reachable from the false branch
    let false_reachable = collect_reachable_steps(&false_start, graph);

    // Find the first step in true_reachable that is also in false_reachable
    // We iterate in order (BFS order from true branch) to find the earliest merge point
    for step_id in &true_reachable {
        if false_reachable.contains(step_id) {
            return Some(step_id.clone());
        }
    }

    None
}

/// Collect all steps reachable from a starting point using BFS.
/// Returns steps in BFS order (closest first).
fn collect_reachable_steps(start_step_id: &str, graph: &ExecutionGraph) -> Vec<String> {
    use std::collections::VecDeque;

    let mut reachable = Vec::new();
    let mut visited = HashSet::new();
    let mut queue = VecDeque::new();

    queue.push_back(start_step_id.to_string());

    while let Some(current_step_id) = queue.pop_front() {
        if visited.contains(&current_step_id) {
            continue;
        }
        visited.insert(current_step_id.clone());
        reachable.push(current_step_id.clone());

        // Get the step
        let step = match graph.steps.get(&current_step_id) {
            Some(s) => s,
            None => continue,
        };

        // Stop at Finish steps (they return, no further steps)
        if matches!(step, Step::Finish(_)) {
            continue;
        }

        // For Conditional steps, add both branches to the queue
        if matches!(step, Step::Conditional(_)) {
            for edge in &graph.execution_plan {
                if edge.from_step == current_step_id {
                    let label = edge.label.as_deref().unwrap_or("");
                    if (label == "true" || label == "false") && !visited.contains(&edge.to_step) {
                        queue.push_back(edge.to_step.clone());
                    }
                }
            }
            continue;
        }

        // Find the next step (follow "next" label or unlabeled edge)
        for edge in &graph.execution_plan {
            if edge.from_step == current_step_id {
                let label = edge.label.as_deref().unwrap_or("");
                // Follow "next" or unlabeled edges, skip "true"/"false" branches
                if label == "next" || label.is_empty() {
                    if !visited.contains(&edge.to_step) {
                        queue.push_back(edge.to_step.clone());
                    }
                    break;
                }
            }
        }
    }

    reachable
}

/// Collect all steps along a branch until we hit a Finish step, another Conditional,
/// or the specified stop_at step (merge point).
fn collect_branch_steps(
    start_step_id: &str,
    graph: &ExecutionGraph,
    stop_at: Option<&str>,
) -> Vec<String> {
    let mut branch_steps = Vec::new();
    let mut visited = HashSet::new();
    let mut current_step_id = start_step_id.to_string();

    loop {
        if visited.contains(&current_step_id) {
            break;
        }

        // Stop before the merge point (it will be emitted separately after the if/else)
        if let Some(merge_point) = stop_at
            && current_step_id == merge_point
        {
            break;
        }

        visited.insert(current_step_id.clone());

        // Get the step
        let step = match graph.steps.get(&current_step_id) {
            Some(s) => s,
            None => break,
        };

        branch_steps.push(current_step_id.clone());

        // Stop at Finish steps (they return)
        if matches!(step, Step::Finish(_)) {
            break;
        }

        // Stop at Conditional steps (they have their own branches)
        // But include the conditional - it will emit its own branch code
        if matches!(step, Step::Conditional(_)) {
            break;
        }

        // Find the next step (follow "next" label or unlabeled edge)
        let mut next_step_id = None;
        for edge in &graph.execution_plan {
            if edge.from_step == current_step_id {
                let label = edge.label.as_deref().unwrap_or("");
                // Follow "next" or unlabeled edges, skip "true"/"false" branches
                if label == "next" || label.is_empty() {
                    next_step_id = Some(edge.to_step.clone());
                    break;
                }
            }
        }

        match next_step_id {
            Some(next) => current_step_id = next,
            None => break,
        }
    }

    branch_steps
}

/// Emit code for a branch (sequence of steps).
///
/// If `stop_at` is provided, the branch will stop before that step (used for merge points).
fn emit_branch_code(
    start_step_id: &str,
    graph: &ExecutionGraph,
    ctx: &mut EmitContext,
    stop_at: Option<&str>,
) -> Result<TokenStream, CodegenError> {
    let branch_steps = collect_branch_steps(start_step_id, graph, stop_at);

    let step_codes: Vec<TokenStream> = branch_steps
        .iter()
        .filter_map(|step_id| graph.steps.get(step_id))
        .map(|step| step.emit(ctx, graph))
        .collect::<Result<Vec<_>, _>>()?;

    Ok(quote! {
        #(#step_codes)*
    })
}

// ============================================================================
// Condition Expression Evaluation
// ============================================================================

/// Emit code for a ConditionExpression.
pub fn emit_condition_expression(
    expr: &ConditionExpression,
    ctx: &mut EmitContext,
    source_var: &proc_macro2::Ident,
) -> TokenStream {
    match expr {
        ConditionExpression::Operation(op) => emit_operation(op, ctx, source_var),
        ConditionExpression::Value(mapping_value) => {
            // A direct value - evaluate as truthy
            let value_code = emit_mapping_value(mapping_value, ctx, source_var);
            quote! {
                {
                    let val = #value_code;
                    is_truthy(&val)
                }
            }
        }
    }
}

/// Emit code for a ConditionOperation.
fn emit_operation(
    op: &ConditionOperation,
    ctx: &mut EmitContext,
    source_var: &proc_macro2::Ident,
) -> TokenStream {
    let arguments = &op.arguments;

    match op.op {
        // Logical operators
        ConditionOperator::And => {
            if arguments.is_empty() {
                return quote! { true };
            }
            let arg_codes: Vec<TokenStream> = arguments
                .iter()
                .map(|arg| emit_argument_as_bool(arg, ctx, source_var))
                .collect();
            quote! { #(#arg_codes)&&* }
        }
        ConditionOperator::Or => {
            if arguments.is_empty() {
                return quote! { false };
            }
            let arg_codes: Vec<TokenStream> = arguments
                .iter()
                .map(|arg| emit_argument_as_bool(arg, ctx, source_var))
                .collect();
            quote! { #(#arg_codes)||* }
        }
        ConditionOperator::Not => {
            if arguments.is_empty() {
                return quote! { true };
            }
            let arg_code = emit_argument_as_bool(&arguments[0], ctx, source_var);
            quote! { !(#arg_code) }
        }

        // Comparison operators
        ConditionOperator::Gt => emit_comparison(arguments, ctx, source_var, quote! { > }),
        ConditionOperator::Gte => emit_comparison(arguments, ctx, source_var, quote! { >= }),
        ConditionOperator::Lt => emit_comparison(arguments, ctx, source_var, quote! { < }),
        ConditionOperator::Lte => emit_comparison(arguments, ctx, source_var, quote! { <= }),
        ConditionOperator::Eq => emit_equality(arguments, ctx, source_var, false),
        ConditionOperator::Ne => emit_equality(arguments, ctx, source_var, true),

        // String operators
        ConditionOperator::StartsWith => emit_starts_with(arguments, ctx, source_var),
        ConditionOperator::EndsWith => emit_ends_with(arguments, ctx, source_var),

        // Array operators
        ConditionOperator::Contains => emit_contains(arguments, ctx, source_var),
        ConditionOperator::In => emit_in(arguments, ctx, source_var),
        ConditionOperator::NotIn => {
            let in_code = emit_in(arguments, ctx, source_var);
            quote! { !(#in_code) }
        }

        // Utility operators
        ConditionOperator::Length => {
            // When used as a boolean, non-zero length is truthy
            if arguments.is_empty() {
                return quote! { false };
            }
            let arg_code = emit_argument_as_value(&arguments[0], ctx, source_var);
            quote! {
                {
                    let val = #arg_code;
                    let len: i64 = match &val {
                        serde_json::Value::String(s) => s.len() as i64,
                        serde_json::Value::Array(a) => a.len() as i64,
                        serde_json::Value::Object(o) => o.len() as i64,
                        serde_json::Value::Null => 0,
                        _ => 1,
                    };
                    len > 0
                }
            }
        }
        ConditionOperator::IsDefined => {
            if arguments.is_empty() {
                return quote! { false };
            }
            let arg_code = emit_argument_as_value(&arguments[0], ctx, source_var);
            quote! { !#arg_code.is_null() }
        }
        ConditionOperator::IsNotEmpty => {
            if arguments.is_empty() {
                return quote! { false };
            }
            let arg_code = emit_argument_as_value(&arguments[0], ctx, source_var);
            quote! {
                {
                    let val = #arg_code;
                    match &val {
                        serde_json::Value::Array(a) => !a.is_empty(),
                        serde_json::Value::String(s) => !s.is_empty(),
                        serde_json::Value::Object(o) => !o.is_empty(),
                        serde_json::Value::Null => false,
                        _ => true,
                    }
                }
            }
        }
        ConditionOperator::IsEmpty => {
            if arguments.is_empty() {
                return quote! { true };
            }
            let arg_code = emit_argument_as_value(&arguments[0], ctx, source_var);
            quote! {
                {
                    let val = #arg_code;
                    match &val {
                        serde_json::Value::Array(a) => a.is_empty(),
                        serde_json::Value::String(s) => s.is_empty(),
                        serde_json::Value::Object(o) => o.is_empty(),
                        serde_json::Value::Null => true,
                        _ => false,
                    }
                }
            }
        }
    }
}

/// Emit code for a ConditionArgument that returns a bool.
fn emit_argument_as_bool(
    arg: &ConditionArgument,
    ctx: &mut EmitContext,
    source_var: &proc_macro2::Ident,
) -> TokenStream {
    match arg {
        ConditionArgument::Expression(expr) => emit_condition_expression(expr, ctx, source_var),
        ConditionArgument::Value(mapping_value) => {
            let value_code = emit_mapping_value(mapping_value, ctx, source_var);
            quote! {
                {
                    let val = #value_code;
                    is_truthy(&val)
                }
            }
        }
    }
}

/// Emit code for a ConditionArgument that returns a Value.
fn emit_argument_as_value(
    arg: &ConditionArgument,
    ctx: &mut EmitContext,
    source_var: &proc_macro2::Ident,
) -> TokenStream {
    match arg {
        ConditionArgument::Expression(expr) => {
            // Evaluate expression and wrap result in Value
            let bool_code = emit_condition_expression(expr, ctx, source_var);
            quote! { serde_json::Value::Bool(#bool_code) }
        }
        ConditionArgument::Value(mapping_value) => {
            emit_mapping_value(mapping_value, ctx, source_var)
        }
    }
}

// ============================================================================
// Operator Implementations
// ============================================================================

/// Emit comparison code (GT, GTE, LT, LTE).
fn emit_comparison(
    arguments: &[ConditionArgument],
    ctx: &mut EmitContext,
    source_var: &proc_macro2::Ident,
    op: TokenStream,
) -> TokenStream {
    if arguments.len() < 2 {
        return quote! { false };
    }

    let left_code = emit_argument_as_value(&arguments[0], ctx, source_var);
    let right_code = emit_argument_as_value(&arguments[1], ctx, source_var);

    quote! {
        {
            let left_val = #left_code;
            let right_val = #right_code;
            let left_num = to_number(&left_val);
            let right_num = to_number(&right_val);
            match (left_num, right_num) {
                (Some(l), Some(r)) => l #op r,
                _ => false,
            }
        }
    }
}

/// Emit equality/inequality code.
fn emit_equality(
    arguments: &[ConditionArgument],
    ctx: &mut EmitContext,
    source_var: &proc_macro2::Ident,
    negate: bool,
) -> TokenStream {
    if arguments.len() < 2 {
        return quote! { false };
    }

    let left_code = emit_argument_as_value(&arguments[0], ctx, source_var);
    let right_code = emit_argument_as_value(&arguments[1], ctx, source_var);

    let eq_check = quote! {
        {
            let left_val = #left_code;
            let right_val = #right_code;
            values_equal(&left_val, &right_val)
        }
    };

    if negate {
        quote! { !(#eq_check) }
    } else {
        eq_check
    }
}

/// Emit CONTAINS code (array contains value).
fn emit_contains(
    arguments: &[ConditionArgument],
    ctx: &mut EmitContext,
    source_var: &proc_macro2::Ident,
) -> TokenStream {
    if arguments.len() < 2 {
        return quote! { false };
    }

    let array_code = emit_argument_as_value(&arguments[0], ctx, source_var);
    let value_code = emit_argument_as_value(&arguments[1], ctx, source_var);

    quote! {
        {
            let arr_val = #array_code;
            let search_val = #value_code;
            if let Some(arr) = arr_val.as_array() {
                arr.iter().any(|item| values_equal(item, &search_val))
            } else {
                false
            }
        }
    }
}

/// Emit IN code (value in array).
fn emit_in(
    arguments: &[ConditionArgument],
    ctx: &mut EmitContext,
    source_var: &proc_macro2::Ident,
) -> TokenStream {
    if arguments.len() < 2 {
        return quote! { false };
    }

    let value_code = emit_argument_as_value(&arguments[0], ctx, source_var);
    let array_code = emit_argument_as_value(&arguments[1], ctx, source_var);

    quote! {
        {
            let search_val = #value_code;
            let arr_val = #array_code;
            if let Some(arr) = arr_val.as_array() {
                arr.iter().any(|item| values_equal(&search_val, item))
            } else {
                false
            }
        }
    }
}

/// Emit STARTS_WITH code (string starts with prefix).
fn emit_starts_with(
    arguments: &[ConditionArgument],
    ctx: &mut EmitContext,
    source_var: &proc_macro2::Ident,
) -> TokenStream {
    if arguments.len() < 2 {
        return quote! { false };
    }

    let string_code = emit_argument_as_value(&arguments[0], ctx, source_var);
    let prefix_code = emit_argument_as_value(&arguments[1], ctx, source_var);

    quote! {
        {
            let str_val = #string_code;
            let prefix_val = #prefix_code;
            match (str_val.as_str(), prefix_val.as_str()) {
                (Some(s), Some(p)) => s.starts_with(p),
                _ => false,
            }
        }
    }
}

/// Emit ENDS_WITH code (string ends with suffix).
fn emit_ends_with(
    arguments: &[ConditionArgument],
    ctx: &mut EmitContext,
    source_var: &proc_macro2::Ident,
) -> TokenStream {
    if arguments.len() < 2 {
        return quote! { false };
    }

    let string_code = emit_argument_as_value(&arguments[0], ctx, source_var);
    let suffix_code = emit_argument_as_value(&arguments[1], ctx, source_var);

    quote! {
        {
            let str_val = #string_code;
            let suffix_val = #suffix_code;
            match (str_val.as_str(), suffix_val.as_str()) {
                (Some(s), Some(suf)) => s.ends_with(suf),
                _ => false,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtara_dsl::{ExecutionPlanEdge, FinishStep, LogLevel, LogStep, MappingValue};
    use std::collections::HashMap;

    /// Helper to create a simple Log step for testing
    fn make_log_step(id: &str) -> Step {
        Step::Log(LogStep {
            id: id.to_string(),
            name: Some(format!("Log {}", id)),
            message: "test".to_string(),
            level: LogLevel::Info,
            context: None,
        })
    }

    /// Helper to create a Finish step for testing
    fn make_finish_step(id: &str) -> Step {
        Step::Finish(FinishStep {
            id: id.to_string(),
            name: Some(format!("Finish {}", id)),
            input_mapping: None,
        })
    }

    /// Helper to create a Conditional step for testing
    fn make_conditional_step(id: &str) -> Step {
        Step::Conditional(ConditionalStep {
            id: id.to_string(),
            name: Some(format!("Conditional {}", id)),
            condition: ConditionExpression::Value(MappingValue::Immediate(
                runtara_dsl::ImmediateValue {
                    value: serde_json::json!(true),
                },
            )),
        })
    }

    /// Helper to create an edge in the execution plan
    fn edge(from: &str, to: &str, label: Option<&str>) -> ExecutionPlanEdge {
        ExecutionPlanEdge {
            from_step: from.to_string(),
            to_step: to.to_string(),
            label: label.map(|s| s.to_string()),
            condition: None,
            priority: None,
        }
    }

    /// Helper to create a minimal ExecutionGraph for testing
    fn make_graph(
        entry_point: &str,
        steps: HashMap<String, Step>,
        execution_plan: Vec<ExecutionPlanEdge>,
    ) -> ExecutionGraph {
        ExecutionGraph {
            name: None,
            description: None,
            entry_point: entry_point.to_string(),
            steps,
            execution_plan,
            variables: HashMap::new(),
            input_schema: HashMap::new(),
            output_schema: HashMap::new(),
            notes: None,
            nodes: None,
            edges: None,
        }
    }

    #[test]
    fn test_find_merge_point_diamond_pattern() {
        // Diamond pattern:
        //       cond
        //      /    \
        //   step1  step2
        //      \    /
        //      merge
        //        |
        //      finish

        let mut steps = HashMap::new();
        steps.insert("cond".to_string(), make_conditional_step("cond"));
        steps.insert("step1".to_string(), make_log_step("step1"));
        steps.insert("step2".to_string(), make_log_step("step2"));
        steps.insert("merge".to_string(), make_log_step("merge"));
        steps.insert("finish".to_string(), make_finish_step("finish"));

        let graph = make_graph(
            "cond",
            steps,
            vec![
                edge("cond", "step1", Some("true")),
                edge("cond", "step2", Some("false")),
                edge("step1", "merge", None),
                edge("step2", "merge", None),
                edge("merge", "finish", None),
            ],
        );

        let merge_point =
            find_merge_point(Some("step1".to_string()), Some("step2".to_string()), &graph);

        assert_eq!(merge_point, Some("merge".to_string()));
    }

    #[test]
    fn test_find_merge_point_no_merge() {
        // No merge - branches end in different Finish steps:
        //       cond
        //      /    \
        //   step1  step2
        //      |      |
        //   finish1 finish2

        let mut steps = HashMap::new();
        steps.insert("cond".to_string(), make_conditional_step("cond"));
        steps.insert("step1".to_string(), make_log_step("step1"));
        steps.insert("step2".to_string(), make_log_step("step2"));
        steps.insert("finish1".to_string(), make_finish_step("finish1"));
        steps.insert("finish2".to_string(), make_finish_step("finish2"));

        let graph = make_graph(
            "cond",
            steps,
            vec![
                edge("cond", "step1", Some("true")),
                edge("cond", "step2", Some("false")),
                edge("step1", "finish1", None),
                edge("step2", "finish2", None),
            ],
        );

        let merge_point =
            find_merge_point(Some("step1".to_string()), Some("step2".to_string()), &graph);

        assert_eq!(merge_point, None);
    }

    #[test]
    fn test_find_merge_point_immediate_merge() {
        // Immediate merge - both branches go directly to the same step:
        //       cond
        //      /    \
        //      merge (both branches)
        //        |
        //      finish

        let mut steps = HashMap::new();
        steps.insert("cond".to_string(), make_conditional_step("cond"));
        steps.insert("merge".to_string(), make_log_step("merge"));
        steps.insert("finish".to_string(), make_finish_step("finish"));

        let graph = make_graph(
            "cond",
            steps,
            vec![
                edge("cond", "merge", Some("true")),
                edge("cond", "merge", Some("false")),
                edge("merge", "finish", None),
            ],
        );

        let merge_point =
            find_merge_point(Some("merge".to_string()), Some("merge".to_string()), &graph);

        // When both branches start at the same step, that's the merge point
        assert_eq!(merge_point, Some("merge".to_string()));
    }

    #[test]
    fn test_find_merge_point_one_sided_conditional() {
        // One-sided conditional (only true branch):
        //       cond
        //         \
        //        step1
        //          |
        //        finish

        let mut steps = HashMap::new();
        steps.insert("cond".to_string(), make_conditional_step("cond"));
        steps.insert("step1".to_string(), make_log_step("step1"));
        steps.insert("finish".to_string(), make_finish_step("finish"));

        let graph = make_graph(
            "cond",
            steps,
            vec![
                edge("cond", "step1", Some("true")),
                // No false branch
                edge("step1", "finish", None),
            ],
        );

        let merge_point = find_merge_point(Some("step1".to_string()), None, &graph);

        assert_eq!(merge_point, None);
    }

    #[test]
    fn test_find_merge_point_complex_diamond() {
        // Complex diamond with multiple steps in each branch:
        //         cond
        //        /    \
        //     step1  step3
        //       |      |
        //     step2  step4
        //        \    /
        //        merge
        //          |
        //        finish

        let mut steps = HashMap::new();
        steps.insert("cond".to_string(), make_conditional_step("cond"));
        steps.insert("step1".to_string(), make_log_step("step1"));
        steps.insert("step2".to_string(), make_log_step("step2"));
        steps.insert("step3".to_string(), make_log_step("step3"));
        steps.insert("step4".to_string(), make_log_step("step4"));
        steps.insert("merge".to_string(), make_log_step("merge"));
        steps.insert("finish".to_string(), make_finish_step("finish"));

        let graph = make_graph(
            "cond",
            steps,
            vec![
                edge("cond", "step1", Some("true")),
                edge("cond", "step3", Some("false")),
                edge("step1", "step2", None),
                edge("step2", "merge", None),
                edge("step3", "step4", None),
                edge("step4", "merge", None),
                edge("merge", "finish", None),
            ],
        );

        let merge_point =
            find_merge_point(Some("step1".to_string()), Some("step3".to_string()), &graph);

        assert_eq!(merge_point, Some("merge".to_string()));
    }

    #[test]
    fn test_collect_branch_steps_stops_at_merge_point() {
        // Same diamond as above
        let mut steps = HashMap::new();
        steps.insert("cond".to_string(), make_conditional_step("cond"));
        steps.insert("step1".to_string(), make_log_step("step1"));
        steps.insert("step2".to_string(), make_log_step("step2"));
        steps.insert("merge".to_string(), make_log_step("merge"));
        steps.insert("finish".to_string(), make_finish_step("finish"));

        let graph = make_graph(
            "cond",
            steps,
            vec![
                edge("cond", "step1", Some("true")),
                edge("cond", "step2", Some("false")),
                edge("step1", "merge", None),
                edge("step2", "merge", None),
                edge("merge", "finish", None),
            ],
        );

        // With merge point, should stop before it
        let branch_steps_with_stop = collect_branch_steps("step1", &graph, Some("merge"));
        assert_eq!(branch_steps_with_stop, vec!["step1"]);

        // Without merge point, should include merge and continue to finish
        let branch_steps_no_stop = collect_branch_steps("step1", &graph, None);
        assert_eq!(branch_steps_no_stop, vec!["step1", "merge", "finish"]);
    }

    #[test]
    fn test_collect_reachable_steps() {
        // Test the reachability traversal
        let mut steps = HashMap::new();
        steps.insert("start".to_string(), make_log_step("start"));
        steps.insert("middle".to_string(), make_log_step("middle"));
        steps.insert("end".to_string(), make_finish_step("end"));

        let graph = make_graph(
            "start",
            steps,
            vec![edge("start", "middle", None), edge("middle", "end", None)],
        );

        let reachable = collect_reachable_steps("start", &graph);
        assert_eq!(reachable, vec!["start", "middle", "end"]);

        let reachable_from_middle = collect_reachable_steps("middle", &graph);
        assert_eq!(reachable_from_middle, vec!["middle", "end"]);
    }

    #[test]
    fn test_nested_conditionals_with_diamond() {
        // Nested diamond:
        //         cond1
        //        /    \
        //     step1  cond2
        //       |    /    \
        //       | step2  step3
        //       |    \    /
        //       |    merge2
        //        \    /
        //        merge1
        //          |
        //        finish

        let mut steps = HashMap::new();
        steps.insert("cond1".to_string(), make_conditional_step("cond1"));
        steps.insert("step1".to_string(), make_log_step("step1"));
        steps.insert("cond2".to_string(), make_conditional_step("cond2"));
        steps.insert("step2".to_string(), make_log_step("step2"));
        steps.insert("step3".to_string(), make_log_step("step3"));
        steps.insert("merge2".to_string(), make_log_step("merge2"));
        steps.insert("merge1".to_string(), make_log_step("merge1"));
        steps.insert("finish".to_string(), make_finish_step("finish"));

        let graph = make_graph(
            "cond1",
            steps,
            vec![
                edge("cond1", "step1", Some("true")),
                edge("cond1", "cond2", Some("false")),
                edge("step1", "merge1", None),
                edge("cond2", "step2", Some("true")),
                edge("cond2", "step3", Some("false")),
                edge("step2", "merge2", None),
                edge("step3", "merge2", None),
                edge("merge2", "merge1", None),
                edge("merge1", "finish", None),
            ],
        );

        // For outer conditional, merge point should be merge1
        let outer_merge =
            find_merge_point(Some("step1".to_string()), Some("cond2".to_string()), &graph);
        assert_eq!(outer_merge, Some("merge1".to_string()));

        // For inner conditional, merge point should be merge2
        let inner_merge =
            find_merge_point(Some("step2".to_string()), Some("step3".to_string()), &graph);
        assert_eq!(inner_merge, Some("merge2".to_string()));
    }
}
