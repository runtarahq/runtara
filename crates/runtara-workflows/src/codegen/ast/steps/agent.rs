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

    // Generate debug event emissions
    let debug_start = emit_step_debug_start(
        ctx,
        step_id,
        step_name,
        "Agent",
        Some(&step_inputs_var),
        input_mapping_json.as_deref(),
    );
    let debug_end = emit_step_debug_end(ctx, step_id, step_name, "Agent", Some(&result_var));

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
    let cache_key = format!("agent::{}::{}::{}", agent_id, capability_id, step_id);

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
            #cache_key,
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
    let cache_key = format!("agent::{}::{}::{}", agent_id, capability_id, step_id);

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
            #cache_key,
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
