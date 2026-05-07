// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Agent step emitter.
//!
//! The Agent step executes an agent capability.
//! All agent capabilities use #[resilient] macro for checkpoint-based crash recovery.
//! Rate limiting is handled via connection service responses.

use proc_macro2::TokenStream;
use quote::quote;

use super::super::CodegenError;
use super::super::context::EmitContext;
use super::super::mapping;
use super::{
    emit_agent_span_start, emit_breakpoint_check, emit_step_debug_end, emit_step_debug_start,
};
use runtara_dsl::AgentStep;
use runtara_dsl::agent_meta::get_all_capabilities;

/// Check if a capability requires rate limiting by looking up its metadata.
/// Returns true if the capability has rate_limited = true in its #[capability] macro.
fn needs_rate_limiting(agent_id: &str, capability_id: &str) -> bool {
    let agent_lower = agent_id.to_lowercase();

    for cap in get_all_capabilities() {
        let module = cap.module.unwrap_or("unknown");
        if module == agent_lower && cap.capability_id == capability_id {
            return cap.rate_limited;
        }
    }

    // Default to false if capability not found (shouldn't happen for valid workflows)
    false
}

/// Emit code for an Agent step.
pub fn emit(step: &AgentStep, ctx: &mut EmitContext) -> Result<TokenStream, CodegenError> {
    let step_id = &step.id;
    let step_name = step.name.as_deref();
    let step_name_display = step_name.unwrap_or("Unnamed");
    let agent_id = &step.agent_id;
    let capability_id = &step.capability_id;

    // All capabilities use #[resilient] for crash recovery.
    // Rate limiting is only applied to external API calls.
    let needs_rate_limit = needs_rate_limiting(agent_id, capability_id);

    // Rate-limited capabilities get more retries and a longer base delay,
    // since 429 errors are expected to succeed after waiting.
    let (max_retries, retry_delay) = if needs_rate_limit {
        (
            step.max_retries.unwrap_or(5),
            step.retry_delay.unwrap_or(2000),
        )
    } else {
        (
            step.max_retries.unwrap_or(3),
            step.retry_delay.unwrap_or(1000),
        )
    };

    // Do all mutable operations first
    let step_var = ctx.declare_step(step_id);
    let source_var = ctx.temp_var("source");
    let step_inputs_var = ctx.temp_var("step_inputs");
    let result_var = ctx.temp_var("result");
    let durable_fn_name =
        ctx.temp_var(&format!("{}_durable", EmitContext::sanitize_ident(step_id)));

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

    // Generate input mapping
    let base_inputs_code = if let Some(ref input_mapping) = step.input_mapping {
        if !input_mapping.is_empty() {
            let mapping_code = mapping::emit_input_mapping(input_mapping, ctx, &source_var);
            quote! { #mapping_code }
        } else {
            quote! { serde_json::Value::Object(serde_json::Map::new()) }
        }
    } else {
        quote! { serde_json::Value::Object(serde_json::Map::new()) }
    };

    // Generate the durable capability execution
    let rate_limit_budget = ctx.rate_limit_budget_ms;
    let step_durable = ctx.durable && step.durable.unwrap_or(true);
    let execute_capability = if needs_rate_limit {
        emit_durable_rate_limited_call(
            step_id,
            agent_id,
            capability_id,
            &step_inputs_var,
            &durable_fn_name,
            step.connection_id.as_deref(),
            ctx,
            step_durable,
            max_retries,
            retry_delay,
            rate_limit_budget,
        )
    } else {
        emit_durable_call(
            step_id,
            agent_id,
            capability_id,
            &step_inputs_var,
            &durable_fn_name,
            step.connection_id.as_deref(),
            ctx,
            step_durable,
            max_retries,
            retry_delay,
            rate_limit_budget,
        )
    };

    // Clone workflow inputs var for debug events (to access _loop_indices)
    let workflow_inputs_var = ctx.inputs_var.clone();

    // Error output variable for debug events on failure
    let error_output_var = ctx.temp_var("error_output");

    // Generate debug event emissions (Agent doesn't create a scope, so no override_scope_id)
    let debug_start = emit_step_debug_start(
        ctx,
        step_id,
        step_name,
        "Agent",
        Some(&step_inputs_var),
        input_mapping_json.as_deref(),
        Some(&workflow_inputs_var),
        None,
    );
    let debug_end = emit_step_debug_end(
        ctx,
        step_id,
        step_name,
        "Agent",
        Some(&result_var),
        Some(&workflow_inputs_var),
        None,
    );
    // Debug end event for error path — uses error output variable instead of result
    let debug_end_error = emit_step_debug_end(
        ctx,
        step_id,
        step_name,
        "Agent",
        Some(&error_output_var),
        Some(&workflow_inputs_var),
        None,
    );

    // Generate tracing span for OpenTelemetry
    let span_def = emit_agent_span_start(step_id, step_name, agent_id, capability_id);

    // Breakpoint check after input mapping — includes resolved inputs in the event
    let breakpoint_check = if step.breakpoint.unwrap_or(false) {
        emit_breakpoint_check(step_id, step_name, "Agent", ctx, Some(&step_inputs_var))
    } else {
        quote! {}
    };

    Ok(quote! {
        let #source_var = #build_source;
        let #step_inputs_var = ::runtara_workflow_stdlib::value_resolver::unwrap_top_level_immediate_envelopes(
            ::runtara_workflow_stdlib::value_resolver::resolve_nested_references(
                #base_inputs_code,
                &#source_var,
            )
        );

        // Breakpoint (after input mapping, before execution)
        #breakpoint_check

        // Define tracing span for this step
        #span_def

        // Wrap step execution in span scope
        let __step_result: std::result::Result<(), String> = __step_span.in_scope(|| {
            #debug_start

            #execute_capability

            match __cap_result {
                Ok(__cap_value) => {
                    let #result_var = __cap_value;

                    #debug_end

                    let #step_var =
                        __step_output_envelope(#step_id, #step_name_display, "Agent", &#result_var);
                    #steps_context.insert(#step_id.to_string(), #step_var.clone());

                    // Check for cancellation or pause via SDK signal polling
                    {
                        let mut __sdk = sdk().lock().unwrap();
                        if let Err(e) = __sdk.check_signals() {
                            return Err(format!("Step {}: {}", #step_id, e));
                        }
                    }

                    Ok(())
                }
                Err(__cap_err) => {
                    // Emit debug end with error info so failures are visible in the UI
                    let #error_output_var = __agent_error_output(&__cap_err);
                    #debug_end_error
                    Err(__cap_err)
                }
            }
        });

        // Propagate any error from the step
        if let Err(e) = __step_result {
            return Err(e);
        }
    })
}

/// Emit a durable capability call using #[resilient] macro.
#[allow(clippy::too_many_arguments)]
fn emit_durable_call(
    step_id: &str,
    agent_id: &str,
    capability_id: &str,
    inputs_var: &proc_macro2::Ident,
    durable_fn_name: &proc_macro2::Ident,
    connection_id: Option<&str>,
    ctx: &EmitContext,
    step_durable: bool,
    max_retries: u32,
    retry_delay: u64,
    rate_limit_budget: u64,
) -> TokenStream {
    // Static base for cache key - will be combined with loop indices at runtime
    let cache_key_base = format!("agent::{}::{}::{}", agent_id, capability_id, step_id);

    // Get the workflow inputs variable to access _loop_indices at runtime
    let workflow_inputs_var = ctx.inputs_var.clone();

    // Generate connection fetching code if connection_id is present and service URL is configured
    let (connection_fetch, final_inputs) = emit_connection_fetch(
        step_id,
        connection_id,
        ctx,
        inputs_var,
        false, // no rate limit handling
        agent_id,
        capability_id,
    );

    let max_retries_lit = max_retries;
    let retry_delay_lit = retry_delay;
    let rate_limit_budget_lit = rate_limit_budget;
    let durable_lit = step_durable;
    let use_shared_default_wrapper =
        step_durable && max_retries == 3 && retry_delay == 1000 && rate_limit_budget == 60_000;

    let durable_call = if use_shared_default_wrapper {
        quote! {
            // Call the shared default resilient function and wrap error with step context
            // AFTER retry decisions have been made.
            let __cap_result = __agent_durable_default(
                &__durable_cache_key,
                #final_inputs.clone(),
                #agent_id,
                #capability_id,
                #step_id,
            )
                .map_err(|e| format!("Step {} failed: Agent {}::{}: {}",
                    #step_id, #agent_id, #capability_id, e));
        }
    } else {
        quote! {
            // Define the resilient agent execution function with cancellation support.
            // The raw capability error is passed through (not wrapped) so that the
            // #[resilient] macro can parse JSON error category for retry decisions.
            #[resilient(durable = #durable_lit, max_retries = #max_retries_lit, delay = #retry_delay_lit, rate_limit_budget = #rate_limit_budget_lit)]
            fn #durable_fn_name(
                cache_key: &str,
                inputs: serde_json::Value,
                agent_id: &str,
                capability_id: &str,
                step_id: &str,
            ) -> std::result::Result<serde_json::Value, String> {
                __workflow_dispatch(agent_id, capability_id, inputs)
            }

            // Call the resilient function and wrap error with step context AFTER retry
            // decisions have been made (inside the resilient fn, errors are raw JSON)
            let __cap_result = #durable_fn_name(
                &__durable_cache_key,
                #final_inputs.clone(),
                #agent_id,
                #capability_id,
                #step_id,
            )
                .map_err(|e| format!("Step {} failed: Agent {}::{}: {}",
                    #step_id, #agent_id, #capability_id, e));
        }
    };

    quote! {
        // Build cache key dynamically, including prefix and loop indices
        let __durable_cache_key = {
            // Get prefix from parent context (set by EmbedWorkflow)
            let prefix = (*#workflow_inputs_var.variables)
                .as_object()
                .and_then(|vars| vars.get("_cache_key_prefix"))
                .and_then(|v| v.as_str())
                .unwrap_or("");

            let base = #cache_key_base;
            let indices_suffix = (*#workflow_inputs_var.variables)
                .as_object()
                .and_then(|vars| vars.get("_loop_indices"))
                .and_then(|v| v.as_array())
                .filter(|arr| !arr.is_empty())
                .map(|arr| {
                    let indices: Vec<String> = arr.iter()
                        .map(|v| v.to_string())
                        .collect();
                    format!("::[{}]", indices.join(","))
                })
                .unwrap_or_default();

            if prefix.is_empty() {
                // No cache prefix - use _workflow_id to prevent collisions between
                // independent workflows running the same agent steps
                let workflow_id = (*#workflow_inputs_var.variables)
                    .as_object()
                    .and_then(|vars| vars.get("_workflow_id"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("root");
                format!("{}::{}{}", workflow_id, base, indices_suffix)
            } else {
                format!("{}::{}{}", prefix, base, indices_suffix)
            }
        };

        #connection_fetch

        #durable_call
    }
}

/// Emit a durable and rate-limited capability call (for HTTP/external APIs).
#[allow(clippy::too_many_arguments)]
fn emit_durable_rate_limited_call(
    step_id: &str,
    agent_id: &str,
    capability_id: &str,
    inputs_var: &proc_macro2::Ident,
    durable_fn_name: &proc_macro2::Ident,
    connection_id: Option<&str>,
    ctx: &EmitContext,
    step_durable: bool,
    max_retries: u32,
    retry_delay: u64,
    rate_limit_budget: u64,
) -> TokenStream {
    // Static base for cache key - will be combined with loop indices at runtime
    let cache_key_base = format!("agent::{}::{}::{}", agent_id, capability_id, step_id);

    // Get the workflow inputs variable to access _loop_indices at runtime
    let workflow_inputs_var = ctx.inputs_var.clone();

    // Generate connection fetching code with rate limit handling
    let (connection_fetch, final_inputs) = emit_connection_fetch(
        step_id,
        connection_id,
        ctx,
        inputs_var,
        true, // with rate limit handling
        agent_id,
        capability_id,
    );

    let max_retries_lit = max_retries;
    let retry_delay_lit = retry_delay;
    let rate_limit_budget_lit = rate_limit_budget;
    let durable_lit = step_durable;
    let use_shared_default_wrapper =
        step_durable && max_retries == 5 && retry_delay == 2000 && rate_limit_budget == 60_000;

    let durable_call = if use_shared_default_wrapper {
        quote! {
            // Call the shared default rate-limited resilient function and wrap error
            // with step context AFTER retry decisions have been made.
            let __cap_result = __agent_durable_rate_limited_default(
                &__durable_cache_key,
                #final_inputs.clone(),
                #agent_id,
                #capability_id,
                #step_id,
            )
                .map_err(|e| format!("Step {} failed: Agent {}::{}: {}",
                    #step_id, #agent_id, #capability_id, e));
        }
    } else {
        quote! {
            // Define the resilient agent execution function (rate-limited) with cancellation support.
            // The raw capability error is passed through (not wrapped) so that the
            // #[resilient] macro can parse JSON error category for retry decisions.
            #[resilient(durable = #durable_lit, max_retries = #max_retries_lit, delay = #retry_delay_lit, rate_limit_budget = #rate_limit_budget_lit)]
            fn #durable_fn_name(
                cache_key: &str,
                inputs: serde_json::Value,
                agent_id: &str,
                capability_id: &str,
                step_id: &str,
            ) -> std::result::Result<serde_json::Value, String> {
                __workflow_dispatch(agent_id, capability_id, inputs)
            }

            // Call the resilient function and wrap error with step context AFTER retry
            // decisions have been made (inside the resilient fn, errors are raw JSON)
            let __cap_result = #durable_fn_name(
                &__durable_cache_key,
                #final_inputs.clone(),
                #agent_id,
                #capability_id,
                #step_id,
            )
                .map_err(|e| format!("Step {} failed: Agent {}::{}: {}",
                    #step_id, #agent_id, #capability_id, e));
        }
    };

    quote! {
        // Build cache key dynamically, including prefix and loop indices
        let __durable_cache_key = {
            // Get prefix from parent context (set by EmbedWorkflow)
            let prefix = (*#workflow_inputs_var.variables)
                .as_object()
                .and_then(|vars| vars.get("_cache_key_prefix"))
                .and_then(|v| v.as_str())
                .unwrap_or("");

            let base = #cache_key_base;
            let indices_suffix = (*#workflow_inputs_var.variables)
                .as_object()
                .and_then(|vars| vars.get("_loop_indices"))
                .and_then(|v| v.as_array())
                .filter(|arr| !arr.is_empty())
                .map(|arr| {
                    let indices: Vec<String> = arr.iter()
                        .map(|v| v.to_string())
                        .collect();
                    format!("::[{}]", indices.join(","))
                })
                .unwrap_or_default();

            if prefix.is_empty() {
                // No cache prefix - use _workflow_id to prevent collisions between
                // independent workflows running the same agent steps
                let workflow_id = (*#workflow_inputs_var.variables)
                    .as_object()
                    .and_then(|vars| vars.get("_workflow_id"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("root");
                format!("{}::{}{}", workflow_id, base, indices_suffix)
            } else {
                format!("{}::{}{}", prefix, base, indices_suffix)
            }
        };

        #connection_fetch

        #durable_call
    }
}

/// Inject connection_id into agent inputs for proxy-based credential resolution.
///
/// Returns a tuple of (injection_code, final_inputs_ident).
/// If no connection_id, returns empty code and the original inputs var.
/// Credentials are resolved server-side by the HTTP proxy using the connection_id.
fn emit_connection_fetch(
    _step_id: &str,
    connection_id: Option<&str>,
    _ctx: &EmitContext,
    inputs_var: &proc_macro2::Ident,
    _with_rate_limit_handling: bool,
    _agent_id: &str,
    _capability_id: &str,
) -> (TokenStream, proc_macro2::Ident) {
    // If no connection_id, just use original inputs
    let Some(conn_id) = connection_id else {
        return (quote! {}, inputs_var.clone());
    };

    let final_inputs = proc_macro2::Ident::new(
        &format!("{}_with_conn", inputs_var),
        proc_macro2::Span::call_site(),
    );

    // Inject both connection_id and _connection with connection_id populated.
    // Agents use _connection.connection_id to set X-Runtara-Connection-Id header,
    // and the proxy resolves credentials server-side.
    let code = quote! {
        let #final_inputs = {
            let mut inputs = #inputs_var.clone();
            if let serde_json::Value::Object(ref mut map) = inputs {
                map.insert("connection_id".to_string(), serde_json::Value::String(#conn_id.to_string()));
                map.insert("_connection".to_string(), serde_json::json!({
                    "connection_id": #conn_id,
                    "integration_id": "",
                    "parameters": {}
                }));
            }
            inputs
        };
    };

    (code, final_inputs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codegen::ast::context::EmitContext;
    use runtara_dsl::{ImmediateValue, MappingValue, ReferenceValue};
    use std::collections::HashMap;

    /// Helper to create a minimal agent step for testing.
    fn create_agent_step(step_id: &str, agent_id: &str, capability_id: &str) -> AgentStep {
        AgentStep {
            id: step_id.to_string(),
            name: Some("Test Agent Step".to_string()),
            agent_id: agent_id.to_string(),
            capability_id: capability_id.to_string(),
            connection_id: None,
            input_mapping: None,
            max_retries: None,
            retry_delay: None,
            timeout: None,
            compensation: None,
            breakpoint: None,
            durable: None,
        }
    }

    #[test]
    fn test_emit_agent_basic_structure() {
        let mut ctx = EmitContext::new(false);
        let step = create_agent_step("agent-basic", "utils", "random-double");

        let tokens = emit(&step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Verify basic structure
        assert!(
            code.contains("__durable_cache_key"),
            "Should build durable cache key"
        );
        assert!(
            code.contains("__agent_durable_default"),
            "Default retry config should call the shared durable agent wrapper"
        );
        assert!(
            !code.contains("# [resilient") && !code.contains("#[resilient"),
            "Default retry config should not emit a per-step resilient function"
        );
    }

    #[test]
    fn test_emit_agent_durable_true_when_workflow_and_step_both_durable() {
        let mut ctx = EmitContext::new(false); // ctx.durable defaults to true
        let step = create_agent_step("a", "utils", "random-double");
        let code = emit(&step, &mut ctx).unwrap().to_string();
        assert!(
            code.contains("__agent_durable_default"),
            "durable-by-default workflow with default retries should use shared durable wrapper, got:\n{}",
            code
        );
    }

    #[test]
    fn test_emit_agent_non_durable_workflow_forces_durable_false() {
        let mut ctx = EmitContext::new(false);
        ctx.durable = false;
        let mut step = create_agent_step("a", "utils", "random-double");
        // Even if step explicitly asks for durable, workflow-level wins.
        step.durable = Some(true);
        let code = emit(&step, &mut ctx).unwrap().to_string();
        assert!(
            code.contains("durable = false"),
            "workflow durable=false must override step durable=true"
        );
    }

    #[test]
    fn test_emit_agent_step_level_opt_out_within_durable_workflow() {
        let mut ctx = EmitContext::new(false); // ctx.durable = true
        let mut step = create_agent_step("a", "utils", "random-double");
        step.durable = Some(false);
        let code = emit(&step, &mut ctx).unwrap().to_string();
        assert!(
            code.contains("durable = false"),
            "step-level durable=false should propagate into the resilient attr"
        );
    }

    #[test]
    fn test_emit_agent_ids_in_cache_key() {
        let mut ctx = EmitContext::new(false);
        let step = create_agent_step("my-agent-step", "http", "http-request");

        let tokens = emit(&step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Cache key is a string literal - TokenStream should preserve it
        // The format is "agent::agent_id::capability_id::step_id"
        assert!(
            code.contains("agent::http::http-request::my-agent-step"),
            "Cache key should include agent::agent_id::capability_id::step_id"
        );
    }

    #[test]
    fn test_emit_agent_default_retry_config() {
        let mut ctx = EmitContext::new(false);
        let step = create_agent_step("agent-retry", "utils", "concat");

        let tokens = emit(&step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Default values: max_retries = 3, retry_delay = 1000
        assert!(
            code.contains("__agent_durable_default"),
            "Default retry config should use shared durable wrapper"
        );
    }

    #[test]
    fn test_emit_agent_custom_retry_config() {
        let mut ctx = EmitContext::new(false);
        let step = AgentStep {
            id: "agent-custom-retry".to_string(),
            name: Some("Custom Retry".to_string()),
            agent_id: "http".to_string(),
            capability_id: "http-request".to_string(),
            connection_id: None,
            input_mapping: None,
            max_retries: Some(5),
            retry_delay: Some(2000),
            timeout: None,
            compensation: None,
            breakpoint: None,
            durable: None,
        };

        let tokens = emit(&step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Custom values
        assert!(
            code.contains("max_retries = 5u32"),
            "Should use custom max_retries = 5"
        );
        assert!(
            code.contains("delay = 2000u64"),
            "Should use custom retry_delay = 2000"
        );
    }

    #[test]
    fn test_emit_agent_invokes_unwrap_top_level_immediate_envelopes() {
        // Pin the runtime call sequence the agent step body emits:
        //   1. resolve_nested_references(base_inputs, &source)
        //   2. unwrap_top_level_immediate_envelopes(...)
        // Workflows whose binaries lack the unwrap call hit
        // INPUT_DESERIALIZATION_ERROR for typed input fields like
        // `condition: Option<ConditionExpression>` when the user wraps the
        // condition in `valueType: "immediate"` per the inputMapping
        // contract.
        let mut ctx = EmitContext::new(false);
        let step = AgentStep {
            id: "retr_fts".to_string(),
            name: Some("Retrieve FTS".to_string()),
            agent_id: "object_model".to_string(),
            capability_id: "query-instances".to_string(),
            connection_id: None,
            input_mapping: Some(HashMap::new()),
            max_retries: None,
            retry_delay: None,
            timeout: None,
            compensation: None,
            breakpoint: None,
            durable: None,
        };
        let tokens = emit(&step, &mut ctx).unwrap();
        let code = tokens.to_string();
        assert!(
            code.contains("unwrap_top_level_immediate_envelopes"),
            "agent step body must invoke unwrap_top_level_immediate_envelopes; got: {}",
            code
        );
        assert!(
            code.contains("resolve_nested_references"),
            "agent step body must invoke resolve_nested_references; got: {}",
            code
        );
    }

    #[test]
    fn test_emit_agent_with_input_mapping() {
        let mut ctx = EmitContext::new(false);
        let mut input_mapping = HashMap::new();
        input_mapping.insert(
            "url".to_string(),
            MappingValue::Immediate(ImmediateValue {
                value: serde_json::json!("https://example.com"),
            }),
        );

        let step = AgentStep {
            id: "agent-mapped".to_string(),
            name: Some("With Mapping".to_string()),
            agent_id: "http".to_string(),
            capability_id: "http-request".to_string(),
            connection_id: None,
            input_mapping: Some(input_mapping),
            max_retries: None,
            retry_delay: None,
            timeout: None,
            compensation: None,
            breakpoint: None,
            durable: None,
        };

        let tokens = emit(&step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Should not use empty object
        assert!(
            !code.contains("Value :: Object (serde_json :: Map :: new ())")
                || code.contains("map.insert"),
            "Should have input mapping code instead of empty object"
        );
    }

    #[test]
    fn test_emit_agent_empty_input_mapping() {
        let mut ctx = EmitContext::new(false);
        let step = AgentStep {
            id: "agent-empty-map".to_string(),
            name: Some("Empty Mapping".to_string()),
            agent_id: "utils".to_string(),
            capability_id: "noop".to_string(),
            connection_id: None,
            input_mapping: Some(HashMap::new()), // Empty map
            max_retries: None,
            retry_delay: None,
            timeout: None,
            compensation: None,
            breakpoint: None,
            durable: None,
        };

        let tokens = emit(&step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Should use empty object for empty input mapping
        assert!(
            code.contains("serde_json :: Value :: Object (serde_json :: Map :: new ())"),
            "Should create empty object for empty input mapping"
        );
    }

    #[test]
    fn test_emit_agent_with_connection_id() {
        let mut ctx = EmitContext::new(false);
        let step = AgentStep {
            id: "agent-conn".to_string(),
            name: Some("With Connection".to_string()),
            agent_id: "http".to_string(),
            capability_id: "http-request".to_string(),
            connection_id: Some("my-connection".to_string()),
            input_mapping: None,
            max_retries: None,
            retry_delay: None,
            timeout: None,
            compensation: None,
            breakpoint: None,
            durable: None,
        };

        let tokens = emit(&step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Should inject connection_id into inputs for proxy-based credential resolution
        assert!(
            code.contains("connection_id"),
            "Should inject connection_id into inputs"
        );
        assert!(
            code.contains("\"my-connection\""),
            "Should include connection ID value"
        );
    }

    #[test]
    fn test_emit_agent_loop_indices_in_cache_key() {
        let mut ctx = EmitContext::new(false);
        let step = create_agent_step("agent-loop", "utils", "random-double");

        let tokens = emit(&step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Verify loop indices handling for cache key uniqueness
        assert!(
            code.contains("_loop_indices"),
            "Should check for _loop_indices in variables"
        );
        assert!(
            code.contains("indices_suffix"),
            "Should build indices suffix"
        );
    }

    #[test]
    fn test_emit_agent_output_structure() {
        let mut ctx = EmitContext::new(false);
        let step = create_agent_step("agent-output", "transform", "flatten");

        let tokens = emit(&step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Verify output JSON structure is built via shared helper.
        assert!(
            code.contains("__step_output_envelope"),
            "Should build output envelope"
        );
        assert!(code.contains("\"Agent\""), "Should have stepType = Agent");
    }

    #[test]
    fn test_emit_agent_stores_in_steps_context() {
        let mut ctx = EmitContext::new(false);
        let step = create_agent_step("agent-store", "utils", "concat");

        let tokens = emit(&step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Verify result is stored in steps_context
        assert!(
            code.contains("steps_context . insert"),
            "Should store result in steps_context"
        );
    }

    #[test]
    fn test_emit_agent_signal_check() {
        let mut ctx = EmitContext::new(false);
        let step = create_agent_step("agent-cancel", "utils", "noop");

        let tokens = emit(&step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Verify signal check (cancel/pause) after execution
        assert!(
            code.contains("check_signals"),
            "Should check for signals (cancel/pause) after step"
        );
        assert!(
            code.contains("sdk ()"),
            "Should acquire SDK lock for signal check"
        );
    }

    #[test]
    fn test_emit_agent_track_events_enabled() {
        let mut ctx = EmitContext::new(true); // debug mode ON
        let step = create_agent_step("agent-debug", "utils", "noop");

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
    fn test_emit_agent_track_events_disabled() {
        let mut ctx = EmitContext::new(false); // debug mode OFF
        let step = create_agent_step("agent-no-debug", "utils", "noop");

        let tokens = emit(&step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Core agent logic should still be present
        assert!(
            code.contains("__agent_durable_default"),
            "Should have capability execution via shared durable wrapper"
        );
        assert!(
            code.contains("__durable_cache_key"),
            "Should have durable cache key"
        );
    }

    #[test]
    fn test_emit_agent_with_unnamed_step() {
        let mut ctx = EmitContext::new(false);
        let step = AgentStep {
            id: "agent-unnamed".to_string(),
            name: None, // No name
            agent_id: "utils".to_string(),
            capability_id: "noop".to_string(),
            connection_id: None,
            input_mapping: None,
            max_retries: None,
            retry_delay: None,
            timeout: None,
            compensation: None,
            breakpoint: None,
            durable: None,
        };

        let tokens = emit(&step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Should use "Unnamed" as display name
        assert!(
            code.contains("\"Unnamed\""),
            "Should use 'Unnamed' for unnamed steps"
        );
    }

    #[test]
    fn test_emit_agent_durable_function_definition() {
        let mut ctx = EmitContext::new(false);
        let mut step = create_agent_step("agent-durable", "utils", "concat");
        step.max_retries = Some(4);

        let tokens = emit(&step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Verify durable function signature
        assert!(code.contains("fn "), "Should define durable function");
        assert!(
            code.contains("cache_key : & str"),
            "Durable function should take cache_key"
        );
        assert!(
            code.contains("-> std :: result :: Result < serde_json :: Value , String >"),
            "Should return Result<Value, String>"
        );
    }

    #[test]
    fn test_emit_agent_error_formatting() {
        let mut ctx = EmitContext::new(false);
        let step = create_agent_step("agent-error", "http", "http-request");

        let tokens = emit(&step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Verify error message includes context
        assert!(
            code.contains("Step {} failed: Agent"),
            "Error message should include step context"
        );
    }

    #[test]
    fn test_emit_agent_with_reference_input() {
        let mut ctx = EmitContext::new(false);
        let mut input_mapping = HashMap::new();
        input_mapping.insert(
            "data".to_string(),
            MappingValue::Reference(ReferenceValue {
                value: "steps.previous.outputs.result".to_string(),
                type_hint: None,
                default: None,
            }),
        );

        let step = AgentStep {
            id: "agent-ref".to_string(),
            name: Some("Reference Input".to_string()),
            agent_id: "transform".to_string(),
            capability_id: "flatten".to_string(),
            connection_id: None,
            input_mapping: Some(input_mapping),
            max_retries: None,
            retry_delay: None,
            timeout: None,
            compensation: None,
            breakpoint: None,
            durable: None,
        };

        let tokens = emit(&step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Should use mapping code (resolve_path is called during mapping)
        // The reference path is used in resolve_path call
        assert!(
            code.contains("resolve_path") || code.contains("data"),
            "Should have mapping code that references input data"
        );
    }

    #[test]
    fn test_needs_rate_limiting_unknown_capability() {
        // Unknown capabilities should not require rate limiting by default
        let result = needs_rate_limiting("nonexistent", "fake-capability");
        assert!(
            !result,
            "Unknown capabilities should default to no rate limiting"
        );
    }

    // =============================================================================
    // Cache key prefix tests
    // =============================================================================

    #[test]
    fn test_emit_agent_includes_cache_key_prefix() {
        let mut ctx = EmitContext::new(false);
        let step = create_agent_step("step-1", "http", "request");

        let tokens = emit(&step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Verify prefix is read from variables
        assert!(
            code.contains("_cache_key_prefix"),
            "Agent cache key must check for _cache_key_prefix"
        );
    }

    #[test]
    fn test_emit_agent_cache_key_uses_prefix_format() {
        let mut ctx = EmitContext::new(false);
        let step = create_agent_step("step-1", "http", "request");

        let tokens = emit(&step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Verify the conditional prefix format is used
        // When prefix is empty: format!("{}{}", base, indices_suffix)
        // When prefix is present: format!("{}::{}{}", prefix, base, indices_suffix)
        assert!(
            code.contains("prefix . is_empty ()"),
            "Should check if prefix is empty"
        );
        assert!(
            code.contains("\"{}::{}{}\""),
            "Should format with prefix when present"
        );
    }

    // =============================================================================
    // Cache key collision prevention tests (workflow_id usage)
    // =============================================================================

    #[test]
    fn test_emit_agent_uses_workflow_id_when_no_prefix() {
        // Verifies that Agent step uses _workflow_id when _cache_key_prefix is empty
        // This prevents cache collisions between independent workflows
        let mut ctx = EmitContext::new(false);
        let step = create_agent_step("check-file", "sftp", "exists");

        let tokens = emit(&step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // When prefix is empty, should read _workflow_id
        assert!(
            code.contains("_workflow_id"),
            "Should read _workflow_id when prefix is empty"
        );
        // Should fallback to "root" if _workflow_id is not set
        assert!(
            code.contains("unwrap_or (\"root\")"),
            "Should fallback to 'root' if no workflow_id"
        );
    }

    #[test]
    fn test_emit_agent_cache_key_format_with_workflow_id() {
        // Verifies the cache key format uses workflow_id when no prefix
        let mut ctx = EmitContext::new(false);
        let step = create_agent_step("my-step", "http", "request");

        let tokens = emit(&step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // The format should be: format!("{}::{}{}", workflow_id, base, indices_suffix)
        // This appears in the prefix.is_empty() branch
        assert!(
            code.contains("workflow_id")
                && code.contains("base")
                && code.contains("indices_suffix"),
            "Cache key should combine workflow_id, base, and indices_suffix"
        );
    }

    #[test]
    fn test_emit_agent_collision_prevention_structure() {
        // This test verifies all elements needed for collision prevention:
        // 1. Check for existing _cache_key_prefix
        // 2. If empty, use _workflow_id
        // 3. Format cache key with proper uniqueness
        let mut ctx = EmitContext::new(false);
        let step = create_agent_step("process-data", "transform", "flatten");

        let tokens = emit(&step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // 1. Must check for prefix
        assert!(
            code.contains("_cache_key_prefix"),
            "Must check for _cache_key_prefix"
        );

        // 2. Must handle empty prefix case with workflow_id
        assert!(
            code.contains("prefix . is_empty ()") && code.contains("_workflow_id"),
            "Must use _workflow_id when prefix is empty"
        );

        // 3. Both branches should produce unique keys
        // - With prefix: "prefix::base::indices"
        // - Without prefix: "workflow_id::base::indices"
        let has_both_formats = code.contains("\"{}::{}{}\"");
        assert!(
            has_both_formats,
            "Should have proper format for cache key with workflow_id"
        );
    }

    #[test]
    fn test_emit_agent_connection_id_injection() {
        let mut ctx = EmitContext::new(false);
        let step = AgentStep {
            id: "agent-conn-ctx".to_string(),
            name: Some("With Context".to_string()),
            agent_id: "http".to_string(),
            capability_id: "http-request".to_string(),
            connection_id: Some("my-connection".to_string()),
            input_mapping: None,
            max_retries: None,
            retry_delay: None,
            timeout: None,
            compensation: None,
            breakpoint: None,
            durable: None,
        };

        let tokens = emit(&step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Proxy-based credential resolution: connection_id is injected into inputs
        assert!(
            code.contains("connection_id"),
            "Should inject connection_id into inputs"
        );
        assert!(
            code.contains("\"my-connection\""),
            "Should include the connection ID value"
        );
        assert!(
            code.contains("_with_conn"),
            "Should create inputs variant with connection"
        );
    }
}
