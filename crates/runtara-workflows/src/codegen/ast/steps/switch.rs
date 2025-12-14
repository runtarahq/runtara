// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Switch step emitter.
//!
//! The Switch step performs multi-way branching based on value matching.

use proc_macro2::TokenStream;
use quote::quote;

use super::super::context::EmitContext;
use super::super::mapping;
use runtara_dsl::{MappingValue, SwitchStep};

/// Emit code for a Switch step.
pub fn emit(step: &SwitchStep, ctx: &mut EmitContext) -> TokenStream {
    let step_id = &step.id;
    let step_name = step.name.as_deref().unwrap_or("Unnamed");
    let debug_mode = ctx.debug_mode;

    // Do all mutable operations first
    let step_var = ctx.declare_step(step_id);
    let source_var = ctx.temp_var("source");
    let inputs_var = ctx.temp_var("switch_inputs");

    // Clone immutable references
    let steps_context = ctx.steps_context_var.clone();
    let runtime_ctx = ctx.runtime_ctx_var.clone();

    // Build the source for input mapping
    let build_source = mapping::emit_build_source(ctx);

    // Build inputs from the typed SwitchConfig
    let inputs_code = if let Some(ref config) = step.config {
        // Emit mapping code for the value field
        let value_mapping: std::collections::HashMap<String, MappingValue> =
            [("value".to_string(), config.value.clone())]
                .into_iter()
                .collect();

        let mapping_code = mapping::emit_input_mapping(&value_mapping, ctx, &source_var);

        // Convert cases to JSON array
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

    // Debug timing variables
    let debug_start_time_var = ctx.temp_var("step_start_time");
    let debug_duration_var = ctx.temp_var("duration_ms");

    let debug_start = if debug_mode {
        quote! {
            #runtime_ctx.step_started(#step_id, "Switch", &#inputs_var);
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
            eprintln!("  -> Switch value: {}", switch_value);
        }
    } else {
        quote! {}
    };

    quote! {
        let #source_var = #build_source;
        let #inputs_var = #inputs_code;

        #debug_start

        // Extract switch components
        let switch_value = #inputs_var.get("value").cloned().unwrap_or(serde_json::Value::Null);
        #debug_log

        let cases = #inputs_var.get("cases")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();

        let default_output = #inputs_var.get("default")
            .cloned()
            .unwrap_or_else(|| serde_json::json!({}));

        // Find matching case
        let mut matched_output: Option<serde_json::Value> = None;

        for (_case_index, case) in cases.iter().enumerate() {
            if matched_output.is_some() {
                break;
            }

            let match_type = case.get("matchType").and_then(|v| v.as_str());
            let match_value = case.get("match");
            let case_output = case.get("output").cloned().unwrap_or(serde_json::Value::Null);

            // Match types use SCREAMING_SNAKE_CASE
            let matches = match match_type {
                // Comparison operators
                Some("EQ") => {
                    match_value.map(|mv| switch_equals(&switch_value, mv)).unwrap_or(false)
                }
                Some("NE") => {
                    match_value.map(|mv| !switch_equals(&switch_value, mv)).unwrap_or(false)
                }
                Some("GT") => {
                    match_value.map(|mv| switch_compare(&switch_value, mv, "gt")).unwrap_or(false)
                }
                Some("GTE") => {
                    match_value.map(|mv| switch_compare(&switch_value, mv, "gte")).unwrap_or(false)
                }
                Some("LT") => {
                    match_value.map(|mv| switch_compare(&switch_value, mv, "lt")).unwrap_or(false)
                }
                Some("LTE") => {
                    match_value.map(|mv| switch_compare(&switch_value, mv, "lte")).unwrap_or(false)
                }

                // String operators
                Some("STARTS_WITH") => {
                    match (switch_value.as_str(), match_value.and_then(|v| v.as_str())) {
                        (Some(s), Some(prefix)) => s.starts_with(prefix),
                        _ => false,
                    }
                }
                Some("ENDS_WITH") => {
                    match (switch_value.as_str(), match_value.and_then(|v| v.as_str())) {
                        (Some(s), Some(suffix)) => s.ends_with(suffix),
                        _ => false,
                    }
                }

                // Array operators
                Some("CONTAINS") => {
                    // Array contains value: switch_value (array) contains match_value
                    switch_value.as_array()
                        .map(|arr| {
                            match_value.map(|mv| arr.iter().any(|v| switch_equals(v, mv))).unwrap_or(false)
                        })
                        .unwrap_or(false)
                }
                Some("IN") => {
                    // Value in array: switch_value is in match_value (array)
                    match_value
                        .and_then(|mv| mv.as_array())
                        .map(|arr| arr.iter().any(|v| switch_equals(&switch_value, v)))
                        .unwrap_or(false)
                }
                Some("NOT_IN") => {
                    // Value not in array
                    match_value
                        .and_then(|mv| mv.as_array())
                        .map(|arr| !arr.iter().any(|v| switch_equals(&switch_value, v)))
                        .unwrap_or(true)
                }

                // Utility operators
                Some("IS_DEFINED") => {
                    !switch_value.is_null()
                }
                Some("IS_EMPTY") => {
                    match &switch_value {
                        serde_json::Value::Array(a) => a.is_empty(),
                        serde_json::Value::String(s) => s.is_empty(),
                        serde_json::Value::Object(o) => o.is_empty(),
                        serde_json::Value::Null => true,
                        _ => false,
                    }
                }
                Some("IS_NOT_EMPTY") => {
                    match &switch_value {
                        serde_json::Value::Array(a) => !a.is_empty(),
                        serde_json::Value::String(s) => !s.is_empty(),
                        serde_json::Value::Object(o) => !o.is_empty(),
                        serde_json::Value::Null => false,
                        _ => true,
                    }
                }

                // Compound match types
                Some("BETWEEN") => {
                    match_value
                        .and_then(|mv| mv.as_array())
                        .filter(|arr| arr.len() >= 2)
                        .map(|arr| {
                            switch_compare(&switch_value, &arr[0], "gte")
                                && switch_compare(&switch_value, &arr[1], "lte")
                        })
                        .unwrap_or(false)
                }
                Some("RANGE") => {
                    match_value.map(|mv| {
                        let mut result = true;
                        if let Some(gte) = mv.get("gte") {
                            result = result && switch_compare(&switch_value, gte, "gte");
                        }
                        if let Some(gt) = mv.get("gt") {
                            result = result && switch_compare(&switch_value, gt, "gt");
                        }
                        if let Some(lte) = mv.get("lte") {
                            result = result && switch_compare(&switch_value, lte, "lte");
                        }
                        if let Some(lt) = mv.get("lt") {
                            result = result && switch_compare(&switch_value, lt, "lt");
                        }
                        result
                    }).unwrap_or(false)
                }
                _ => false,
            };

            if matches {
                matched_output = Some(process_switch_output(&case_output, &#source_var));
            }
        }

        let output = matched_output.unwrap_or_else(|| process_switch_output(&default_output, &#source_var));

        let #step_var = serde_json::json!({
            "stepId": #step_id,
            "stepName": #step_name,
            "stepType": "Switch",
            "outputs": output
        });

        #debug_complete

        #steps_context.insert(#step_id.to_string(), #step_var.clone());
    }
}
