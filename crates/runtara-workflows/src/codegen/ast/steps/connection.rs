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

use crate::codegen::ast::CodegenError;
use crate::codegen::ast::context::EmitContext;
use runtara_dsl::ConnectionStep;

/// Emit code for a Connection step.
///
/// The generated code:
/// 1. Fetches connection from external service
/// 2. Handles rate limiting with durable sleep
/// 3. Stores connection data in step outputs (NOT logged or checkpointed)
pub fn emit(step: &ConnectionStep, ctx: &mut EmitContext) -> Result<TokenStream, CodegenError> {
    let step_id = &step.id;
    let connection_id = &step.connection_id;
    let integration_id = &step.integration_id;

    let result_var = ctx.temp_var("conn_result");
    let steps_context = ctx.steps_context_var.clone();

    // Generate connection fetch code with rate limit handling
    // NOTE: We intentionally do NOT emit debug events for connection steps
    // to prevent sensitive data from being logged
    // CONNECTION_SERVICE_URL can be provided at compile-time or runtime via env var
    Ok(quote! {
        // Fetch connection from external service (no debug logging for security)
        let #result_var = {
            let __conn_service_url = get_connection_service_url()
                .ok_or_else(|| format!("Step {} requires CONNECTION_SERVICE_URL to be configured", #step_id))?;

            // Build request context for connection usage tracking
            let __conn_scenario_id = std::env::var("SCENARIO_ID").ok();
            let __conn_instance_id = std::env::var("RUNTARA_INSTANCE_ID").ok();
            let __conn_ctx = ConnectionRequestContext {
                tag: Some(#integration_id),
                step_id: Some(#step_id),
                scenario_id: __conn_scenario_id.as_deref(),
                instance_id: __conn_instance_id.as_deref(),
            };

            let mut __conn_response = fetch_connection(
                __conn_service_url,
                TENANT_ID,
                #connection_id,
                Some(&__conn_ctx)
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
                        __conn_service_url,
                        TENANT_ID,
                        #connection_id,
                        Some(&__conn_ctx)
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
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codegen::ast::context::EmitContext;

    /// Helper to create a minimal connection step for testing.
    fn create_connection_step(step_id: &str, connection_id: &str) -> ConnectionStep {
        ConnectionStep {
            id: step_id.to_string(),
            name: Some("Test Connection".to_string()),
            connection_id: connection_id.to_string(),
            integration_id: "bearer".to_string(),
        }
    }

    #[test]
    fn test_emit_connection_basic_structure() {
        let mut ctx = EmitContext::new(false);
        let step = create_connection_step("conn-basic", "my-api-conn");

        let tokens = emit(&step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Verify basic structure
        assert!(
            code.contains("fetch_connection"),
            "Should call fetch_connection"
        );
        assert!(
            code.contains("get_connection_service_url"),
            "Should get connection service URL"
        );
    }

    #[test]
    fn test_emit_connection_includes_connection_id() {
        let mut ctx = EmitContext::new(false);
        let step = create_connection_step("conn-id", "test-connection-123");

        let tokens = emit(&step, &mut ctx).unwrap();
        let code = tokens.to_string();

        assert!(
            code.contains("test-connection-123"),
            "Should include connection_id in fetch call"
        );
    }

    #[test]
    fn test_emit_connection_rate_limit_handling() {
        let mut ctx = EmitContext::new(false);
        let step = create_connection_step("conn-rl", "api-conn");

        let tokens = emit(&step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Verify rate limit handling
        assert!(code.contains("rate_limit"), "Should check rate_limit field");
        assert!(code.contains("is_limited"), "Should check is_limited flag");
        assert!(code.contains("wait_duration"), "Should get wait duration");
    }

    #[test]
    fn test_emit_connection_durable_sleep() {
        let mut ctx = EmitContext::new(false);
        let step = create_connection_step("conn-sleep", "api-conn");

        let tokens = emit(&step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Verify durable sleep for rate limiting
        assert!(
            code.contains("durable_sleep"),
            "Should use durable_sleep for rate limit wait"
        );
    }

    #[test]
    fn test_emit_connection_heartbeat() {
        let mut ctx = EmitContext::new(false);
        let step = create_connection_step("conn-hb", "api-conn");

        let tokens = emit(&step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Verify heartbeat calls during rate limit wait
        assert!(
            code.contains("heartbeat"),
            "Should emit heartbeat during wait"
        );
    }

    #[test]
    fn test_emit_connection_refetch_after_wait() {
        let mut ctx = EmitContext::new(false);
        let step = create_connection_step("conn-refetch", "api-conn");

        let tokens = emit(&step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Should re-fetch connection after rate limit wait
        // There should be multiple fetch_connection calls
        let fetch_count = code.matches("fetch_connection").count();
        assert!(
            fetch_count >= 2,
            "Should call fetch_connection at least twice (initial + re-fetch)"
        );
    }

    #[test]
    fn test_emit_connection_output_structure() {
        let mut ctx = EmitContext::new(false);
        let step = create_connection_step("conn-output", "api-conn");

        let tokens = emit(&step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Verify output includes connection fields
        assert!(
            code.contains("parameters"),
            "Output should include parameters"
        );
        assert!(
            code.contains("integration_id"),
            "Output should include integration_id"
        );
        assert!(
            code.contains("connection_subtype"),
            "Output should include connection_subtype"
        );
    }

    #[test]
    fn test_emit_connection_stores_in_steps_context() {
        let mut ctx = EmitContext::new(false);
        let step = create_connection_step("conn-store", "api-conn");

        let tokens = emit(&step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Verify result is stored in steps_context
        assert!(
            code.contains("steps_context . insert"),
            "Should store result in steps_context"
        );
        assert!(code.contains("\"conn-store\""), "Should use step_id as key");
    }

    #[test]
    fn test_emit_connection_no_debug_events() {
        let mut ctx = EmitContext::new(true); // debug mode ON
        let step = create_connection_step("conn-no-debug", "api-conn");

        let tokens = emit(&step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Connection steps should NOT emit debug events for security
        assert!(
            !code.contains("step_debug_start"),
            "Should NOT emit debug start event for security"
        );
        assert!(
            !code.contains("step_debug_end"),
            "Should NOT emit debug end event for security"
        );
    }

    #[test]
    fn test_emit_connection_error_handling() {
        let mut ctx = EmitContext::new(false);
        let step = create_connection_step("conn-error", "api-conn");

        let tokens = emit(&step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Verify error handling with step context
        assert!(code.contains("map_err"), "Should map errors with context");
        assert!(
            code.contains("failed to fetch connection"),
            "Error message should include context"
        );
    }

    #[test]
    fn test_emit_connection_tenant_id() {
        let mut ctx = EmitContext::new(false);
        let step = create_connection_step("conn-tenant", "api-conn");

        let tokens = emit(&step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Verify tenant ID is passed to fetch
        assert!(
            code.contains("TENANT_ID"),
            "Should pass TENANT_ID to fetch_connection"
        );
    }

    #[test]
    fn test_emit_connection_with_different_integration_types() {
        // Test with different integration types
        for integration in &["bearer", "api_key", "basic_auth", "sftp"] {
            let mut ctx = EmitContext::new(false);
            let step = ConnectionStep {
                id: format!("conn-{}", integration),
                name: Some(format!("{} Connection", integration)),
                connection_id: "test-conn".to_string(),
                integration_id: integration.to_string(),
            };

            let tokens = emit(&step, &mut ctx).unwrap();
            let code = tokens.to_string();

            // All integration types should generate similar code
            assert!(
                code.contains("fetch_connection"),
                "Should call fetch_connection for {} integration",
                integration
            );
        }
    }

    #[test]
    fn test_emit_connection_request_context() {
        let mut ctx = EmitContext::new(false);
        let step = create_connection_step("conn-ctx", "api-conn");

        let tokens = emit(&step, &mut ctx).unwrap();
        let code = tokens.to_string();

        assert!(
            code.contains("ConnectionRequestContext"),
            "Should create ConnectionRequestContext"
        );
        assert!(
            code.contains("SCENARIO_ID"),
            "Should read SCENARIO_ID env var"
        );
        assert!(
            code.contains("RUNTARA_INSTANCE_ID"),
            "Should read RUNTARA_INSTANCE_ID env var"
        );
    }

    #[test]
    fn test_emit_connection_context_uses_integration_id_as_tag() {
        let mut ctx = EmitContext::new(false);
        let step = ConnectionStep {
            id: "conn-tag".to_string(),
            name: Some("Tag Test".to_string()),
            connection_id: "test-conn".to_string(),
            integration_id: "shopify_graphql".to_string(),
        };

        let tokens = emit(&step, &mut ctx).unwrap();
        let code = tokens.to_string();

        assert!(
            code.contains("\"shopify_graphql\""),
            "Should use integration_id as tag value"
        );
    }

    #[test]
    fn test_emit_connection_context_passed_to_both_fetches() {
        let mut ctx = EmitContext::new(false);
        let step = create_connection_step("conn-ctx-both", "api-conn");

        let tokens = emit(&step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Both fetch_connection calls should include Some(& __conn_ctx)
        let ctx_count = code.matches("Some (& __conn_ctx)").count();
        assert!(
            ctx_count >= 2,
            "Should pass context to both fetch_connection calls (initial + re-fetch), found {}",
            ctx_count
        );
    }
}
