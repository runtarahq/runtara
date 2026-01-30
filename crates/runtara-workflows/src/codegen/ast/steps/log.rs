// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Log step emitter.
//!
//! The Log step emits custom log/debug events during workflow execution.
//! Events are stored in the instance_events table for debugging and observability.

use proc_macro2::TokenStream;
use quote::quote;

use super::super::CodegenError;
use super::super::context::EmitContext;
use super::super::mapping;
use super::{emit_step_span_end, emit_step_span_start};
use runtara_dsl::{LogLevel, LogStep};

/// Emit code for a Log step.
pub fn emit(step: &LogStep, ctx: &mut EmitContext) -> Result<TokenStream, CodegenError> {
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

    // Generate tracing span for OpenTelemetry
    let span_start = emit_step_span_start(step_id, step_name, "Log");
    let span_end = emit_step_span_end();

    Ok(quote! {
        let #source_var = #build_source;
        let #context_var = #context_code;

        // Start tracing span for this step
        #span_start

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

        // End tracing span
        #span_end
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codegen::ast::context::EmitContext;
    use runtara_dsl::{ImmediateValue, MappingValue};
    use std::collections::HashMap;

    /// Helper to create a minimal log step for testing.
    fn create_log_step(step_id: &str, level: LogLevel, message: &str) -> LogStep {
        LogStep {
            id: step_id.to_string(),
            name: Some("Test Log".to_string()),
            level,
            message: message.to_string(),
            context: None,
        }
    }

    #[test]
    fn test_emit_log_basic_structure() {
        let mut ctx = EmitContext::new(false);
        let step = create_log_step("log-basic", LogLevel::Info, "Hello world");

        let tokens = emit(&step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Verify basic structure
        assert!(
            code.contains("custom_event"),
            "Should emit custom event via SDK"
        );
        assert!(
            code.contains("workflow_log"),
            "Event type should be workflow_log"
        );
    }

    #[test]
    fn test_emit_log_level_debug() {
        let mut ctx = EmitContext::new(false);
        let step = create_log_step("log-debug", LogLevel::Debug, "Debug message");

        let tokens = emit(&step, &mut ctx).unwrap();
        let code = tokens.to_string();

        assert!(code.contains("\"debug\""), "Should have level = debug");
    }

    #[test]
    fn test_emit_log_level_info() {
        let mut ctx = EmitContext::new(false);
        let step = create_log_step("log-info", LogLevel::Info, "Info message");

        let tokens = emit(&step, &mut ctx).unwrap();
        let code = tokens.to_string();

        assert!(code.contains("\"info\""), "Should have level = info");
    }

    #[test]
    fn test_emit_log_level_warn() {
        let mut ctx = EmitContext::new(false);
        let step = create_log_step("log-warn", LogLevel::Warn, "Warning message");

        let tokens = emit(&step, &mut ctx).unwrap();
        let code = tokens.to_string();

        assert!(code.contains("\"warn\""), "Should have level = warn");
    }

    #[test]
    fn test_emit_log_level_error() {
        let mut ctx = EmitContext::new(false);
        let step = create_log_step("log-error", LogLevel::Error, "Error message");

        let tokens = emit(&step, &mut ctx).unwrap();
        let code = tokens.to_string();

        assert!(code.contains("\"error\""), "Should have level = error");
    }

    #[test]
    fn test_emit_log_includes_message() {
        let mut ctx = EmitContext::new(false);
        let step = create_log_step("log-msg", LogLevel::Info, "Test message content");

        let tokens = emit(&step, &mut ctx).unwrap();
        let code = tokens.to_string();

        assert!(
            code.contains("Test message content"),
            "Should include the log message"
        );
    }

    #[test]
    fn test_emit_log_includes_timestamp() {
        let mut ctx = EmitContext::new(false);
        let step = create_log_step("log-ts", LogLevel::Info, "message");

        let tokens = emit(&step, &mut ctx).unwrap();
        let code = tokens.to_string();

        assert!(
            code.contains("timestamp_ms"),
            "Should include timestamp_ms field"
        );
        assert!(
            code.contains("SystemTime :: now"),
            "Should use current system time"
        );
    }

    #[test]
    fn test_emit_log_with_context() {
        let mut ctx = EmitContext::new(false);
        let mut context = HashMap::new();
        context.insert(
            "user_id".to_string(),
            MappingValue::Immediate(ImmediateValue {
                value: serde_json::json!("user123"),
            }),
        );

        let step = LogStep {
            id: "log-ctx".to_string(),
            name: Some("With Context".to_string()),
            level: LogLevel::Info,
            message: "User action".to_string(),
            context: Some(context),
        };

        let tokens = emit(&step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Should have context mapping code
        assert!(
            code.contains("context"),
            "Should include context in payload"
        );
    }

    #[test]
    fn test_emit_log_empty_context() {
        let mut ctx = EmitContext::new(false);
        let step = LogStep {
            id: "log-empty-ctx".to_string(),
            name: Some("Empty Context".to_string()),
            level: LogLevel::Info,
            message: "message".to_string(),
            context: Some(HashMap::new()),
        };

        let tokens = emit(&step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Should use empty object for empty context
        assert!(
            code.contains("serde_json :: Value :: Object (serde_json :: Map :: new ())"),
            "Should create empty object for empty context"
        );
    }

    #[test]
    fn test_emit_log_output_structure() {
        let mut ctx = EmitContext::new(false);
        let step = create_log_step("log-output", LogLevel::Info, "message");

        let tokens = emit(&step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Verify output JSON structure
        assert!(code.contains("\"stepId\""), "Should include stepId");
        assert!(code.contains("\"stepName\""), "Should include stepName");
        assert!(code.contains("\"stepType\""), "Should include stepType");
        assert!(code.contains("\"Log\""), "Should have stepType = Log");
        assert!(code.contains("\"outputs\""), "Should include outputs");
    }

    #[test]
    fn test_emit_log_stores_in_steps_context() {
        let mut ctx = EmitContext::new(false);
        let step = create_log_step("log-store", LogLevel::Info, "message");

        let tokens = emit(&step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Verify result is stored in steps_context
        assert!(
            code.contains("steps_context . insert"),
            "Should store result in steps_context"
        );
    }

    #[test]
    fn test_emit_log_with_unnamed_step() {
        let mut ctx = EmitContext::new(false);
        let step = LogStep {
            id: "log-unnamed".to_string(),
            name: None, // No name
            level: LogLevel::Info,
            message: "unnamed log".to_string(),
            context: None,
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
    fn test_emit_log_sdk_call() {
        let mut ctx = EmitContext::new(false);
        let step = create_log_step("log-sdk", LogLevel::Info, "message");

        let tokens = emit(&step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Verify SDK custom_event call
        assert!(code.contains("sdk ()"), "Should acquire SDK lock");
        assert!(
            code.contains(". custom_event"),
            "Should call custom_event method"
        );
        assert!(
            code.contains("serde_json :: to_vec"),
            "Should serialize payload to bytes"
        );
    }
}
