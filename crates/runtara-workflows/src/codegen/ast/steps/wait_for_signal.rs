// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! WaitForSignal step code generation.
//!
//! Generates code for the WaitForSignal step that pauses workflow execution
//! until an external signal is received.
//!
//! Signal ID Generation:
//! The signal_id is deterministically generated from:
//! - instance_id (from SDK)
//! - scenario_id (from context)
//! - step_id (from DSL)
//! - loop_indices (from runtime context, if in nested loops)
//!
//! Format: "{instance_id}/{scenario_id}/{step_id}/{loop_indices}"
//! Example: "inst-abc/root/approval_step/[0,2]"

use proc_macro2::TokenStream;
use quote::quote;
use runtara_dsl::WaitForSignalStep;

use super::super::CodegenError;
use super::super::context::EmitContext;
use super::super::mapping;
use super::super::program;
use super::{emit_step_debug_end, emit_step_debug_start, emit_step_span_start};

/// Emit code for a WaitForSignal step.
///
/// Generates code that:
/// 1. Computes a deterministic signal_id
/// 2. Executes the on_wait subgraph (if present) to notify external systems
/// 3. Polls for the signal with configurable timeout
/// 4. Returns the signal payload as step output
pub fn emit(step: &WaitForSignalStep, ctx: &mut EmitContext) -> Result<TokenStream, CodegenError> {
    let step_id = &step.id;
    let step_name = step.name.as_deref();
    let step_name_display = step_name.unwrap_or("Unnamed");

    // Get variable names from context
    let step_var = ctx.declare_step(step_id);
    let source_var = ctx.temp_var("source");
    let steps_context = ctx.steps_context_var.clone();
    let inputs_var = ctx.inputs_var.clone();

    // Build the source for mapping evaluation
    let build_source = mapping::emit_build_source(ctx);

    // Emit timeout mapping if present
    let timeout_code = if let Some(ref timeout) = step.timeout_ms {
        let timeout_mapping = mapping::emit_mapping_value(timeout, ctx, &source_var);
        quote! {
            let __timeout_value = #timeout_mapping;
            let __timeout_ms: Option<u64> = match __timeout_value {
                serde_json::Value::Null => None,
                serde_json::Value::Number(n) => n.as_u64().or_else(|| n.as_f64().map(|f| f as u64)),
                _ => {
                    return Err(format!(
                        "WaitForSignal step '{}': timeout_ms must be a number, got: {}",
                        #step_id, __timeout_value
                    ));
                }
            };
        }
    } else {
        quote! {
            let __timeout_ms: Option<u64> = None;
        }
    };

    // Poll interval (default 1000ms)
    let poll_interval = step.poll_interval_ms.unwrap_or(1000);

    // Generate on_wait subgraph if present
    let on_wait_code = if let Some(ref on_wait) = step.on_wait {
        let on_wait_fn_name = ctx.temp_var(&format!(
            "{}_on_wait",
            EmitContext::sanitize_ident(step_id)
        ));
        let on_wait_fn = program::emit_graph_as_function(&on_wait_fn_name, on_wait, ctx)?;

        quote! {
            // Define and execute on_wait subgraph
            #on_wait_fn

            // Build inputs for on_wait with signal context
            let __on_wait_inputs = {
                let mut vars = match (*#inputs_var.variables).clone() {
                    serde_json::Value::Object(m) => m,
                    _ => serde_json::Map::new(),
                };
                // Inject signal_id and instance_id for external system notification
                vars.insert("_signal_id".to_string(), serde_json::json!(__signal_id));
                vars.insert("_instance_id".to_string(), serde_json::json!(__instance_id));

                ScenarioInputs {
                    data: #inputs_var.data.clone(),
                    variables: Arc::new(serde_json::Value::Object(vars)),
                    parent_scope_id: #inputs_var.parent_scope_id.clone(),
                }
            };

            // Execute on_wait subgraph
            #on_wait_fn_name(Arc::new(__on_wait_inputs)).await
                .map_err(|e| format!("WaitForSignal step '{}' on_wait failed: {}", #step_id, e))?;
        }
    } else {
        quote! {}
    };

    // Generate debug events if enabled
    let debug_start = emit_step_debug_start(
        ctx,
        step_id,
        step_name,
        "WaitForSignal",
        None,
        None,
        Some(&inputs_var),
        None,
    );
    let debug_end = emit_step_debug_end(
        ctx,
        step_id,
        step_name,
        "WaitForSignal",
        Some(&step_var),
        Some(&inputs_var),
        None,
    );

    // Emit step span
    let span_start = emit_step_span_start(step_id, step_name, "WaitForSignal");

    Ok(quote! {
        // Build source for mapping
        let #source_var = #build_source;

        // Define tracing span for this step
        #span_start

        // Wrap step execution in async block instrumented with span
        async {
            #debug_start

            // Get instance_id from SDK
            let __instance_id = {
                let __sdk = sdk().lock().await;
                __sdk.instance_id().to_string()
            };

            // Build deterministic signal_id
            let __signal_id = {
                // Get scenario_id from context
                let scenario_id = (*#inputs_var.variables)
                    .as_object()
                    .and_then(|vars| vars.get("_scenario_id"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("root");

                // Get loop indices for uniqueness in nested loops
                let indices_suffix = (*#inputs_var.variables)
                    .as_object()
                    .and_then(|vars| vars.get("_loop_indices"))
                    .and_then(|v| v.as_array())
                    .filter(|arr| !arr.is_empty())
                    .map(|arr| {
                        let indices: Vec<String> = arr.iter()
                            .map(|v| v.to_string())
                            .collect();
                        format!("/[{}]", indices.join(","))
                    })
                    .unwrap_or_default();

                format!("{}/{}/{}{}", __instance_id, scenario_id, #step_id, indices_suffix)
            };

            tracing::debug!(signal_id = %__signal_id, "WaitForSignal: computed signal_id");

            // Evaluate timeout
            #timeout_code

            // Execute on_wait subgraph (notifies external system of signal_id)
            #on_wait_code

            // Poll for signal with timeout
            let __poll_interval = std::time::Duration::from_millis(#poll_interval);
            let __start_time = std::time::Instant::now();
            let __signal_payload: serde_json::Value;

            loop {
                // Check for cancellation
                {
                    let mut __sdk = sdk().lock().await;
                    if let Err(e) = __sdk.check_signals().await {
                        return Err(format!("WaitForSignal step '{}': {}", #step_id, e));
                    }
                }

                // Poll for custom signal
                let __maybe_signal = {
                    let mut __sdk = sdk().lock().await;
                    __sdk.poll_custom_signal(&__signal_id).await
                        .map_err(|e| format!("WaitForSignal step '{}' poll failed: {}", #step_id, e))?
                };

                if let Some(payload) = __maybe_signal {
                    // Signal received!
                    __signal_payload = serde_json::from_slice(&payload)
                        .unwrap_or_else(|_| serde_json::Value::String(
                            String::from_utf8_lossy(&payload).to_string()
                        ));
                    tracing::info!(signal_id = %__signal_id, "WaitForSignal: signal received");
                    break;
                }

                // Check timeout
                if let Some(timeout) = __timeout_ms {
                    if __start_time.elapsed().as_millis() as u64 >= timeout {
                        return Err(format!(
                            "WaitForSignal step '{}' timed out after {}ms waiting for signal '{}'",
                            #step_id, timeout, __signal_id
                        ));
                    }
                }

                // Sleep before next poll
                tokio::time::sleep(__poll_interval).await;
            }

            // Produce step result
            let #step_var = serde_json::json!({
                "stepId": #step_id,
                "stepName": #step_name_display,
                "stepType": "WaitForSignal",
                "signal_id": __signal_id,
                "payload": __signal_payload
            });

            #steps_context.insert(#step_id.to_string(), #step_var.clone());

            #debug_end

            Ok::<(), String>(())
        }.instrument(__step_span).await?;
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtara_dsl::{ImmediateValue, MappingValue};

    fn create_wait_for_signal_step(id: &str) -> WaitForSignalStep {
        WaitForSignalStep {
            id: id.to_string(),
            name: Some("Test Wait".to_string()),
            on_wait: None,
            timeout_ms: None,
            poll_interval_ms: None,
        }
    }

    fn create_wait_with_timeout(id: &str, timeout_ms: u64) -> WaitForSignalStep {
        WaitForSignalStep {
            id: id.to_string(),
            name: Some("Test Wait with Timeout".to_string()),
            on_wait: None,
            timeout_ms: Some(MappingValue::Immediate(ImmediateValue {
                value: serde_json::json!(timeout_ms),
            })),
            poll_interval_ms: None,
        }
    }

    #[test]
    fn test_emit_basic_wait_for_signal() {
        let step = create_wait_for_signal_step("wait-1");
        let mut ctx = EmitContext::new(false);
        let result = emit(&step, &mut ctx);

        assert!(result.is_ok());
        let code = result.unwrap().to_string();
        assert!(code.contains("poll_custom_signal"));
        assert!(code.contains("signal_id"));
        assert!(code.contains("WaitForSignal"));
    }

    #[test]
    fn test_emit_wait_with_timeout() {
        let step = create_wait_with_timeout("wait-timeout", 30000);
        let mut ctx = EmitContext::new(false);
        let result = emit(&step, &mut ctx);

        assert!(result.is_ok());
        let code = result.unwrap().to_string();
        assert!(code.contains("timeout"));
        assert!(code.contains("timed out"));
    }

    #[test]
    fn test_emit_wait_generates_signal_id() {
        let step = create_wait_for_signal_step("approval-step");
        let mut ctx = EmitContext::new(false);
        let result = emit(&step, &mut ctx);

        assert!(result.is_ok());
        let code = result.unwrap().to_string();
        // Should include signal_id generation logic
        assert!(code.contains("_scenario_id"));
        assert!(code.contains("_loop_indices"));
        assert!(code.contains("approval-step"));
    }

    #[test]
    fn test_emit_wait_with_poll_interval() {
        let step = WaitForSignalStep {
            id: "wait-poll".to_string(),
            name: Some("Custom Poll Interval".to_string()),
            on_wait: None,
            timeout_ms: None,
            poll_interval_ms: Some(500),
        };
        let mut ctx = EmitContext::new(false);
        let result = emit(&step, &mut ctx);

        assert!(result.is_ok());
        let code = result.unwrap().to_string();
        assert!(code.contains("500"));
    }

    #[test]
    fn test_emit_wait_stores_in_steps_context() {
        let step = create_wait_for_signal_step("wait-store");
        let mut ctx = EmitContext::new(false);
        let result = emit(&step, &mut ctx);

        assert!(result.is_ok());
        let code = result.unwrap().to_string();
        assert!(code.contains("steps_context . insert"));
    }

    #[test]
    fn test_emit_wait_debug_mode() {
        let step = create_wait_for_signal_step("wait-debug");
        let mut ctx = EmitContext::new(true); // debug mode ON
        let result = emit(&step, &mut ctx);

        assert!(result.is_ok());
        let code = result.unwrap().to_string();
        assert!(code.contains("step_debug_start") || code.contains("debug"));
    }
}
