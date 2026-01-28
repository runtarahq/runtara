// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Switch step emitter.
//!
//! The Switch step performs multi-way branching based on value matching.
//! Cases are expanded at compile time using the same condition expression
//! infrastructure as the Conditional step.

use proc_macro2::TokenStream;
use quote::quote;

use super::super::condition_emitters::emit_condition_expression;
use super::super::context::EmitContext;
use super::super::mapping;
use super::super::{CodegenError, json_to_tokens};
use super::branching;
use super::{emit_step_debug_end, emit_step_debug_start, find_next_step_for_label};
use runtara_dsl::{
    ConditionArgument, ConditionExpression, ConditionOperation, ConditionOperator, ExecutionGraph,
    ImmediateValue, MappingValue, SwitchCase, SwitchMatchType, SwitchStep,
};

/// Emit code for a Switch step.
///
/// Dispatches to `emit_value_switch` (non-routing) or `emit_routing_switch`
/// (when cases have `route` labels).
#[allow(clippy::too_many_lines)]
pub fn emit(
    step: &SwitchStep,
    ctx: &mut EmitContext,
    graph: &ExecutionGraph,
) -> Result<TokenStream, CodegenError> {
    let is_routing = step.config.as_ref().is_some_and(|c| c.is_routing());
    if is_routing {
        emit_routing_switch(step, ctx, graph)
    } else {
        emit_value_switch(step, ctx)
    }
}

/// Emit code for a value-only Switch step (no routing).
#[allow(clippy::too_many_lines)]
fn emit_value_switch(
    step: &SwitchStep,
    ctx: &mut EmitContext,
) -> Result<TokenStream, CodegenError> {
    let step_id = &step.id;
    let step_name = step.name.as_deref();
    let step_name_display = step_name.unwrap_or("Unnamed");

    // Do all mutable operations first
    let step_var = ctx.declare_step(step_id);
    let source_var = ctx.temp_var("source");
    let inputs_var = ctx.temp_var("switch_inputs");

    // Clone immutable references
    let steps_context = ctx.steps_context_var.clone();

    // Build the source for input mapping
    let build_source = mapping::emit_build_source(ctx);

    // Serialize config to JSON for debug events
    let config_json = step
        .config
        .as_ref()
        .and_then(|c| serde_json::to_string(c).ok());

    // Build inputs for debug events (value from mapping + static cases/default)
    let inputs_code = if let Some(ref config) = step.config {
        let value_mapping: std::collections::HashMap<String, MappingValue> =
            [("value".to_string(), config.value.clone())]
                .into_iter()
                .collect();

        let mapping_code = mapping::emit_input_mapping(&value_mapping, ctx, &source_var);

        let cases_json = serde_json::to_string(&config.cases).unwrap_or_else(|_| "[]".to_string());
        let default_json = config
            .default
            .as_ref()
            .map(|v| serde_json::to_string(v).unwrap_or_else(|_| "{}".to_string()))
            .unwrap_or_else(|| "{}".to_string());

        quote! {
            {
                let mut inputs = #mapping_code;
                if let serde_json::Value::Object(ref mut map) = inputs {
                    let cases: serde_json::Value = serde_json::from_str(#cases_json).unwrap_or(serde_json::json!([]));
                    let default: serde_json::Value = serde_json::from_str(#default_json).unwrap_or(serde_json::json!({}));
                    map.insert("cases".to_string(), cases);
                    map.insert("default".to_string(), default);
                }
                inputs
            }
        }
    } else {
        quote! { serde_json::Value::Object(serde_json::Map::new()) }
    };

    // Clone scenario inputs var for debug events
    let scenario_inputs_var = ctx.inputs_var.clone();

    // Generate debug event emissions
    let debug_start = emit_step_debug_start(
        ctx,
        step_id,
        step_name,
        "Switch",
        Some(&inputs_var),
        config_json.as_deref(),
        Some(&scenario_inputs_var),
        None,
    );
    let debug_end = emit_step_debug_end(
        ctx,
        step_id,
        step_name,
        "Switch",
        Some(&step_var),
        Some(&scenario_inputs_var),
        None,
    );

    // Compile-time case expansion: convert each case to a condition expression
    // and emit inline match blocks
    let case_blocks = if let Some(ref config) = step.config {
        config
            .cases
            .iter()
            .map(|case| {
                let condition = switch_case_to_condition(&config.value, case);
                let match_code = emit_condition_expression(&condition, ctx, &source_var);
                let output_tokens = json_to_tokens(&case.output);

                quote! {
                    if matched_output.is_none() {
                        let __sw_matches = #match_code;
                        if __sw_matches {
                            matched_output = Some(process_switch_output(&#output_tokens, &#source_var));
                        }
                    }
                }
            })
            .collect::<Vec<_>>()
    } else {
        vec![]
    };

    // Default output
    let default_tokens = step
        .config
        .as_ref()
        .and_then(|c| c.default.as_ref())
        .map(json_to_tokens)
        .unwrap_or_else(|| {
            quote! { serde_json::Value::Object(serde_json::Map::new()) }
        });

    Ok(quote! {
        let #source_var = #build_source;
        let #inputs_var = #inputs_code;

        #debug_start

        // Compile-time expanded case matching
        let mut matched_output: Option<serde_json::Value> = None;

        #(#case_blocks)*

        let output = matched_output.unwrap_or_else(|| process_switch_output(&#default_tokens, &#source_var));

        let #step_var = serde_json::json!({
            "stepId": #step_id,
            "stepName": #step_name_display,
            "stepType": "Switch",
            "outputs": output
        });

        #debug_end

        #steps_context.insert(#step_id.to_string(), #step_var.clone());
    })
}

/// Emit code for a routing Switch step.
///
/// Cases match a value (like value switch) but also carry a `route` label.
/// The matched route determines which execution branch to follow, similar
/// to how Conditional uses "true"/"false" edge labels.
#[allow(clippy::too_many_lines)]
fn emit_routing_switch(
    step: &SwitchStep,
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
    let inputs_var = ctx.temp_var("switch_inputs");

    // Clone immutable references
    let steps_context = ctx.steps_context_var.clone();

    // Build the source for input mapping
    let build_source = mapping::emit_build_source(ctx);

    // Serialize config to JSON for debug events
    let config_json = step
        .config
        .as_ref()
        .and_then(|c| serde_json::to_string(c).ok());

    // Build inputs for debug events
    let inputs_code = if let Some(ref config) = step.config {
        let value_mapping: std::collections::HashMap<String, MappingValue> =
            [("value".to_string(), config.value.clone())]
                .into_iter()
                .collect();

        let mapping_code = mapping::emit_input_mapping(&value_mapping, ctx, &source_var);

        let cases_json = serde_json::to_string(&config.cases).unwrap_or_else(|_| "[]".to_string());
        let default_json = config
            .default
            .as_ref()
            .map(|v| serde_json::to_string(v).unwrap_or_else(|_| "{}".to_string()))
            .unwrap_or_else(|| "{}".to_string());

        quote! {
            {
                let mut inputs = #mapping_code;
                if let serde_json::Value::Object(ref mut map) = inputs {
                    let cases: serde_json::Value = serde_json::from_str(#cases_json).unwrap_or(serde_json::json!([]));
                    let default: serde_json::Value = serde_json::from_str(#default_json).unwrap_or(serde_json::json!({}));
                    map.insert("cases".to_string(), cases);
                    map.insert("default".to_string(), default);
                }
                inputs
            }
        }
    } else {
        quote! { serde_json::Value::Object(serde_json::Map::new()) }
    };

    // Clone scenario inputs var for debug events
    let scenario_inputs_var = ctx.inputs_var.clone();

    // Generate debug event emissions
    let debug_start = emit_step_debug_start(
        ctx,
        step_id,
        step_name,
        "Switch",
        Some(&inputs_var),
        config_json.as_deref(),
        Some(&scenario_inputs_var),
        None,
    );
    let debug_end = emit_step_debug_end(
        ctx,
        step_id,
        step_name,
        "Switch",
        Some(&step_var),
        Some(&scenario_inputs_var),
        None,
    );

    // Compile-time case expansion with route tracking
    let case_blocks = if let Some(ref config) = step.config {
        config
            .cases
            .iter()
            .map(|case| {
                let condition = switch_case_to_condition(&config.value, case);
                let match_code = emit_condition_expression(&condition, ctx, &source_var);
                let output_tokens = json_to_tokens(&case.output);
                let route_label = case.route.as_deref().unwrap_or("default");

                quote! {
                    if matched_route.is_none() {
                        let __sw_matches = #match_code;
                        if __sw_matches {
                            matched_output = Some(process_switch_output(&#output_tokens, &#source_var));
                            matched_route = Some(#route_label);
                        }
                    }
                }
            })
            .collect::<Vec<_>>()
    } else {
        vec![]
    };

    // Default output
    let default_tokens = step
        .config
        .as_ref()
        .and_then(|c| c.default.as_ref())
        .map(json_to_tokens)
        .unwrap_or_else(|| {
            quote! { serde_json::Value::Object(serde_json::Map::new()) }
        });

    // Collect route labels from cases (unique, sorted)
    let route_labels: Vec<String> = step
        .config
        .as_ref()
        .map(|c| {
            c.route_labels()
                .into_iter()
                .map(|s| s.to_string())
                .collect()
        })
        .unwrap_or_default();

    // Find branch start steps for each route label + "default"
    let mut branch_starts: Vec<Option<String>> = Vec::new();
    let mut label_start_pairs: Vec<(String, Option<String>)> = Vec::new();

    for label in &route_labels {
        let start = find_next_step_for_label(step_id, label, execution_plan).map(|s| s.to_string());
        branch_starts.push(start.clone());
        label_start_pairs.push((label.clone(), start));
    }

    // Default branch
    let default_start =
        find_next_step_for_label(step_id, "default", execution_plan).map(|s| s.to_string());
    branch_starts.push(default_start.clone());

    // Find merge point where all branches converge
    let merge_point = branching::find_merge_point_n(&branch_starts, graph);

    // Emit branch code for each route label
    let mut route_branch_codes: Vec<TokenStream> = Vec::new();
    for (label, start) in &label_start_pairs {
        let branch_code = if let Some(start_step_id) = start {
            branching::emit_branch_code(start_step_id, graph, ctx, merge_point.as_deref())?
        } else {
            quote! {}
        };
        let label_str = label.as_str();
        route_branch_codes.push(quote! {
            if __route == #label_str {
                #branch_code
            }
        });
    }

    // Default branch code
    let default_branch_code = if let Some(start_step_id) = &default_start {
        branching::emit_branch_code(start_step_id, graph, ctx, merge_point.as_deref())?
    } else {
        quote! {}
    };

    // Common suffix after merge point
    let common_suffix_code = if let Some(ref merge_step_id) = merge_point {
        branching::emit_branch_code(merge_step_id, graph, ctx, None)?
    } else {
        quote! {}
    };

    // Build the route dispatch: if/else-if chain for route labels, else for default
    let route_dispatch = if route_branch_codes.is_empty() {
        // No named routes — just default
        quote! { #default_branch_code }
    } else {
        // Build if/else-if chain with trailing else for default
        quote! {
            #(#route_branch_codes else)* {
                #default_branch_code
            }
        }
    };

    Ok(quote! {
        let #source_var = #build_source;
        let #inputs_var = #inputs_code;

        #debug_start

        // Compile-time expanded case matching with route tracking
        let mut matched_output: Option<serde_json::Value> = None;
        let mut matched_route: Option<&str> = None;

        #(#case_blocks)*

        let output = matched_output.unwrap_or_else(|| process_switch_output(&#default_tokens, &#source_var));
        let __route: &str = matched_route.unwrap_or("default");

        let #step_var = serde_json::json!({
            "stepId": #step_id,
            "stepName": #step_name_display,
            "stepType": "Switch",
            "outputs": output,
            "route": __route
        });

        #debug_end

        #steps_context.insert(#step_id.to_string(), #step_var.clone());

        // Route dispatch
        #route_dispatch

        // Common suffix after merge point
        #common_suffix_code
    })
}

/// Convert a `SwitchCase` into a `ConditionExpression` that the shared
/// condition emitters can evaluate.
///
/// The switch value (`config.value`) becomes the left operand and
/// `case.match_value` becomes the right operand (as an immediate value).
/// Compound types (BETWEEN, RANGE) are decomposed into AND-combined
/// sub-conditions.
fn switch_case_to_condition(switch_value: &MappingValue, case: &SwitchCase) -> ConditionExpression {
    let left = ConditionArgument::Value(switch_value.clone());
    let right = ConditionArgument::Value(MappingValue::Immediate(ImmediateValue {
        value: case.match_value.clone(),
    }));

    match case.match_type {
        // When EQ is used with an array match value, treat as IN (value in array)
        SwitchMatchType::Eq if case.match_value.is_array() => {
            binary_op(ConditionOperator::In, left, right)
        }
        SwitchMatchType::Eq => binary_op(ConditionOperator::Eq, left, right),
        SwitchMatchType::Ne => binary_op(ConditionOperator::Ne, left, right),
        SwitchMatchType::Gt => binary_op(ConditionOperator::Gt, left, right),
        SwitchMatchType::Gte => binary_op(ConditionOperator::Gte, left, right),
        SwitchMatchType::Lt => binary_op(ConditionOperator::Lt, left, right),
        SwitchMatchType::Lte => binary_op(ConditionOperator::Lte, left, right),
        SwitchMatchType::StartsWith => binary_op(ConditionOperator::StartsWith, left, right),
        SwitchMatchType::EndsWith => binary_op(ConditionOperator::EndsWith, left, right),
        SwitchMatchType::Contains => binary_op(ConditionOperator::Contains, left, right),
        SwitchMatchType::In => binary_op(ConditionOperator::In, left, right),
        SwitchMatchType::NotIn => binary_op(ConditionOperator::NotIn, left, right),

        // Unary operators — only the switch value as argument
        SwitchMatchType::IsDefined => unary_op(ConditionOperator::IsDefined, left),
        SwitchMatchType::IsEmpty => unary_op(ConditionOperator::IsEmpty, left),
        SwitchMatchType::IsNotEmpty => unary_op(ConditionOperator::IsNotEmpty, left),

        // BETWEEN([min, max]) → AND(GTE(value, min), LTE(value, max))
        SwitchMatchType::Between => build_between(switch_value, &case.match_value),

        // RANGE({gte?, gt?, lte?, lt?}) → AND(bound checks...)
        SwitchMatchType::Range => build_range(switch_value, &case.match_value),
    }
}

/// Helper: binary operation expression.
fn binary_op(
    op: ConditionOperator,
    left: ConditionArgument,
    right: ConditionArgument,
) -> ConditionExpression {
    ConditionExpression::Operation(ConditionOperation {
        op,
        arguments: vec![left, right],
    })
}

/// Helper: unary operation expression.
fn unary_op(op: ConditionOperator, arg: ConditionArgument) -> ConditionExpression {
    ConditionExpression::Operation(ConditionOperation {
        op,
        arguments: vec![arg],
    })
}

/// Build a BETWEEN condition: AND(GTE(value, min), LTE(value, max)).
fn build_between(
    switch_value: &MappingValue,
    match_value: &serde_json::Value,
) -> ConditionExpression {
    let arr = match_value.as_array();
    if let Some(arr) = arr.filter(|a| a.len() >= 2) {
        let left = ConditionArgument::Value(switch_value.clone());
        let min_arg = ConditionArgument::Value(MappingValue::Immediate(ImmediateValue {
            value: arr[0].clone(),
        }));
        let max_arg = ConditionArgument::Value(MappingValue::Immediate(ImmediateValue {
            value: arr[1].clone(),
        }));

        ConditionExpression::Operation(ConditionOperation {
            op: ConditionOperator::And,
            arguments: vec![
                ConditionArgument::Expression(Box::new(ConditionExpression::Operation(
                    ConditionOperation {
                        op: ConditionOperator::Gte,
                        arguments: vec![left.clone(), min_arg],
                    },
                ))),
                ConditionArgument::Expression(Box::new(ConditionExpression::Operation(
                    ConditionOperation {
                        op: ConditionOperator::Lte,
                        arguments: vec![left, max_arg],
                    },
                ))),
            ],
        })
    } else {
        // Invalid BETWEEN format → always false
        ConditionExpression::Value(MappingValue::Immediate(ImmediateValue {
            value: serde_json::json!(false),
        }))
    }
}

/// Build a RANGE condition: AND(bound checks...) from an object with
/// optional `gte`, `gt`, `lte`, `lt` fields.
fn build_range(
    switch_value: &MappingValue,
    match_value: &serde_json::Value,
) -> ConditionExpression {
    let mut bound_conditions = Vec::new();

    if let Some(obj) = match_value.as_object() {
        for (key, value) in obj {
            let op = match key.as_str() {
                "gte" => Some(ConditionOperator::Gte),
                "gt" => Some(ConditionOperator::Gt),
                "lte" => Some(ConditionOperator::Lte),
                "lt" => Some(ConditionOperator::Lt),
                _ => None,
            };

            if let Some(op) = op {
                let left = ConditionArgument::Value(switch_value.clone());
                let bound_arg = ConditionArgument::Value(MappingValue::Immediate(ImmediateValue {
                    value: value.clone(),
                }));
                bound_conditions.push(ConditionArgument::Expression(Box::new(
                    ConditionExpression::Operation(ConditionOperation {
                        op,
                        arguments: vec![left, bound_arg],
                    }),
                )));
            }
        }
    }

    if bound_conditions.is_empty() {
        // No valid bounds → vacuously true
        ConditionExpression::Value(MappingValue::Immediate(ImmediateValue {
            value: serde_json::json!(true),
        }))
    } else if bound_conditions.len() == 1 {
        // Single bound — unwrap from vec
        match bound_conditions.into_iter().next().unwrap() {
            ConditionArgument::Expression(expr) => *expr,
            other => unary_op(ConditionOperator::IsDefined, other),
        }
    } else {
        ConditionExpression::Operation(ConditionOperation {
            op: ConditionOperator::And,
            arguments: bound_conditions,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codegen::ast::context::EmitContext;
    use runtara_dsl::{ExecutionGraph, FinishStep, ReferenceValue, Step, SwitchConfig};
    use std::collections::HashMap;

    /// Minimal graph for tests that don't exercise branching.
    fn empty_graph() -> ExecutionGraph {
        let mut steps = HashMap::new();
        steps.insert(
            "finish".to_string(),
            Step::Finish(FinishStep {
                id: "finish".to_string(),
                name: None,
                input_mapping: None,
            }),
        );
        ExecutionGraph {
            name: None,
            description: None,
            entry_point: "finish".to_string(),
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

    /// Helper to create a minimal switch step for testing.
    fn create_switch_step(step_id: &str, value_ref: &str, cases: Vec<SwitchCase>) -> SwitchStep {
        SwitchStep {
            id: step_id.to_string(),
            name: Some("Test Switch".to_string()),
            config: Some(SwitchConfig {
                value: MappingValue::Reference(ReferenceValue {
                    value: value_ref.to_string(),
                    type_hint: None,
                    default: None,
                }),
                cases,
                default: Some(serde_json::json!({"result": "default"})),
            }),
        }
    }

    /// Helper to create a switch case.
    fn create_case(
        match_type: SwitchMatchType,
        match_value: serde_json::Value,
        output: serde_json::Value,
    ) -> SwitchCase {
        SwitchCase {
            match_type,
            match_value,
            output,
            route: None,
        }
    }

    #[test]
    fn test_emit_switch_basic_structure() {
        let mut ctx = EmitContext::new(false);
        let cases = vec![create_case(
            SwitchMatchType::Eq,
            serde_json::json!("test"),
            serde_json::json!({"matched": true}),
        )];
        let step = create_switch_step("switch-basic", "steps.previous.output", cases);

        let tokens = emit(&step, &mut ctx, &empty_graph()).unwrap();
        let code = tokens.to_string();

        assert!(
            code.contains("matched_output"),
            "Should track matched output"
        );
        assert!(
            code.contains("process_switch_output"),
            "Should process switch output"
        );
    }

    #[test]
    fn test_emit_switch_uses_values_equal() {
        let mut ctx = EmitContext::new(false);
        let cases = vec![create_case(
            SwitchMatchType::Eq,
            serde_json::json!("test"),
            serde_json::json!({"matched": true}),
        )];
        let step = create_switch_step("switch-eq", "value", cases);

        let tokens = emit(&step, &mut ctx, &empty_graph()).unwrap();
        let code = tokens.to_string();

        // EQ uses values_equal from condition emitters
        assert!(
            code.contains("values_equal"),
            "EQ should use values_equal helper"
        );
    }

    #[test]
    fn test_emit_switch_uses_to_number_for_comparison() {
        let mut ctx = EmitContext::new(false);
        let cases = vec![create_case(
            SwitchMatchType::Gt,
            serde_json::json!(10),
            serde_json::json!({"result": "large"}),
        )];
        let step = create_switch_step("switch-gt", "value", cases);

        let tokens = emit(&step, &mut ctx, &empty_graph()).unwrap();
        let code = tokens.to_string();

        // GT uses to_number from condition emitters
        assert!(code.contains("to_number"), "GT should use to_number helper");
    }

    #[test]
    fn test_emit_switch_eq_with_array_converts_to_in() {
        let mut ctx = EmitContext::new(false);
        let cases = vec![create_case(
            SwitchMatchType::Eq,
            serde_json::json!(["a", "b", "c"]),
            serde_json::json!({"matched": true}),
        )];
        let step = create_switch_step("switch-eq-array", "value", cases);

        let tokens = emit(&step, &mut ctx, &empty_graph()).unwrap();
        let code = tokens.to_string();

        // EQ with array match → IN semantics (uses values_equal in a loop)
        assert!(
            code.contains("values_equal"),
            "EQ with array should use IN semantics via values_equal"
        );
    }

    #[test]
    fn test_emit_switch_string_operators() {
        let mut ctx = EmitContext::new(false);
        let cases = vec![
            create_case(
                SwitchMatchType::StartsWith,
                serde_json::json!("pre"),
                serde_json::json!({"matched": "starts"}),
            ),
            create_case(
                SwitchMatchType::EndsWith,
                serde_json::json!("suf"),
                serde_json::json!({"matched": "ends"}),
            ),
        ];
        let step = create_switch_step("switch-string", "value", cases);

        let tokens = emit(&step, &mut ctx, &empty_graph()).unwrap();
        let code = tokens.to_string();

        assert!(
            code.contains("starts_with"),
            "Should use starts_with method"
        );
        assert!(code.contains("ends_with"), "Should use ends_with method");
    }

    #[test]
    fn test_emit_switch_utility_operators() {
        let mut ctx = EmitContext::new(false);
        let cases = vec![
            create_case(
                SwitchMatchType::IsDefined,
                serde_json::json!(null),
                serde_json::json!({"defined": true}),
            ),
            create_case(
                SwitchMatchType::IsEmpty,
                serde_json::json!(null),
                serde_json::json!({"empty": true}),
            ),
            create_case(
                SwitchMatchType::IsNotEmpty,
                serde_json::json!(null),
                serde_json::json!({"not_empty": true}),
            ),
        ];
        let step = create_switch_step("switch-utility", "value", cases);

        let tokens = emit(&step, &mut ctx, &empty_graph()).unwrap();
        let code = tokens.to_string();

        assert!(code.contains("is_null"), "IsDefined should check for null");
        assert!(code.contains("is_empty"), "IsEmpty should check for empty");
    }

    #[test]
    fn test_emit_switch_between_decomposition() {
        let mut ctx = EmitContext::new(false);
        let cases = vec![create_case(
            SwitchMatchType::Between,
            serde_json::json!([10, 20]),
            serde_json::json!({"in_range": true}),
        )];
        let step = create_switch_step("switch-between", "value", cases);

        let tokens = emit(&step, &mut ctx, &empty_graph()).unwrap();
        let code = tokens.to_string();

        // BETWEEN decomposes to AND(GTE, LTE) — uses to_number twice
        assert!(
            code.contains("to_number"),
            "BETWEEN should use to_number for bounds"
        );
    }

    #[test]
    fn test_emit_switch_range_decomposition() {
        let mut ctx = EmitContext::new(false);
        let cases = vec![create_case(
            SwitchMatchType::Range,
            serde_json::json!({"gte": 5, "lt": 15}),
            serde_json::json!({"in_range": true}),
        )];
        let step = create_switch_step("switch-range", "value", cases);

        let tokens = emit(&step, &mut ctx, &empty_graph()).unwrap();
        let code = tokens.to_string();

        assert!(
            code.contains("to_number"),
            "RANGE should use to_number for bounds"
        );
    }

    #[test]
    fn test_emit_switch_default_fallback() {
        let mut ctx = EmitContext::new(false);
        let step = create_switch_step("switch-default", "value", vec![]);

        let tokens = emit(&step, &mut ctx, &empty_graph()).unwrap();
        let code = tokens.to_string();

        assert!(
            code.contains("unwrap_or_else"),
            "Should use default when no match"
        );
        assert!(
            code.contains("process_switch_output"),
            "Should process switch output"
        );
    }

    #[test]
    fn test_emit_switch_output_structure() {
        let mut ctx = EmitContext::new(false);
        let step = create_switch_step("switch-output", "value", vec![]);

        let tokens = emit(&step, &mut ctx, &empty_graph()).unwrap();
        let code = tokens.to_string();

        assert!(code.contains("\"stepId\""), "Should include stepId");
        assert!(code.contains("\"stepName\""), "Should include stepName");
        assert!(code.contains("\"stepType\""), "Should include stepType");
        assert!(code.contains("\"Switch\""), "Should have stepType = Switch");
        assert!(code.contains("\"outputs\""), "Should include outputs");
    }

    #[test]
    fn test_emit_switch_stores_in_steps_context() {
        let mut ctx = EmitContext::new(false);
        let step = create_switch_step("switch-store", "value", vec![]);

        let tokens = emit(&step, &mut ctx, &empty_graph()).unwrap();
        let code = tokens.to_string();

        assert!(
            code.contains("steps_context . insert"),
            "Should store result in steps_context"
        );
        assert!(
            code.contains("\"switch-store\""),
            "Should use step_id as key"
        );
    }

    #[test]
    fn test_emit_switch_debug_mode_enabled() {
        let mut ctx = EmitContext::new(true);
        let step = create_switch_step("switch-debug", "value", vec![]);

        let tokens = emit(&step, &mut ctx, &empty_graph()).unwrap();
        let code = tokens.to_string();

        assert!(
            code.contains("step_debug_start"),
            "Should emit debug start event"
        );
        assert!(
            code.contains("step_debug_end"),
            "Should emit debug end event"
        );
    }

    #[test]
    fn test_emit_switch_debug_mode_disabled() {
        let mut ctx = EmitContext::new(false);
        let step = create_switch_step("switch-no-debug", "value", vec![]);

        let tokens = emit(&step, &mut ctx, &empty_graph()).unwrap();
        let code = tokens.to_string();

        assert!(code.contains("matched_output"), "Should track matching");
    }

    #[test]
    fn test_emit_switch_with_unnamed_step() {
        let mut ctx = EmitContext::new(false);
        let step = SwitchStep {
            id: "switch-unnamed".to_string(),
            name: None,
            config: Some(SwitchConfig {
                value: MappingValue::Immediate(ImmediateValue {
                    value: serde_json::json!("test"),
                }),
                cases: vec![],
                default: None,
            }),
        };

        let tokens = emit(&step, &mut ctx, &empty_graph()).unwrap();
        let code = tokens.to_string();

        assert!(
            code.contains("\"Unnamed\""),
            "Should use 'Unnamed' for unnamed steps"
        );
    }

    #[test]
    fn test_emit_switch_no_config() {
        let mut ctx = EmitContext::new(false);
        let step = SwitchStep {
            id: "switch-no-config".to_string(),
            name: Some("Empty Switch".to_string()),
            config: None,
        };

        let tokens = emit(&step, &mut ctx, &empty_graph()).unwrap();
        let code = tokens.to_string();

        assert!(
            code.contains("serde_json :: Value :: Object (serde_json :: Map :: new ())"),
            "Should create empty object when no config"
        );
    }

    #[test]
    fn test_emit_switch_immediate_value() {
        let mut ctx = EmitContext::new(false);
        let step = SwitchStep {
            id: "switch-immediate".to_string(),
            name: Some("Immediate Switch".to_string()),
            config: Some(SwitchConfig {
                value: MappingValue::Immediate(ImmediateValue {
                    value: serde_json::json!(42),
                }),
                cases: vec![create_case(
                    SwitchMatchType::Eq,
                    serde_json::json!(42),
                    serde_json::json!({"matched": true}),
                )],
                default: None,
            }),
        };

        let tokens = emit(&step, &mut ctx, &empty_graph()).unwrap();
        let code = tokens.to_string();

        assert!(
            code.contains("values_equal"),
            "Should use values_equal for immediate EQ"
        );
    }

    #[test]
    fn test_emit_switch_multiple_cases_expanded() {
        let mut ctx = EmitContext::new(false);
        let cases = vec![
            create_case(
                SwitchMatchType::Eq,
                serde_json::json!("pending"),
                serde_json::json!({"status": "waiting"}),
            ),
            create_case(
                SwitchMatchType::Eq,
                serde_json::json!("complete"),
                serde_json::json!({"status": "done"}),
            ),
        ];
        let step = create_switch_step("switch-multi", "data.status", cases);

        let tokens = emit(&step, &mut ctx, &empty_graph()).unwrap();
        let code = tokens.to_string();

        // Each case becomes a separate if-block checking matched_output.is_none()
        let none_count = code.matches("matched_output . is_none ()").count();
        assert!(
            none_count >= 2,
            "Should have separate is_none() checks for each case, found {}",
            none_count,
        );
    }

    // ── switch_case_to_condition unit tests ─────────────────────────

    #[test]
    fn test_case_to_condition_eq() {
        let value = MappingValue::Immediate(ImmediateValue {
            value: serde_json::json!("test"),
        });
        let case = create_case(
            SwitchMatchType::Eq,
            serde_json::json!("hello"),
            serde_json::json!({}),
        );
        let expr = switch_case_to_condition(&value, &case);

        if let ConditionExpression::Operation(op) = expr {
            assert_eq!(op.op, ConditionOperator::Eq);
            assert_eq!(op.arguments.len(), 2);
        } else {
            panic!("Expected Operation");
        }
    }

    #[test]
    fn test_case_to_condition_eq_array_becomes_in() {
        let value = MappingValue::Immediate(ImmediateValue {
            value: serde_json::json!("test"),
        });
        let case = create_case(
            SwitchMatchType::Eq,
            serde_json::json!(["a", "b"]),
            serde_json::json!({}),
        );
        let expr = switch_case_to_condition(&value, &case);

        if let ConditionExpression::Operation(op) = expr {
            assert_eq!(
                op.op,
                ConditionOperator::In,
                "EQ with array should become IN"
            );
        } else {
            panic!("Expected Operation");
        }
    }

    #[test]
    fn test_case_to_condition_between() {
        let value = MappingValue::Immediate(ImmediateValue {
            value: serde_json::json!(15),
        });
        let case = create_case(
            SwitchMatchType::Between,
            serde_json::json!([10, 20]),
            serde_json::json!({}),
        );
        let expr = switch_case_to_condition(&value, &case);

        if let ConditionExpression::Operation(op) = expr {
            assert_eq!(op.op, ConditionOperator::And, "BETWEEN should become AND");
            assert_eq!(
                op.arguments.len(),
                2,
                "BETWEEN AND should have 2 sub-conditions"
            );
        } else {
            panic!("Expected Operation");
        }
    }

    #[test]
    fn test_case_to_condition_range() {
        let value = MappingValue::Immediate(ImmediateValue {
            value: serde_json::json!(15),
        });
        let case = create_case(
            SwitchMatchType::Range,
            serde_json::json!({"gte": 10, "lt": 20}),
            serde_json::json!({}),
        );
        let expr = switch_case_to_condition(&value, &case);

        if let ConditionExpression::Operation(op) = expr {
            assert_eq!(
                op.op,
                ConditionOperator::And,
                "RANGE with 2 bounds should become AND"
            );
            assert_eq!(op.arguments.len(), 2);
        } else {
            panic!("Expected Operation");
        }
    }

    #[test]
    fn test_case_to_condition_unary() {
        let value = MappingValue::Immediate(ImmediateValue {
            value: serde_json::json!("test"),
        });
        let case = create_case(
            SwitchMatchType::IsDefined,
            serde_json::json!(null),
            serde_json::json!({}),
        );
        let expr = switch_case_to_condition(&value, &case);

        if let ConditionExpression::Operation(op) = expr {
            assert_eq!(op.op, ConditionOperator::IsDefined);
            assert_eq!(
                op.arguments.len(),
                1,
                "Unary operator should have 1 argument"
            );
        } else {
            panic!("Expected Operation");
        }
    }

    // ── routing switch tests ───────────────────────────────────────

    use runtara_dsl::{ExecutionPlanEdge, LogLevel, LogStep};

    fn edge(from: &str, to: &str, label: Option<&str>) -> ExecutionPlanEdge {
        ExecutionPlanEdge {
            from_step: from.to_string(),
            to_step: to.to_string(),
            label: label.map(|s| s.to_string()),
            condition: None,
            priority: None,
        }
    }

    fn make_log_step(id: &str) -> Step {
        Step::Log(LogStep {
            id: id.to_string(),
            name: Some(format!("Log {}", id)),
            message: "test".to_string(),
            level: LogLevel::Info,
            context: None,
        })
    }

    fn make_finish_step(id: &str) -> Step {
        Step::Finish(FinishStep {
            id: id.to_string(),
            name: Some(format!("Finish {}", id)),
            input_mapping: None,
        })
    }

    fn make_routing_graph() -> ExecutionGraph {
        //    switch
        //   /  |   \
        //  s1  s2  s3   (routes: pending, active, default)
        //   \  |   /
        //    merge
        //      |
        //    finish
        let mut steps = HashMap::new();
        steps.insert(
            "sw".to_string(),
            Step::Switch(SwitchStep {
                id: "sw".to_string(),
                name: Some("Route Switch".to_string()),
                config: Some(SwitchConfig {
                    value: MappingValue::Reference(ReferenceValue {
                        value: "steps.prev.outputs.status".to_string(),
                        type_hint: None,
                        default: None,
                    }),
                    cases: vec![
                        SwitchCase {
                            match_type: SwitchMatchType::Eq,
                            match_value: serde_json::json!("pending"),
                            output: serde_json::json!({"s": "waiting"}),
                            route: Some("pending".to_string()),
                        },
                        SwitchCase {
                            match_type: SwitchMatchType::Eq,
                            match_value: serde_json::json!("active"),
                            output: serde_json::json!({"s": "active"}),
                            route: Some("active".to_string()),
                        },
                    ],
                    default: Some(serde_json::json!({"s": "unknown"})),
                }),
            }),
        );
        steps.insert("s1".to_string(), make_log_step("s1"));
        steps.insert("s2".to_string(), make_log_step("s2"));
        steps.insert("s3".to_string(), make_log_step("s3"));
        steps.insert("merge".to_string(), make_log_step("merge"));
        steps.insert("finish".to_string(), make_finish_step("finish"));

        ExecutionGraph {
            name: None,
            description: None,
            entry_point: "sw".to_string(),
            steps,
            execution_plan: vec![
                edge("sw", "s1", Some("pending")),
                edge("sw", "s2", Some("active")),
                edge("sw", "s3", Some("default")),
                edge("s1", "merge", None),
                edge("s2", "merge", None),
                edge("s3", "merge", None),
                edge("merge", "finish", None),
            ],
            variables: HashMap::new(),
            input_schema: HashMap::new(),
            output_schema: HashMap::new(),
            notes: None,
            nodes: None,
            edges: None,
        }
    }

    #[test]
    fn test_routing_switch_tracks_matched_route() {
        let mut ctx = EmitContext::new(false);
        let graph = make_routing_graph();
        let step = graph.steps.get("sw").unwrap();

        if let Step::Switch(sw) = step {
            let tokens = emit(sw, &mut ctx, &graph).unwrap();
            let code = tokens.to_string();

            assert!(
                code.contains("matched_route"),
                "Routing switch should track matched_route"
            );
            assert!(
                code.contains("\"pending\""),
                "Should include pending route label"
            );
            assert!(
                code.contains("\"active\""),
                "Should include active route label"
            );
            assert!(
                code.contains("\"default\""),
                "Should include default route fallback"
            );
        } else {
            panic!("Expected Switch step");
        }
    }

    #[test]
    fn test_routing_switch_includes_route_in_output() {
        let mut ctx = EmitContext::new(false);
        let graph = make_routing_graph();
        let step = graph.steps.get("sw").unwrap();

        if let Step::Switch(sw) = step {
            let tokens = emit(sw, &mut ctx, &graph).unwrap();
            let code = tokens.to_string();

            assert!(
                code.contains("\"route\""),
                "Routing switch output should include route field"
            );
            assert!(code.contains("__route"), "Should use __route variable");
        } else {
            panic!("Expected Switch step");
        }
    }

    #[test]
    fn test_routing_switch_emits_route_dispatch() {
        let mut ctx = EmitContext::new(false);
        let graph = make_routing_graph();
        let step = graph.steps.get("sw").unwrap();

        if let Step::Switch(sw) = step {
            let tokens = emit(sw, &mut ctx, &graph).unwrap();
            let code = tokens.to_string();

            // Should have if/else dispatch on __route
            assert!(
                code.contains("__route =="),
                "Should dispatch on __route variable"
            );
        } else {
            panic!("Expected Switch step");
        }
    }

    #[test]
    fn test_routing_switch_emits_branch_steps() {
        let mut ctx = EmitContext::new(false);
        let graph = make_routing_graph();
        let step = graph.steps.get("sw").unwrap();

        if let Step::Switch(sw) = step {
            let tokens = emit(sw, &mut ctx, &graph).unwrap();
            let code = tokens.to_string();

            // Branch steps (s1, s2, s3) should appear in the generated code
            assert!(code.contains("\"s1\""), "Should include branch step s1");
            assert!(code.contains("\"s2\""), "Should include branch step s2");
            assert!(code.contains("\"s3\""), "Should include branch step s3");
        } else {
            panic!("Expected Switch step");
        }
    }

    #[test]
    fn test_routing_switch_emits_merge_point_code() {
        let mut ctx = EmitContext::new(false);
        let graph = make_routing_graph();
        let step = graph.steps.get("sw").unwrap();

        if let Step::Switch(sw) = step {
            let tokens = emit(sw, &mut ctx, &graph).unwrap();
            let code = tokens.to_string();

            // Merge step should appear after the route dispatch
            assert!(
                code.contains("\"merge\""),
                "Should include merge step after route dispatch"
            );
            // Finish step should appear in the common suffix
            assert!(
                code.contains("\"finish\""),
                "Should include finish step in common suffix"
            );
        } else {
            panic!("Expected Switch step");
        }
    }

    #[test]
    fn test_non_routing_switch_does_not_include_route() {
        let mut ctx = EmitContext::new(false);
        let step = create_switch_step(
            "sw-noroute",
            "value",
            vec![create_case(
                SwitchMatchType::Eq,
                serde_json::json!("a"),
                serde_json::json!({}),
            )],
        );

        let tokens = emit(&step, &mut ctx, &empty_graph()).unwrap();
        let code = tokens.to_string();

        assert!(
            !code.contains("matched_route"),
            "Non-routing switch should NOT track matched_route"
        );
        assert!(
            !code.contains("__route"),
            "Non-routing switch should NOT use __route variable"
        );
    }

    #[test]
    fn test_routing_switch_divergent_branches_no_merge() {
        //    switch
        //   /      \
        //  s1       s2      (routes: a, default)
        //  |        |
        //  finish1  finish2   — no merge point
        let mut steps = HashMap::new();
        steps.insert(
            "sw".to_string(),
            Step::Switch(SwitchStep {
                id: "sw".to_string(),
                name: Some("Divergent Switch".to_string()),
                config: Some(SwitchConfig {
                    value: MappingValue::Immediate(ImmediateValue {
                        value: serde_json::json!("x"),
                    }),
                    cases: vec![SwitchCase {
                        match_type: SwitchMatchType::Eq,
                        match_value: serde_json::json!("a"),
                        output: serde_json::json!({"v": 1}),
                        route: Some("a".to_string()),
                    }],
                    default: Some(serde_json::json!({"v": 0})),
                }),
            }),
        );
        steps.insert("s1".to_string(), make_log_step("s1"));
        steps.insert("s2".to_string(), make_log_step("s2"));
        steps.insert("finish1".to_string(), make_finish_step("finish1"));
        steps.insert("finish2".to_string(), make_finish_step("finish2"));

        let graph = ExecutionGraph {
            name: None,
            description: None,
            entry_point: "sw".to_string(),
            steps,
            execution_plan: vec![
                edge("sw", "s1", Some("a")),
                edge("sw", "s2", Some("default")),
                edge("s1", "finish1", None),
                edge("s2", "finish2", None),
            ],
            variables: HashMap::new(),
            input_schema: HashMap::new(),
            output_schema: HashMap::new(),
            notes: None,
            nodes: None,
            edges: None,
        };

        let mut ctx = EmitContext::new(false);
        let step = graph.steps.get("sw").unwrap();
        if let Step::Switch(sw) = step {
            let tokens = emit(sw, &mut ctx, &graph).unwrap();
            let code = tokens.to_string();

            // Both branches should be emitted
            assert!(code.contains("\"s1\""), "Should include branch step s1");
            assert!(code.contains("\"s2\""), "Should include branch step s2");
            assert!(
                code.contains("\"finish1\""),
                "Should include finish1 in branch a"
            );
            assert!(
                code.contains("\"finish2\""),
                "Should include finish2 in default branch"
            );
        } else {
            panic!("Expected Switch step");
        }
    }

    #[test]
    fn test_routing_switch_debug_mode() {
        let mut ctx = EmitContext::new(true);
        let graph = make_routing_graph();
        let step = graph.steps.get("sw").unwrap();

        if let Step::Switch(sw) = step {
            let tokens = emit(sw, &mut ctx, &graph).unwrap();
            let code = tokens.to_string();

            assert!(
                code.contains("step_debug_start"),
                "Routing switch should emit debug start event"
            );
            assert!(
                code.contains("step_debug_end"),
                "Routing switch should emit debug end event"
            );
            // Route tracking should still work in debug mode
            assert!(
                code.contains("matched_route"),
                "Debug mode should not suppress route tracking"
            );
        } else {
            panic!("Expected Switch step");
        }
    }

    #[test]
    fn test_routing_switch_single_route_plus_default() {
        //    switch
        //   /      \
        //  s1       s2   (routes: only, default)
        //   \      /
        //    merge
        //      |
        //    finish
        let mut steps = HashMap::new();
        steps.insert(
            "sw".to_string(),
            Step::Switch(SwitchStep {
                id: "sw".to_string(),
                name: Some("Single Route".to_string()),
                config: Some(SwitchConfig {
                    value: MappingValue::Immediate(ImmediateValue {
                        value: serde_json::json!("x"),
                    }),
                    cases: vec![SwitchCase {
                        match_type: SwitchMatchType::Eq,
                        match_value: serde_json::json!("yes"),
                        output: serde_json::json!({"hit": true}),
                        route: Some("only".to_string()),
                    }],
                    default: Some(serde_json::json!({"hit": false})),
                }),
            }),
        );
        steps.insert("s1".to_string(), make_log_step("s1"));
        steps.insert("s2".to_string(), make_log_step("s2"));
        steps.insert("merge".to_string(), make_log_step("merge"));
        steps.insert("finish".to_string(), make_finish_step("finish"));

        let graph = ExecutionGraph {
            name: None,
            description: None,
            entry_point: "sw".to_string(),
            steps,
            execution_plan: vec![
                edge("sw", "s1", Some("only")),
                edge("sw", "s2", Some("default")),
                edge("s1", "merge", None),
                edge("s2", "merge", None),
                edge("merge", "finish", None),
            ],
            variables: HashMap::new(),
            input_schema: HashMap::new(),
            output_schema: HashMap::new(),
            notes: None,
            nodes: None,
            edges: None,
        };

        let mut ctx = EmitContext::new(false);
        let step = graph.steps.get("sw").unwrap();
        if let Step::Switch(sw) = step {
            let tokens = emit(sw, &mut ctx, &graph).unwrap();
            let code = tokens.to_string();

            // Single route dispatch
            assert!(
                code.contains("\"only\""),
                "Should include the single route label"
            );
            assert!(code.contains("__route =="), "Should dispatch on route");
            // Merge point steps
            assert!(code.contains("\"merge\""), "Should emit merge point step");
        } else {
            panic!("Expected Switch step");
        }
    }

    #[test]
    fn test_routing_switch_missing_edge_for_route() {
        // A case has route "orphan" but no execution plan edge for it.
        // The codegen should handle this gracefully (empty branch).
        let mut steps = HashMap::new();
        steps.insert(
            "sw".to_string(),
            Step::Switch(SwitchStep {
                id: "sw".to_string(),
                name: Some("Orphan Route".to_string()),
                config: Some(SwitchConfig {
                    value: MappingValue::Immediate(ImmediateValue {
                        value: serde_json::json!("x"),
                    }),
                    cases: vec![SwitchCase {
                        match_type: SwitchMatchType::Eq,
                        match_value: serde_json::json!("a"),
                        output: serde_json::json!({}),
                        route: Some("orphan".to_string()),
                    }],
                    default: Some(serde_json::json!({})),
                }),
            }),
        );
        steps.insert("fallback".to_string(), make_log_step("fallback"));
        steps.insert("finish".to_string(), make_finish_step("finish"));

        let graph = ExecutionGraph {
            name: None,
            description: None,
            entry_point: "sw".to_string(),
            steps,
            // Only default edge — no edge for "orphan"
            execution_plan: vec![
                edge("sw", "fallback", Some("default")),
                edge("fallback", "finish", None),
            ],
            variables: HashMap::new(),
            input_schema: HashMap::new(),
            output_schema: HashMap::new(),
            notes: None,
            nodes: None,
            edges: None,
        };

        let mut ctx = EmitContext::new(false);
        let step = graph.steps.get("sw").unwrap();
        if let Step::Switch(sw) = step {
            // Should not panic — missing edge produces empty branch
            let tokens = emit(sw, &mut ctx, &graph).unwrap();
            let code = tokens.to_string();

            assert!(
                code.contains("\"orphan\""),
                "Should still reference the orphan route label in dispatch"
            );
            assert!(
                code.contains("\"fallback\""),
                "Default branch should include fallback step"
            );
        } else {
            panic!("Expected Switch step");
        }
    }

    #[test]
    fn test_routing_switch_stores_route_in_steps_context() {
        let mut ctx = EmitContext::new(false);
        let graph = make_routing_graph();
        let step = graph.steps.get("sw").unwrap();

        if let Step::Switch(sw) = step {
            let tokens = emit(sw, &mut ctx, &graph).unwrap();
            let code = tokens.to_string();

            // Verify step result stored in context includes route
            assert!(
                code.contains("steps_context . insert"),
                "Should store result in steps_context"
            );
            assert!(
                code.contains("\"route\""),
                "Step result should include route field"
            );
            assert!(
                code.contains("\"outputs\""),
                "Step result should include outputs field"
            );
        } else {
            panic!("Expected Switch step");
        }
    }
}
