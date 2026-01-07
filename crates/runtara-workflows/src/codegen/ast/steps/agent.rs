// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Agent step emitter.
//!
//! The Agent step executes an agent capability.
//! All agent capabilities use #[durable] macro for checkpoint-based crash recovery.
//! Rate limiting is handled via connection service responses.

use proc_macro2::TokenStream;
use quote::quote;

use super::super::context::EmitContext;
use super::super::mapping;
use super::{emit_step_debug_end, emit_step_debug_start};
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
pub fn emit(step: &AgentStep, ctx: &mut EmitContext) -> TokenStream {
    let step_id = &step.id;
    let step_name = step.name.as_deref();
    let step_name_display = step_name.unwrap_or("Unnamed");
    let agent_id = &step.agent_id;
    let capability_id = &step.capability_id;

    // Get retry configuration with defaults
    let max_retries = step.max_retries.unwrap_or(3);
    let retry_delay = step.retry_delay.unwrap_or(1000);

    // All capabilities use #[durable] for crash recovery.
    // Rate limiting is only applied to external API calls.
    let needs_rate_limit = needs_rate_limiting(agent_id, capability_id);

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
    let execute_capability = if needs_rate_limit {
        emit_durable_rate_limited_call(
            step_id,
            agent_id,
            capability_id,
            &step_inputs_var,
            &result_var,
            &durable_fn_name,
            step.connection_id.as_deref(),
            ctx,
            max_retries,
            retry_delay,
        )
    } else {
        emit_durable_call(
            step_id,
            agent_id,
            capability_id,
            &step_inputs_var,
            &result_var,
            &durable_fn_name,
            step.connection_id.as_deref(),
            ctx,
            max_retries,
            retry_delay,
        )
    };

    // Clone scenario inputs var for debug events (to access _loop_indices)
    let scenario_inputs_var = ctx.inputs_var.clone();

    // Generate debug event emissions
    let debug_start = emit_step_debug_start(
        ctx,
        step_id,
        step_name,
        "Agent",
        Some(&step_inputs_var),
        input_mapping_json.as_deref(),
        Some(&scenario_inputs_var),
    );
    let debug_end = emit_step_debug_end(
        ctx,
        step_id,
        step_name,
        "Agent",
        Some(&result_var),
        Some(&scenario_inputs_var),
    );

    quote! {
        let #source_var = #build_source;
        let #step_inputs_var = #base_inputs_code;

        #debug_start

        #execute_capability

        #debug_end

        let #step_var = serde_json::json!({
            "stepId": #step_id,
            "stepName": #step_name_display,
            "stepType": "Agent",
            "outputs": #result_var
        });
        #steps_context.insert(#step_id.to_string(), #step_var.clone());

        // Check for cancellation via SDK checkpoint response
        {
            let mut __sdk = sdk().lock().await;
            if let Err(e) = __sdk.check_cancelled().await {
                return Err(format!("Step {} cancelled: {}", #step_id, e));
            }
        }
    }
}

/// Emit a durable capability call using #[durable] macro.
#[allow(clippy::too_many_arguments)]
fn emit_durable_call(
    step_id: &str,
    agent_id: &str,
    capability_id: &str,
    inputs_var: &proc_macro2::Ident,
    result_var: &proc_macro2::Ident,
    durable_fn_name: &proc_macro2::Ident,
    connection_id: Option<&str>,
    ctx: &EmitContext,
    max_retries: u32,
    retry_delay: u64,
) -> TokenStream {
    // Static base for cache key - will be combined with loop indices at runtime
    let cache_key_base = format!("agent::{}::{}::{}", agent_id, capability_id, step_id);

    // Get the scenario inputs variable to access _loop_indices at runtime
    let scenario_inputs_var = ctx.inputs_var.clone();

    // Generate connection fetching code if connection_id is present and service URL is configured
    let (connection_fetch, final_inputs) = emit_connection_fetch(
        step_id,
        connection_id,
        ctx,
        inputs_var,
        false, // no rate limit handling
    );

    let max_retries_lit = max_retries;
    let retry_delay_lit = retry_delay;

    quote! {
        // Build cache key dynamically, including loop indices if inside Split/While
        let __durable_cache_key = {
            let base = #cache_key_base;
            let indices_suffix = (*#scenario_inputs_var.variables)
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
            format!("{}{}", base, indices_suffix)
        };

        // Define the durable agent execution function
        #[durable(max_retries = #max_retries_lit, delay = #retry_delay_lit)]
        async fn #durable_fn_name(
            cache_key: &str,
            inputs: serde_json::Value,
            agent_id: &str,
            capability_id: &str,
            step_id: &str,
        ) -> std::result::Result<serde_json::Value, String> {
            let result = registry::execute_capability(agent_id, capability_id, inputs).await
                .map_err(|e| format!("Step {} failed: Agent {}::{}: {}",
                    step_id, agent_id, capability_id, e))?;
            Ok(result)
        }

        #connection_fetch

        let #result_var = #durable_fn_name(
            &__durable_cache_key,
            #final_inputs.clone(),
            #agent_id,
            #capability_id,
            #step_id,
        ).await?;
    }
}

/// Emit a durable and rate-limited capability call (for HTTP/external APIs).
#[allow(clippy::too_many_arguments)]
fn emit_durable_rate_limited_call(
    step_id: &str,
    agent_id: &str,
    capability_id: &str,
    inputs_var: &proc_macro2::Ident,
    result_var: &proc_macro2::Ident,
    durable_fn_name: &proc_macro2::Ident,
    connection_id: Option<&str>,
    ctx: &EmitContext,
    max_retries: u32,
    retry_delay: u64,
) -> TokenStream {
    // Static base for cache key - will be combined with loop indices at runtime
    let cache_key_base = format!("agent::{}::{}::{}", agent_id, capability_id, step_id);

    // Get the scenario inputs variable to access _loop_indices at runtime
    let scenario_inputs_var = ctx.inputs_var.clone();

    // Generate connection fetching code with rate limit handling
    let (connection_fetch, final_inputs) = emit_connection_fetch(
        step_id,
        connection_id,
        ctx,
        inputs_var,
        true, // with rate limit handling
    );

    let max_retries_lit = max_retries;
    let retry_delay_lit = retry_delay;

    quote! {
        // Build cache key dynamically, including loop indices if inside Split/While
        let __durable_cache_key = {
            let base = #cache_key_base;
            let indices_suffix = (*#scenario_inputs_var.variables)
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
            format!("{}{}", base, indices_suffix)
        };

        // Define the durable agent execution function (rate-limited)
        #[durable(max_retries = #max_retries_lit, delay = #retry_delay_lit)]
        async fn #durable_fn_name(
            cache_key: &str,
            inputs: serde_json::Value,
            agent_id: &str,
            capability_id: &str,
            step_id: &str,
        ) -> std::result::Result<serde_json::Value, String> {
            let result = registry::execute_capability(agent_id, capability_id, inputs).await
                .map_err(|e| format!("Step {} failed: Agent {}::{}: {}",
                    step_id, agent_id, capability_id, e))?;
            Ok(result)
        }

        #connection_fetch

        let #result_var = #durable_fn_name(
            &__durable_cache_key,
            #final_inputs.clone(),
            #agent_id,
            #capability_id,
            #step_id,
        ).await?;
    }
}

/// Emit code to fetch connection from external service and inject into inputs.
///
/// Returns a tuple of (connection_fetch_code, final_inputs_ident).
/// If no connection_id or no service URL, returns empty code and the original inputs var.
fn emit_connection_fetch(
    step_id: &str,
    connection_id: Option<&str>,
    ctx: &EmitContext,
    inputs_var: &proc_macro2::Ident,
    with_rate_limit_handling: bool,
) -> (TokenStream, proc_macro2::Ident) {
    // If no connection_id or no service URL configured, just use original inputs
    let Some(conn_id) = connection_id else {
        return (quote! {}, inputs_var.clone());
    };

    // connection_service_url must be configured when connection_id is specified
    // Generate code that checks at runtime (since URL can come from env var)
    let _ = &ctx.connection_service_url; // Acknowledge we checked it, but defer to runtime

    // Generate connection fetch code with rate limit handling
    let final_inputs = proc_macro2::Ident::new(
        &format!("{}_with_conn", inputs_var),
        proc_macro2::Span::call_site(),
    );

    let rate_limit_code = if with_rate_limit_handling {
        quote! {
            // Check rate limit state and wait if needed
            if let Some(ref rl) = __conn_response.rate_limit {
                if rl.is_limited {
                    let wait_duration = rl.wait_duration();
                    eprintln!("DEBUG: Step {} connection {} is rate limited, waiting {:?}",
                        #step_id, #conn_id, wait_duration);

                    // Use durable sleep so we survive crashes while waiting
                    {
                        let __sdk = sdk().lock().await;
                        __sdk.durable_sleep(wait_duration).await
                            .map_err(|e| format!("Step {} rate limit sleep failed: {}", #step_id, e))?;
                    }

                    // Re-fetch connection after waiting
                    __conn_response = fetch_connection(
                        __conn_service_url,
                        TENANT_ID,
                        #conn_id
                    ).map_err(|e| format!("Step {} failed to re-fetch connection {}: {}",
                        #step_id, #conn_id, e))?;
                }
            }
        }
    } else {
        quote! {}
    };

    let code = quote! {
        let #final_inputs = {
            // Fetch connection from external service
            let __conn_service_url = get_connection_service_url()
                .ok_or_else(|| format!("Step {} requires CONNECTION_SERVICE_URL to be configured", #step_id))?;
            let mut __conn_response = fetch_connection(
                __conn_service_url,
                TENANT_ID,
                #conn_id
            ).map_err(|e| format!("Step {} failed to fetch connection {}: {}",
                #step_id, #conn_id, e))?;

            #rate_limit_code

            // Inject connection parameters into inputs
            let mut inputs = #inputs_var.clone();
            if let serde_json::Value::Object(ref mut map) = inputs {
                map.insert("connection_id".to_string(), serde_json::Value::String(#conn_id.to_string()));
                map.insert("_connection".to_string(), serde_json::json!({
                    "parameters": __conn_response.parameters,
                    "integration_id": __conn_response.integration_id,
                    "connection_subtype": __conn_response.connection_subtype
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
        }
    }

    #[test]
    fn test_emit_agent_basic_structure() {
        let mut ctx = EmitContext::new(false);
        let step = create_agent_step("agent-basic", "utils", "random-double");

        let tokens = emit(&step, &mut ctx);
        let code = tokens.to_string();

        // Verify basic structure
        assert!(
            code.contains("__durable_cache_key"),
            "Should build durable cache key"
        );
        assert!(
            code.contains("# [durable") || code.contains("#[durable"),
            "Should use #[durable] macro"
        );
        assert!(
            code.contains("registry :: execute_capability"),
            "Should call registry::execute_capability"
        );
    }

    #[test]
    fn test_emit_agent_ids_in_cache_key() {
        let mut ctx = EmitContext::new(false);
        let step = create_agent_step("my-agent-step", "http", "http-request");

        let tokens = emit(&step, &mut ctx);
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

        let tokens = emit(&step, &mut ctx);
        let code = tokens.to_string();

        // Default values: max_retries = 3, retry_delay = 1000
        assert!(
            code.contains("max_retries = 3u32"),
            "Should use default max_retries = 3"
        );
        assert!(
            code.contains("delay = 1000u64"),
            "Should use default retry_delay = 1000"
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
        };

        let tokens = emit(&step, &mut ctx);
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
        };

        let tokens = emit(&step, &mut ctx);
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
        };

        let tokens = emit(&step, &mut ctx);
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
        };

        let tokens = emit(&step, &mut ctx);
        let code = tokens.to_string();

        // Should include connection fetching code
        assert!(
            code.contains("get_connection_service_url"),
            "Should fetch connection service URL"
        );
        assert!(
            code.contains("fetch_connection"),
            "Should call fetch_connection"
        );
        assert!(
            code.contains("\"my-connection\""),
            "Should include connection ID"
        );
        assert!(
            code.contains("_connection"),
            "Should inject _connection into inputs"
        );
    }

    #[test]
    fn test_emit_agent_loop_indices_in_cache_key() {
        let mut ctx = EmitContext::new(false);
        let step = create_agent_step("agent-loop", "utils", "random-double");

        let tokens = emit(&step, &mut ctx);
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

        let tokens = emit(&step, &mut ctx);
        let code = tokens.to_string();

        // Verify output JSON structure
        assert!(code.contains("\"stepId\""), "Should include stepId");
        assert!(code.contains("\"stepName\""), "Should include stepName");
        assert!(code.contains("\"stepType\""), "Should include stepType");
        assert!(code.contains("\"Agent\""), "Should have stepType = Agent");
        assert!(code.contains("\"outputs\""), "Should include outputs");
    }

    #[test]
    fn test_emit_agent_stores_in_steps_context() {
        let mut ctx = EmitContext::new(false);
        let step = create_agent_step("agent-store", "utils", "concat");

        let tokens = emit(&step, &mut ctx);
        let code = tokens.to_string();

        // Verify result is stored in steps_context
        assert!(
            code.contains("steps_context . insert"),
            "Should store result in steps_context"
        );
    }

    #[test]
    fn test_emit_agent_cancellation_check() {
        let mut ctx = EmitContext::new(false);
        let step = create_agent_step("agent-cancel", "utils", "noop");

        let tokens = emit(&step, &mut ctx);
        let code = tokens.to_string();

        // Verify cancellation check after execution
        assert!(
            code.contains("check_cancelled"),
            "Should check for cancellation after step"
        );
        assert!(
            code.contains("sdk ()"),
            "Should acquire SDK lock for cancellation check"
        );
    }

    #[test]
    fn test_emit_agent_debug_mode_enabled() {
        let mut ctx = EmitContext::new(true); // debug mode ON
        let step = create_agent_step("agent-debug", "utils", "noop");

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
    fn test_emit_agent_debug_mode_disabled() {
        let mut ctx = EmitContext::new(false); // debug mode OFF
        let step = create_agent_step("agent-no-debug", "utils", "noop");

        let tokens = emit(&step, &mut ctx);
        let code = tokens.to_string();

        // Core agent logic should still be present
        assert!(
            code.contains("execute_capability"),
            "Should have capability execution"
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
    fn test_emit_agent_durable_function_definition() {
        let mut ctx = EmitContext::new(false);
        let step = create_agent_step("agent-durable", "utils", "concat");

        let tokens = emit(&step, &mut ctx);
        let code = tokens.to_string();

        // Verify durable function signature
        assert!(
            code.contains("async fn"),
            "Should define async durable function"
        );
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

        let tokens = emit(&step, &mut ctx);
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
        };

        let tokens = emit(&step, &mut ctx);
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
}
