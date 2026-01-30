// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Finish step emitter.
//!
//! The Finish step defines the scenario outputs and returns from the workflow.

use proc_macro2::TokenStream;
use quote::quote;

use super::super::CodegenError;
use super::super::context::EmitContext;
use super::super::mapping;
use super::{emit_step_debug_end, emit_step_debug_start, emit_step_span_start};
use runtara_dsl::FinishStep;

/// Emit code for a Finish step.
///
/// The Finish step computes its outputs and immediately returns from the
/// workflow function. This is necessary to support multiple Finish steps
/// in different branches (e.g., after a Conditional step).
pub fn emit(step: &FinishStep, ctx: &mut EmitContext) -> Result<TokenStream, CodegenError> {
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

    // Generate debug event emissions (Finish doesn't create a scope)
    let debug_start = emit_step_debug_start(
        ctx,
        step_id,
        step_name,
        "Finish",
        Some(&finish_inputs_var),
        input_mapping_json.as_deref(),
        Some(&scenario_inputs_var),
        None,
    );
    let debug_end = emit_step_debug_end(
        ctx,
        step_id,
        step_name,
        "Finish",
        Some(&step_var),
        Some(&scenario_inputs_var),
        None,
    );

    // Generate tracing span for OpenTelemetry
    let span_def = emit_step_span_start(step_id, step_name, "Finish");

    // The Finish step immediately returns from the workflow function.
    // This allows multiple Finish steps in different branches to work correctly.
    Ok(quote! {
        let #source_var = #build_source;
        let #finish_inputs_var = serde_json::json!({"finishing": true});

        // Define tracing span for this step
        #span_def

        // Wrap step execution in async block instrumented with span
        // The async block returns the finish output value
        let __finish_output: serde_json::Value = async {
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

            #outputs_var
        }.instrument(__step_span).await;

        // Return immediately with the outputs
        return Ok(__finish_output);
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codegen::ast::context::EmitContext;
    use runtara_dsl::{ImmediateValue, MappingValue, ReferenceValue};
    use std::collections::HashMap;

    /// Helper to create a minimal finish step for testing.
    fn create_finish_step(step_id: &str) -> FinishStep {
        FinishStep {
            id: step_id.to_string(),
            name: Some("Test Finish".to_string()),
            input_mapping: None,
        }
    }

    #[test]
    fn test_emit_finish_basic_structure() {
        let mut ctx = EmitContext::new(false);
        let step = create_finish_step("finish-basic");

        let tokens = emit(&step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Verify basic structure
        assert!(
            code.contains("return Ok"),
            "Finish step should return from workflow"
        );
        assert!(
            code.contains("\"finishing\" : true"),
            "Should have finishing indicator in inputs"
        );
    }

    #[test]
    fn test_emit_finish_returns_outputs() {
        let mut ctx = EmitContext::new(false);
        let step = create_finish_step("finish-return");

        let tokens = emit(&step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Finish step should return outputs
        assert!(code.contains("return Ok"), "Should return with Ok wrapper");
    }

    #[test]
    fn test_emit_finish_extracts_outputs_field() {
        let mut ctx = EmitContext::new(false);
        let step = create_finish_step("finish-extract");

        let tokens = emit(&step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Should extract outputs field if present
        assert!(
            code.contains(r#". get ("outputs")"#),
            "Should try to extract outputs field"
        );
        assert!(
            code.contains("unwrap_or"),
            "Should fall back to whole value if no outputs field"
        );
    }

    #[test]
    fn test_emit_finish_with_input_mapping() {
        let mut ctx = EmitContext::new(false);
        let mut mapping = HashMap::new();
        mapping.insert(
            "result".to_string(),
            MappingValue::Reference(ReferenceValue {
                value: "steps.final.outputs".to_string(),
                type_hint: None,
                default: None,
            }),
        );

        let step = FinishStep {
            id: "finish-mapped".to_string(),
            name: Some("Mapped Finish".to_string()),
            input_mapping: Some(mapping),
        };

        let tokens = emit(&step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Should not use empty object
        assert!(
            !code.contains("Value :: Object (serde_json :: Map :: new ())")
                || code.contains("resolve_path"),
            "Should have input mapping code"
        );
    }

    #[test]
    fn test_emit_finish_empty_input_mapping() {
        let mut ctx = EmitContext::new(false);
        let step = FinishStep {
            id: "finish-empty".to_string(),
            name: Some("Empty Mapping".to_string()),
            input_mapping: Some(HashMap::new()),
        };

        let tokens = emit(&step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Should use empty object for empty mapping
        assert!(
            code.contains("serde_json :: Value :: Object (serde_json :: Map :: new ())"),
            "Should create empty object for empty input mapping"
        );
    }

    #[test]
    fn test_emit_finish_output_structure() {
        let mut ctx = EmitContext::new(false);
        let step = create_finish_step("finish-output");

        let tokens = emit(&step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Verify output JSON structure
        assert!(code.contains("\"stepId\""), "Should include stepId");
        assert!(code.contains("\"stepName\""), "Should include stepName");
        assert!(code.contains("\"stepType\""), "Should include stepType");
        assert!(code.contains("\"Finish\""), "Should have stepType = Finish");
        assert!(code.contains("\"outputs\""), "Should include outputs");
    }

    #[test]
    fn test_emit_finish_stores_in_steps_context() {
        let mut ctx = EmitContext::new(false);
        let step = create_finish_step("finish-store");

        let tokens = emit(&step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Verify result is stored before returning
        assert!(
            code.contains("steps_context . insert"),
            "Should store result in steps_context before returning"
        );
    }

    #[test]
    fn test_emit_finish_debug_mode_enabled() {
        let mut ctx = EmitContext::new(true); // debug mode ON
        let step = create_finish_step("finish-debug");

        let tokens = emit(&step, &mut ctx).unwrap();
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
    fn test_emit_finish_debug_mode_disabled() {
        let mut ctx = EmitContext::new(false); // debug mode OFF
        let step = create_finish_step("finish-no-debug");

        let tokens = emit(&step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Core finish logic should still be present
        assert!(code.contains("return Ok"), "Should still return");
    }

    #[test]
    fn test_emit_finish_with_unnamed_step() {
        let mut ctx = EmitContext::new(false);
        let step = FinishStep {
            id: "finish-unnamed".to_string(),
            name: None, // No name
            input_mapping: None,
        };

        let tokens = emit(&step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Should use "Finish" as default display name
        assert!(
            code.contains("\"Finish\""),
            "Should use 'Finish' for unnamed finish steps"
        );
    }

    #[test]
    fn test_emit_finish_with_immediate_value() {
        let mut ctx = EmitContext::new(false);
        let mut mapping = HashMap::new();
        mapping.insert(
            "message".to_string(),
            MappingValue::Immediate(ImmediateValue {
                value: serde_json::json!("Workflow completed successfully"),
            }),
        );

        let step = FinishStep {
            id: "finish-immediate".to_string(),
            name: Some("Immediate Finish".to_string()),
            input_mapping: Some(mapping),
        };

        let tokens = emit(&step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Should include immediate value in mapping
        assert!(
            code.contains("message") || code.contains("Workflow completed"),
            "Should include immediate value mapping"
        );
    }
}
