// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Conditional step emitter.
//!
//! The Conditional step evaluates conditions and branches execution.
//! Conditions are defined via the structured `condition` field using ConditionExpression.

use proc_macro2::TokenStream;
use quote::quote;
use std::collections::HashSet;

use super::super::context::EmitContext;
use super::super::mapping;
use super::super::steps;
use super::StepEmitter;
use runtara_dsl::{
    ConditionArgument, ConditionExpression, ConditionOperation, ConditionOperator, ConditionalStep,
    ExecutionGraph, MappingValue, Step,
};

/// Emit code for a Conditional step.
pub fn emit(step: &ConditionalStep, ctx: &mut EmitContext, graph: &ExecutionGraph) -> TokenStream {
    let step_id = &step.id;
    let step_name = step.name.as_deref().unwrap_or("Unnamed");
    let debug_mode = ctx.debug_mode;
    let execution_plan = &graph.execution_plan;

    // Do all mutable operations first
    let step_var = ctx.declare_step(step_id);
    let source_var = ctx.temp_var("source");
    let condition_var = ctx.temp_var("condition_result");

    // Clone immutable references
    let steps_context = ctx.steps_context_var.clone();
    let runtime_ctx = ctx.runtime_ctx_var.clone();

    // Build the source for input mapping
    let build_source = mapping::emit_build_source(ctx);

    // Generate condition evaluation from the structured condition
    let condition_eval = emit_condition_expression(&step.condition, ctx, &source_var);

    // Find the true and false branch starting steps
    let true_step_id = steps::find_next_step_for_label(step_id, "true", execution_plan);
    let false_step_id = steps::find_next_step_for_label(step_id, "false", execution_plan);

    // Emit code for the true branch
    let true_branch_code = if let Some(start_step_id) = true_step_id {
        emit_branch_code(start_step_id, graph, ctx)
    } else {
        quote! {}
    };

    // Emit code for the false branch
    let false_branch_code = if let Some(start_step_id) = false_step_id {
        emit_branch_code(start_step_id, graph, ctx)
    } else {
        quote! {}
    };

    // Debug timing variables
    let debug_start_time_var = ctx.temp_var("step_start_time");
    let debug_duration_var = ctx.temp_var("duration_ms");
    let condition_inputs_var = ctx.temp_var("condition_inputs");

    let debug_start = if debug_mode {
        quote! {
            let #condition_inputs_var = serde_json::json!({"condition": "evaluating"});
            #runtime_ctx.step_started(#step_id, "Conditional", &#condition_inputs_var);
            let #debug_start_time_var = std::time::Instant::now();
        }
    } else {
        quote! {}
    };

    let debug_complete = if debug_mode {
        quote! {
            let #debug_duration_var = #debug_start_time_var.elapsed().as_millis() as u64;
            #runtime_ctx.step_completed(#step_id, &#step_var, #debug_duration_var);
        }
    } else {
        quote! {}
    };

    let debug_log = if debug_mode {
        quote! {
            eprintln!("  -> Condition result: {}", #condition_var);
            if #condition_var {
                eprintln!("  -> Taking TRUE branch");
            } else {
                eprintln!("  -> Taking FALSE branch");
            }
        }
    } else {
        quote! {}
    };

    quote! {
        let #source_var = #build_source;

        #debug_start

        let #condition_var: bool = #condition_eval;

        #debug_log

        let #step_var = serde_json::json!({
            "stepId": #step_id,
            "stepName": #step_name,
            "stepType": "Conditional",
            "outputs": {
                "result": #condition_var
            }
        });

        #debug_complete

        #steps_context.insert(#step_id.to_string(), #step_var.clone());

        // Execute the appropriate branch
        if #condition_var {
            #true_branch_code
        } else {
            #false_branch_code
        }
    }
}

/// Collect all steps along a branch until we hit a Finish step or another Conditional.
fn collect_branch_steps(start_step_id: &str, graph: &ExecutionGraph) -> Vec<String> {
    let mut branch_steps = Vec::new();
    let mut visited = HashSet::new();
    let mut current_step_id = start_step_id.to_string();

    loop {
        if visited.contains(&current_step_id) {
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
fn emit_branch_code(
    start_step_id: &str,
    graph: &ExecutionGraph,
    ctx: &mut EmitContext,
) -> TokenStream {
    let branch_steps = collect_branch_steps(start_step_id, graph);

    let step_codes: Vec<TokenStream> = branch_steps
        .iter()
        .filter_map(|step_id| graph.steps.get(step_id).map(|step| step.emit(ctx, graph)))
        .collect();

    quote! {
        #(#step_codes)*
    }
}

// ============================================================================
// Condition Expression Evaluation
// ============================================================================

/// Emit code for a ConditionExpression.
fn emit_condition_expression(
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

/// Emit code for a MappingValue (reference or immediate).
fn emit_mapping_value(
    mapping_value: &MappingValue,
    _ctx: &mut EmitContext,
    source_var: &proc_macro2::Ident,
) -> TokenStream {
    match mapping_value {
        MappingValue::Reference(r) => {
            let json_pointer = path_to_json_pointer(&r.value);
            quote! {
                #source_var.pointer(#json_pointer).cloned().unwrap_or(serde_json::Value::Null)
            }
        }
        MappingValue::Immediate(i) => {
            let tokens = super::super::json_to_tokens(&i.value);
            quote! { #tokens }
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

// ============================================================================
// Utilities
// ============================================================================

/// Convert a dot-notation path to a JSON pointer.
fn path_to_json_pointer(path: &str) -> String {
    let normalized = path
        .replace("['", ".")
        .replace("']", "")
        .replace("[\"", ".")
        .replace("\"]", "");
    let parts: Vec<&str> = normalized.split('.').collect();
    format!("/{}", parts.join("/"))
}
