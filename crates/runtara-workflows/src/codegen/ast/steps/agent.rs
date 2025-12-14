// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Agent step emitter.
//!
//! The Agent step executes an agent capability.
//! All agent capabilities are checkpointed via runtara-sdk for crash recovery.
//! Rate limiting is handled via connection service responses.

use proc_macro2::TokenStream;
use quote::quote;

use super::super::context::EmitContext;
use super::super::mapping;
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
    let step_name = step.name.as_deref().unwrap_or("Unnamed");
    let agent_id = &step.agent_id;
    let capability_id = &step.capability_id;

    // All capabilities are checkpointed for crash recovery.
    // Rate limiting is only applied to external API calls.
    let needs_rate_limit = needs_rate_limiting(agent_id, capability_id);

    // Do all mutable operations first
    let step_var = ctx.declare_step(step_id);
    let source_var = ctx.temp_var("source");
    let step_inputs_var = ctx.temp_var("step_inputs");
    let result_var = ctx.temp_var("result");

    // Clone immutable references
    let steps_context = ctx.steps_context_var.clone();

    // Build the source for input mapping
    let build_source = mapping::emit_build_source(ctx);

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

    // Generate the capability execution code - all capabilities are checkpointed via SDK
    // Connection fetching and rate limiting is handled within emit_durable_call when connection_id is present
    let execute_capability = if needs_rate_limit {
        // Checkpointing + rate limiting for external API calls
        emit_durable_rate_limited_call(
            step_id,
            agent_id,
            capability_id,
            &step_inputs_var,
            &result_var,
            step.connection_id.as_deref(),
            ctx,
        )
    } else {
        // Checkpointing only (all other capabilities)
        emit_durable_call(
            step_id,
            agent_id,
            capability_id,
            &step_inputs_var,
            &result_var,
            step.connection_id.as_deref(),
            ctx,
        )
    };

    // Use base_inputs_code directly - connection injection is handled inside the execution code
    let inputs_code = base_inputs_code;

    quote! {
        let #source_var = #build_source;
        let #step_inputs_var = #inputs_code;

        #execute_capability

        let #step_var = serde_json::json!({
            "stepId": #step_id,
            "stepName": #step_name,
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

/// Emit a durable capability call using runtara-sdk checkpoint API.
/// Uses the checkpoint pattern:
/// 1. Check for existing checkpoint (resume case)
/// 2. Execute capability if no checkpoint
/// 3. Save result as checkpoint
///
/// If connection_id is provided and connection_service_url is configured,
/// the connection will be fetched from the external service and injected into inputs.
fn emit_durable_call(
    step_id: &str,
    agent_id: &str,
    capability_id: &str,
    inputs_var: &proc_macro2::Ident,
    result_var: &proc_macro2::Ident,
    connection_id: Option<&str>,
    ctx: &EmitContext,
) -> TokenStream {
    let cache_key = format!("agent::{}::{}::{}", agent_id, capability_id, step_id);

    // Generate connection fetching code if connection_id is present and service URL is configured
    let (connection_fetch, final_inputs) = emit_connection_fetch(
        step_id,
        connection_id,
        ctx,
        inputs_var,
        false, // no rate limit handling
    );

    quote! {
        let #result_var = {
            let __sdk = sdk().lock().await;

            // Check for existing checkpoint (resume case)
            match __sdk.get_checkpoint(#cache_key).await {
                Ok(Some(cached_bytes)) => {
                    // Found cached result - deserialize and return
                    drop(__sdk);
                    match serde_json::from_slice::<serde_json::Value>(&cached_bytes) {
                        Ok(cached_value) => cached_value,
                        Err(e) => {
                            return Err(format!("Step {} failed to deserialize cached result: {}", #step_id, e));
                        }
                    }
                }
                Ok(None) => {
                    // No cached result - execute capability
                    drop(__sdk);

                    #connection_fetch

                    let result = registry::execute_capability(#agent_id, #capability_id, #final_inputs.clone())
                        .map_err(|e| format!("Step {} failed: Agent {}::{}: {}",
                            #step_id, #agent_id, #capability_id, e))?;

                    // Save result as checkpoint
                    let result_bytes = serde_json::to_vec(&result)
                        .map_err(|e| format!("Step {} failed to serialize result: {}", #step_id, e))?;

                    let __sdk = sdk().lock().await;
                    if let Err(e) = __sdk.checkpoint(#cache_key, &result_bytes).await {
                        eprintln!("WARN: Step {} checkpoint save failed: {}", #step_id, e);
                        // Continue even if checkpoint fails - result is still valid
                    }

                    result
                }
                Err(e) => {
                    // Checkpoint lookup error - log and continue with execution
                    eprintln!("WARN: Step {} checkpoint lookup failed: {}", #step_id, e);
                    drop(__sdk);

                    #connection_fetch

                    let result = registry::execute_capability(#agent_id, #capability_id, #final_inputs.clone())
                        .map_err(|e| format!("Step {} failed: Agent {}::{}: {}",
                            #step_id, #agent_id, #capability_id, e))?;
                    result
                }
            }
        };
    }
}

/// Emit a durable and rate-limited capability call (for HTTP/external APIs).
/// Same as emit_durable_call but with rate limiting via connection service.
fn emit_durable_rate_limited_call(
    step_id: &str,
    agent_id: &str,
    capability_id: &str,
    inputs_var: &proc_macro2::Ident,
    result_var: &proc_macro2::Ident,
    connection_id: Option<&str>,
    ctx: &EmitContext,
) -> TokenStream {
    let cache_key = format!("agent::{}::{}::{}", agent_id, capability_id, step_id);

    // Generate connection fetching code with rate limit handling
    let (connection_fetch, final_inputs) = emit_connection_fetch(
        step_id,
        connection_id,
        ctx,
        inputs_var,
        true, // with rate limit handling
    );

    quote! {
        let #result_var = {
            let __sdk = sdk().lock().await;

            // Check for existing checkpoint (resume case - skip rate limiting)
            match __sdk.get_checkpoint(#cache_key).await {
                Ok(Some(cached_bytes)) => {
                    // Found cached result - deserialize and return (no rate limiting needed)
                    drop(__sdk);
                    match serde_json::from_slice::<serde_json::Value>(&cached_bytes) {
                        Ok(cached_value) => cached_value,
                        Err(e) => {
                            return Err(format!("Step {} failed to deserialize cached result: {}", #step_id, e));
                        }
                    }
                }
                Ok(None) => {
                    drop(__sdk);

                    #connection_fetch

                    let result = registry::execute_capability(#agent_id, #capability_id, #final_inputs.clone())
                        .map_err(|e| format!("Step {} failed: Agent {}::{}: {}",
                            #step_id, #agent_id, #capability_id, e))?;

                    // Save result as checkpoint
                    let result_bytes = serde_json::to_vec(&result)
                        .map_err(|e| format!("Step {} failed to serialize result: {}", #step_id, e))?;

                    let __sdk = sdk().lock().await;
                    if let Err(e) = __sdk.checkpoint(#cache_key, &result_bytes).await {
                        eprintln!("WARN: Step {} checkpoint save failed: {}", #step_id, e);
                        // Continue even if checkpoint fails - result is still valid
                    }

                    result
                }
                Err(e) => {
                    // Checkpoint lookup error - log and continue with execution
                    eprintln!("WARN: Step {} checkpoint lookup failed: {}", #step_id, e);
                    drop(__sdk);

                    #connection_fetch

                    let result = registry::execute_capability(#agent_id, #capability_id, #final_inputs.clone())
                        .map_err(|e| format!("Step {} failed: Agent {}::{}: {}",
                            #step_id, #agent_id, #capability_id, e))?;
                    result
                }
            }
        };
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

    let Some(_service_url) = &ctx.connection_service_url else {
        // No service URL - just inject connection_id into inputs for legacy support
        let final_inputs = proc_macro2::Ident::new(
            &format!("{}_with_conn", inputs_var),
            proc_macro2::Span::call_site(),
        );
        let code = quote! {
            let #final_inputs = {
                let mut inputs = #inputs_var.clone();
                if let serde_json::Value::Object(ref mut map) = inputs {
                    map.insert("connection_id".to_string(), serde_json::Value::String(#conn_id.to_string()));
                }
                inputs
            };
        };
        return (code, final_inputs);
    };

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
                        CONNECTION_SERVICE_URL.expect("connection service URL"),
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
            let mut __conn_response = fetch_connection(
                CONNECTION_SERVICE_URL.expect("connection service URL"),
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
