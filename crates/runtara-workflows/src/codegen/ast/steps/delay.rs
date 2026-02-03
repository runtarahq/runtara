// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Delay step code generation.
//!
//! Generates code for the Delay step that pauses workflow execution for a
//! specified duration. This is a **durable** delay - if the workflow crashes
//! during the delay, it resumes from where it left off.
//!
//! Platform-specific behavior:
//! - Native: Uses `sdk.durable_sleep()` which stores wake time in database
//! - WASI/Embedded: Uses blocking sleep (for now)

use proc_macro2::TokenStream;
use quote::quote;
use runtara_dsl::DelayStep;

use super::super::CodegenError;
use super::super::context::EmitContext;
use super::super::mapping;
use super::emit_step_span_start;

/// Emit code for a Delay step.
///
/// Generates code that:
/// 1. Creates a tracing span for the delay
/// 2. Evaluates the duration_ms mapping
/// 3. Calls `sdk.durable_sleep()` for durable delay
/// 4. Produces a step result with the delay info
pub fn emit(step: &DelayStep, ctx: &mut EmitContext) -> Result<TokenStream, CodegenError> {
    let step_id = &step.id;
    let step_name = step.name.as_deref();
    let step_name_display = step_name.unwrap_or("Unnamed");

    // Get variable names from context
    let step_var = ctx.declare_step(step_id);
    let source_var = ctx.temp_var("source");
    let steps_context = ctx.steps_context_var.clone();

    // Build the source for mapping evaluation
    let build_source = mapping::emit_build_source(ctx);

    // Emit duration mapping
    let duration_code = mapping::emit_mapping_value(&step.duration_ms, ctx, &source_var);

    // Emit step span
    let span_start = emit_step_span_start(step_id, step_name, "Delay");

    Ok(quote! {
        // Build source for mapping
        let #source_var = #build_source;

        // Define tracing span for this step
        #span_start

        // Wrap step execution in async block instrumented with span
        async {
            // Get duration in milliseconds from mapping
            let __duration_value = #duration_code;
            let __duration_ms: u64 = match __duration_value.as_u64() {
                Some(ms) => ms,
                None => match __duration_value.as_f64() {
                    Some(ms) => ms as u64,
                    None => {
                        return Err(format!(
                            "Delay step '{}': duration_ms must be a number, got: {}",
                            #step_id, __duration_value
                        ));
                    }
                }
            };

            // Perform durable sleep via SDK
            {
                let __sdk = sdk().lock().await;
                let __duration = std::time::Duration::from_millis(__duration_ms);
                __sdk.durable_sleep(__duration).await
                    .map_err(|e| format!("Delay step '{}' failed: {}", #step_id, e))?;
            }

            // Produce step result
            let #step_var = serde_json::json!({
                "stepId": #step_id,
                "stepName": #step_name_display,
                "stepType": "Delay",
                "duration_ms": __duration_ms
            });

            #steps_context.insert(#step_id.to_string(), #step_var.clone());

            Ok::<(), String>(())
        }.instrument(__step_span).await?;
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtara_dsl::{ImmediateValue, MappingValue, ReferenceValue};

    fn create_delay_step(id: &str, duration: MappingValue) -> DelayStep {
        DelayStep {
            id: id.to_string(),
            name: Some("Test Delay".to_string()),
            duration_ms: duration,
        }
    }

    #[test]
    fn test_emit_delay_immediate() {
        let step = create_delay_step(
            "delay-1",
            MappingValue::Immediate(ImmediateValue {
                value: serde_json::json!(5000),
            }),
        );
        let mut ctx = EmitContext::new(false);
        let result = emit(&step, &mut ctx);

        assert!(result.is_ok());
        let code = result.unwrap().to_string();
        assert!(code.contains("durable_sleep"));
        assert!(code.contains("duration_ms"));
    }

    #[test]
    fn test_emit_delay_reference() {
        let step = create_delay_step(
            "delay-2",
            MappingValue::Reference(ReferenceValue {
                value: "data.waitTime".to_string(),
                type_hint: None,
                default: None,
            }),
        );
        let mut ctx = EmitContext::new(false);
        let result = emit(&step, &mut ctx);

        assert!(result.is_ok());
        let code = result.unwrap().to_string();
        assert!(code.contains("durable_sleep"));
    }
}
