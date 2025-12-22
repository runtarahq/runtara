// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Log step emitter.
//!
//! The Log step emits custom log/debug events during workflow execution.
//! Events are stored in the instance_events table for debugging and observability.

use proc_macro2::TokenStream;
use quote::quote;

use super::super::context::EmitContext;
use super::super::mapping;
use runtara_dsl::{LogLevel, LogStep};

/// Emit code for a Log step.
pub fn emit(step: &LogStep, ctx: &mut EmitContext) -> TokenStream {
    let step_id = &step.id;
    let step_name = step.name.as_deref();
    let step_name_display = step_name.unwrap_or("Unnamed");
    let message = &step.message;

    // Map log level to string
    let level_str = match step.level {
        LogLevel::Debug => "debug",
        LogLevel::Info => "info",
        LogLevel::Warn => "warn",
        LogLevel::Error => "error",
    };

    // Do all mutable operations first
    let step_var = ctx.declare_step(step_id);
    let source_var = ctx.temp_var("source");
    let context_var = ctx.temp_var("log_context");

    // Clone immutable references
    let steps_context = ctx.steps_context_var.clone();

    // Build the source for input mapping
    let build_source = mapping::emit_build_source(ctx);

    // Generate context mapping if present
    let context_code = if let Some(ref context_mapping) = step.context {
        if !context_mapping.is_empty() {
            let mapping_code = mapping::emit_input_mapping(context_mapping, ctx, &source_var);
            quote! { #mapping_code }
        } else {
            quote! { serde_json::Value::Object(serde_json::Map::new()) }
        }
    } else {
        quote! { serde_json::Value::Object(serde_json::Map::new()) }
    };

    quote! {
        let #source_var = #build_source;
        let #context_var = #context_code;

        // Emit log event via SDK custom_event
        {
            let __log_payload = serde_json::json!({
                "step_id": #step_id,
                "step_name": #step_name_display,
                "level": #level_str,
                "message": #message,
                "context": #context_var,
                "timestamp_ms": std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as i64)
                    .unwrap_or(0),
            });

            let __payload_bytes = serde_json::to_vec(&__log_payload).unwrap_or_default();
            let __sdk_guard = sdk().lock().await;
            let _ = __sdk_guard.custom_event("workflow_log", __payload_bytes).await;
        }

        let #step_var = serde_json::json!({
            "stepId": #step_id,
            "stepName": #step_name_display,
            "stepType": "Log",
            "outputs": {
                "level": #level_str,
                "message": #message
            }
        });

        #steps_context.insert(#step_id.to_string(), #step_var.clone());
    }
}
