// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Connection step emitter.
//!
//! Generates code to acquire a connection from the connection service.
//! Connection data is sensitive and:
//! - Never logged in debug events
//! - Never stored in checkpoints
//! - Can only be passed to secure agents (validated at compile time)

use proc_macro2::TokenStream;
use quote::quote;

use crate::codegen::ast::context::EmitContext;
use runtara_dsl::ConnectionStep;

/// Emit code for a Connection step.
///
/// The generated code:
/// 1. Fetches connection from external service
/// 2. Handles rate limiting with durable sleep
/// 3. Stores connection data in step outputs (NOT logged or checkpointed)
pub fn emit(step: &ConnectionStep, ctx: &mut EmitContext) -> TokenStream {
    let step_id = &step.id;
    let connection_id = &step.connection_id;
    let _integration_id = &step.integration_id;

    let result_var = ctx.temp_var("conn_result");
    let steps_context = ctx.steps_context_var.clone();

    // Check if connection service is configured
    let Some(_service_url) = &ctx.connection_service_url else {
        // No connection service URL - emit error
        return quote! {
            compile_error!("Connection step requires CONNECTION_SERVICE_URL to be configured");
        };
    };

    // Generate connection fetch code with rate limit handling
    // NOTE: We intentionally do NOT emit debug events for connection steps
    // to prevent sensitive data from being logged
    quote! {
        // Fetch connection from external service (no debug logging for security)
        let #result_var = {
            let mut __conn_response = fetch_connection(
                CONNECTION_SERVICE_URL.expect("connection service URL"),
                TENANT_ID,
                #connection_id
            ).map_err(|e| format!("Step {} failed to fetch connection {}: {}",
                #step_id, #connection_id, e))?;

            // Check rate limit state and wait if needed
            if let Some(ref rl) = __conn_response.rate_limit {
                if rl.is_limited {
                    let wait_duration = rl.wait_duration();

                    // Emit heartbeat while waiting to prevent timeout
                    {
                        let __sdk = sdk().lock().await;
                        __sdk.heartbeat().await
                            .map_err(|e| format!("Step {} heartbeat failed: {}", #step_id, e))?;
                    }

                    // Use durable sleep so we survive crashes while waiting
                    {
                        let __sdk = sdk().lock().await;
                        __sdk.durable_sleep(wait_duration).await
                            .map_err(|e| format!("Step {} rate limit sleep failed: {}", #step_id, e))?;
                    }

                    // Emit another heartbeat after sleep
                    {
                        let __sdk = sdk().lock().await;
                        __sdk.heartbeat().await
                            .map_err(|e| format!("Step {} heartbeat failed: {}", #step_id, e))?;
                    }

                    // Re-fetch connection after waiting
                    __conn_response = fetch_connection(
                        CONNECTION_SERVICE_URL.expect("connection service URL"),
                        TENANT_ID,
                        #connection_id
                    ).map_err(|e| format!("Step {} failed to re-fetch connection {}: {}",
                        #step_id, #connection_id, e))?;
                }
            }

            // Build connection output (sensitive - never logged)
            serde_json::json!({
                "parameters": __conn_response.parameters,
                "integration_id": __conn_response.integration_id,
                "connection_subtype": __conn_response.connection_subtype
            })
        };

        // Store in steps context (but NOT checkpointed or logged)
        #steps_context.insert(
            #step_id.to_string(),
            serde_json::json!({ "outputs": #result_var })
        );
    }
}
