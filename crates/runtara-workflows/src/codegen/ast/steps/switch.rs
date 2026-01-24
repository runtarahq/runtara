// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Switch step emitter.
//!
//! The Switch step performs multi-way branching based on value matching.

use proc_macro2::TokenStream;
use quote::quote;

use super::super::context::EmitContext;
use super::super::mapping;
use super::{emit_step_debug_end, emit_step_debug_start};
use runtara_dsl::{MappingValue, SwitchStep};

/// Emit code for a Switch step.
#[allow(clippy::too_many_lines)]
pub fn emit(step: &SwitchStep, ctx: &mut EmitContext) -> TokenStream {
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

    // Clone scenario inputs var for debug events (to access _loop_indices)
    let scenario_inputs_var = ctx.inputs_var.clone();

    // Generate debug event emissions (Switch doesn't create a scope)
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

    quote! {
        let #source_var = #build_source;
        let #inputs_var = #inputs_code;

        #debug_start

        // Extract switch components
        let switch_value = #inputs_var.get("value").cloned().unwrap_or(serde_json::Value::Null);

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
            "stepName": #step_name_display,
            "stepType": "Switch",
            "outputs": output
        });

        #debug_end

        #steps_context.insert(#step_id.to_string(), #step_var.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codegen::ast::context::EmitContext;
    use runtara_dsl::{ImmediateValue, ReferenceValue, SwitchCase, SwitchConfig, SwitchMatchType};

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

        let tokens = emit(&step, &mut ctx);
        let code = tokens.to_string();

        // Verify basic structure elements
        assert!(code.contains("switch_value"), "Should extract switch value");
        assert!(code.contains("cases"), "Should extract cases array");
        assert!(
            code.contains("default_output"),
            "Should handle default output"
        );
        assert!(
            code.contains("matched_output"),
            "Should track matched output"
        );
    }

    #[test]
    fn test_emit_switch_value_extraction() {
        let mut ctx = EmitContext::new(false);
        let step = create_switch_step("switch-value", "inputs.status", vec![]);

        let tokens = emit(&step, &mut ctx);
        let code = tokens.to_string();

        // Debug: print actual code to see token format
        // eprintln!("Generated code:\n{}", code);

        // Verify value is extracted from inputs
        // TokenStream adds spaces around quotes: . get ("value")
        assert!(
            code.contains(r#". get ("value")"#),
            "Should get value from inputs"
        );
        assert!(
            code.contains("unwrap_or (serde_json :: Value :: Null)"),
            "Should default to Null if value missing"
        );
    }

    #[test]
    fn test_emit_switch_cases_iteration() {
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
        let step = create_switch_step("switch-cases", "data.status", cases);

        let tokens = emit(&step, &mut ctx);
        let code = tokens.to_string();

        // Verify case iteration structure
        assert!(
            code.contains("for (_case_index , case) in cases . iter () . enumerate ()"),
            "Should iterate over cases"
        );
        assert!(
            code.contains("if matched_output . is_some ()"),
            "Should check if already matched"
        );
        assert!(code.contains("break"), "Should break on first match");
    }

    #[test]
    fn test_emit_switch_match_type_extraction() {
        let mut ctx = EmitContext::new(false);
        let cases = vec![create_case(
            SwitchMatchType::Gt,
            serde_json::json!(10),
            serde_json::json!({"result": "large"}),
        )];
        let step = create_switch_step("switch-match-type", "value", cases);

        let tokens = emit(&step, &mut ctx);
        let code = tokens.to_string();

        // Verify match type is extracted (TokenStream adds spaces)
        assert!(
            code.contains(r#". get ("matchType")"#),
            "Should get matchType from case"
        );
        assert!(
            code.contains(r#". get ("match")"#),
            "Should get match value from case"
        );
        assert!(
            code.contains(r#". get ("output")"#),
            "Should get output from case"
        );
    }

    #[test]
    fn test_emit_switch_comparison_operators() {
        let mut ctx = EmitContext::new(false);
        let step = create_switch_step("switch-compare", "value", vec![]);

        let tokens = emit(&step, &mut ctx);
        let code = tokens.to_string();

        // Verify comparison operator handling (TokenStream adds spaces)
        assert!(
            code.contains(r#"Some ("EQ")"#),
            "Should handle EQ comparison"
        );
        assert!(
            code.contains(r#"Some ("NE")"#),
            "Should handle NE comparison"
        );
        assert!(
            code.contains(r#"Some ("GT")"#),
            "Should handle GT comparison"
        );
        assert!(
            code.contains(r#"Some ("GTE")"#),
            "Should handle GTE comparison"
        );
        assert!(
            code.contains(r#"Some ("LT")"#),
            "Should handle LT comparison"
        );
        assert!(
            code.contains(r#"Some ("LTE")"#),
            "Should handle LTE comparison"
        );
    }

    #[test]
    fn test_emit_switch_string_operators() {
        let mut ctx = EmitContext::new(false);
        let step = create_switch_step("switch-string", "value", vec![]);

        let tokens = emit(&step, &mut ctx);
        let code = tokens.to_string();

        // Verify string operator handling (TokenStream adds spaces)
        assert!(
            code.contains(r#"Some ("STARTS_WITH")"#),
            "Should handle STARTS_WITH"
        );
        assert!(
            code.contains(r#"Some ("ENDS_WITH")"#),
            "Should handle ENDS_WITH"
        );
        assert!(
            code.contains("starts_with"),
            "Should use starts_with method"
        );
        assert!(code.contains("ends_with"), "Should use ends_with method");
    }

    #[test]
    fn test_emit_switch_array_operators() {
        let mut ctx = EmitContext::new(false);
        let step = create_switch_step("switch-array", "value", vec![]);

        let tokens = emit(&step, &mut ctx);
        let code = tokens.to_string();

        // Verify array operator handling (TokenStream adds spaces)
        assert!(
            code.contains(r#"Some ("CONTAINS")"#),
            "Should handle CONTAINS"
        );
        assert!(code.contains(r#"Some ("IN")"#), "Should handle IN");
        assert!(code.contains(r#"Some ("NOT_IN")"#), "Should handle NOT_IN");
        assert!(code.contains("as_array"), "Should check for array type");
    }

    #[test]
    fn test_emit_switch_utility_operators() {
        let mut ctx = EmitContext::new(false);
        let step = create_switch_step("switch-utility", "value", vec![]);

        let tokens = emit(&step, &mut ctx);
        let code = tokens.to_string();

        // Verify utility operator handling (TokenStream adds spaces)
        assert!(
            code.contains(r#"Some ("IS_DEFINED")"#),
            "Should handle IS_DEFINED"
        );
        assert!(
            code.contains(r#"Some ("IS_EMPTY")"#),
            "Should handle IS_EMPTY"
        );
        assert!(
            code.contains(r#"Some ("IS_NOT_EMPTY")"#),
            "Should handle IS_NOT_EMPTY"
        );
        assert!(code.contains("is_null"), "Should check for null");
        assert!(code.contains("is_empty"), "Should check for empty");
    }

    #[test]
    fn test_emit_switch_compound_operators() {
        let mut ctx = EmitContext::new(false);
        let step = create_switch_step("switch-compound", "value", vec![]);

        let tokens = emit(&step, &mut ctx);
        let code = tokens.to_string();

        // Verify compound operator handling (TokenStream adds spaces)
        assert!(
            code.contains(r#"Some ("BETWEEN")"#),
            "Should handle BETWEEN"
        );
        assert!(code.contains(r#"Some ("RANGE")"#), "Should handle RANGE");
    }

    #[test]
    fn test_emit_switch_default_fallback() {
        let mut ctx = EmitContext::new(false);
        let step = create_switch_step("switch-default", "value", vec![]);

        let tokens = emit(&step, &mut ctx);
        let code = tokens.to_string();

        // Verify default fallback logic
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

        let tokens = emit(&step, &mut ctx);
        let code = tokens.to_string();

        // Verify output JSON structure
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

        let tokens = emit(&step, &mut ctx);
        let code = tokens.to_string();

        // Verify result is stored in steps_context
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
        let mut ctx = EmitContext::new(true); // debug mode ON
        let step = create_switch_step("switch-debug", "value", vec![]);

        let tokens = emit(&step, &mut ctx);
        let code = tokens.to_string();

        // Verify debug events are emitted
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
        let mut ctx = EmitContext::new(false); // debug mode OFF
        let step = create_switch_step("switch-no-debug", "value", vec![]);

        let tokens = emit(&step, &mut ctx);
        let code = tokens.to_string();

        // Core switch logic should still be present
        assert!(code.contains("switch_value"), "Should have switch logic");
        assert!(code.contains("matched_output"), "Should track matching");
    }

    #[test]
    fn test_emit_switch_with_unnamed_step() {
        let mut ctx = EmitContext::new(false);
        let step = SwitchStep {
            id: "switch-unnamed".to_string(),
            name: None, // No name
            config: Some(SwitchConfig {
                value: MappingValue::Immediate(ImmediateValue {
                    value: serde_json::json!("test"),
                }),
                cases: vec![],
                default: None,
            }),
        };

        let tokens = emit(&step, &mut ctx);
        let code = tokens.to_string();

        // Should use "Unnamed" as display name
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
            config: None, // No config
        };

        let tokens = emit(&step, &mut ctx);
        let code = tokens.to_string();

        // Should create empty object for inputs
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

        let tokens = emit(&step, &mut ctx);
        let code = tokens.to_string();

        // Should handle immediate values
        assert!(
            code.contains("switch_value"),
            "Should extract switch value from immediate"
        );
    }

    #[test]
    fn test_emit_switch_helper_functions() {
        let mut ctx = EmitContext::new(false);
        let step = create_switch_step("switch-helpers", "value", vec![]);

        let tokens = emit(&step, &mut ctx);
        let code = tokens.to_string();

        // Verify helper function calls
        assert!(
            code.contains("switch_equals"),
            "Should use switch_equals helper"
        );
        assert!(
            code.contains("switch_compare"),
            "Should use switch_compare helper"
        );
    }
}
