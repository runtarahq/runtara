// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Error step emitter.
//!
//! The Error step emits a structured error and terminates the workflow.
//! This is the primary mechanism for business logic errors that should
//! be distinguishable from technical errors.

use proc_macro2::TokenStream;
use quote::quote;

use super::super::CodegenError;
use super::super::context::EmitContext;
use super::super::mapping;
use super::{emit_step_debug_end, emit_step_debug_start, emit_step_span_start};
use runtara_dsl::{ErrorCategory, ErrorSeverity, ErrorStep};

/// Emit code for an Error step.
pub fn emit(step: &ErrorStep, ctx: &mut EmitContext) -> Result<TokenStream, CodegenError> {
    let step_id = &step.id;
    let step_name = step.name.as_deref();
    let step_name_display = step_name.unwrap_or("Unnamed");
    let error_code = &step.code;
    let error_message = &step.message;

    // Map category to string
    let category_str = match step.category {
        ErrorCategory::Transient => "transient",
        ErrorCategory::Permanent => "permanent",
    };

    // Map severity to string (default to "error" if not specified)
    let severity_str = match step.severity.unwrap_or_default() {
        ErrorSeverity::Info => "info",
        ErrorSeverity::Warning => "warning",
        ErrorSeverity::Error => "error",
        ErrorSeverity::Critical => "critical",
    };

    // Do all mutable operations first
    let step_var = ctx.declare_step(step_id);
    let source_var = ctx.temp_var("source");
    let context_var = ctx.temp_var("error_context");
    let error_output_var = ctx.temp_var("error_output");

    // Clone immutable references
    let steps_context = ctx.steps_context_var.clone();

    // Get the scenario inputs variable to access _loop_indices at runtime
    let scenario_inputs_var = ctx.inputs_var.clone();

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

    // Generate debug event emissions (Error doesn't create a scope)
    let debug_start = emit_step_debug_start(
        ctx,
        step_id,
        step_name,
        "Error",
        None, // no pre-computed inputs var
        None, // no input mapping JSON
        Some(&scenario_inputs_var),
        None, // no scope override
    );
    let debug_end = emit_step_debug_end(
        ctx,
        step_id,
        step_name,
        "Error",
        Some(&error_output_var),
        Some(&scenario_inputs_var),
        None,
    );

    // Generate tracing span for OpenTelemetry
    let span_def = emit_step_span_start(step_id, step_name, "Error");

    Ok(quote! {
        let #source_var = #build_source;
        let #context_var = #context_code;

        // Define tracing span for this step
        #span_def

        // Wrap step execution in async block instrumented with span
        // The async block returns the error string to propagate
        let __error_result: String = async {
            #debug_start

            // Emit structured error event via SDK custom_event
            {
                let __error_payload = serde_json::json!({
                    "step_id": #step_id,
                    "step_name": #step_name_display,
                    "category": #category_str,
                    "code": #error_code,
                    "message": #error_message,
                    "severity": #severity_str,
                    "context": #context_var,
                    "timestamp_ms": std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_millis() as i64)
                        .unwrap_or(0),
                });

                let __payload_bytes = serde_json::to_vec(&__error_payload).unwrap_or_default();
                let __sdk_guard = sdk().lock().await;
                let _ = __sdk_guard.custom_event("workflow_error", __payload_bytes).await;
            }

            // Store step result in context (even though we'll fail after)
            let #step_var = serde_json::json!({
                "stepId": #step_id,
                "stepName": #step_name_display,
                "stepType": "Error",
                "outputs": {
                    "category": #category_str,
                    "code": #error_code,
                    "message": #error_message,
                    "severity": #severity_str
                }
            });

            #steps_context.insert(#step_id.to_string(), #step_var.clone());

            // Build structured error message with context for the workflow failure
            let __error_context = serde_json::json!({
                "stepId": #step_id,
                "stepName": #step_name_display,
                "category": #category_str,
                "code": #error_code,
                "message": #error_message,
                "severity": #severity_str,
                "context": #context_var
            });

            // Emit debug end with error info so the step is visible in step summaries
            let #error_output_var = serde_json::json!({
                "_error": true,
                "category": #category_str,
                "code": #error_code,
                "message": #error_message,
                "severity": #severity_str
            });
            #debug_end

            // Return the error string from the async block
            serde_json::to_string(&__error_context).unwrap_or_else(|_| {
                format!("[{}] {}", #error_code, #error_message)
            })
        }.instrument(__step_span).await;

        // Terminate workflow with structured error
        return Err(__error_result);
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codegen::ast::context::EmitContext;
    use runtara_dsl::{ImmediateValue, MappingValue};
    use std::collections::HashMap;

    /// Helper to create a minimal error step for testing.
    fn create_error_step(
        step_id: &str,
        category: ErrorCategory,
        code: &str,
        message: &str,
    ) -> ErrorStep {
        ErrorStep {
            id: step_id.to_string(),
            name: Some("Test Error".to_string()),
            category,
            code: code.to_string(),
            message: message.to_string(),
            severity: None,
            context: None,
        }
    }

    #[test]
    fn test_emit_error_basic_structure() {
        let mut ctx = EmitContext::new(false);
        let step = create_error_step(
            "error-basic",
            ErrorCategory::Permanent,
            "INVALID_ORDER",
            "Order is invalid",
        );

        let tokens = emit(&step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Verify basic structure
        assert!(
            code.contains("custom_event"),
            "Should emit custom event via SDK"
        );
        assert!(
            code.contains("workflow_error"),
            "Event type should be workflow_error"
        );
        assert!(
            code.contains("return Err"),
            "Should return error to terminate workflow"
        );
        assert!(
            code.contains("serde_json :: to_string"),
            "Should serialize error context as JSON"
        );
    }

    #[test]
    fn test_emit_error_category_transient() {
        let mut ctx = EmitContext::new(false);
        let step = create_error_step(
            "error-transient",
            ErrorCategory::Transient,
            "TIMEOUT",
            "Request timed out",
        );

        let tokens = emit(&step, &mut ctx).unwrap();
        let code = tokens.to_string();

        assert!(
            code.contains("\"transient\""),
            "Should have category = transient"
        );
    }

    #[test]
    fn test_emit_error_category_permanent() {
        let mut ctx = EmitContext::new(false);
        let step = create_error_step(
            "error-permanent",
            ErrorCategory::Permanent,
            "NOT_FOUND",
            "Resource not found",
        );

        let tokens = emit(&step, &mut ctx).unwrap();
        let code = tokens.to_string();

        assert!(
            code.contains("\"permanent\""),
            "Should have category = permanent"
        );
    }

    #[test]
    fn test_emit_error_permanent_business_pattern() {
        // Business errors are now permanent errors with Warning severity
        let mut ctx = EmitContext::new(false);
        let step = ErrorStep {
            id: "error-business".to_string(),
            name: Some("Credit Limit Error".to_string()),
            category: ErrorCategory::Permanent,
            code: "CREDIT_LIMIT_EXCEEDED".to_string(),
            message: "Credit limit exceeded".to_string(),
            severity: Some(ErrorSeverity::Warning), // Warning = expected business outcome
            context: None,
        };

        let tokens = emit(&step, &mut ctx).unwrap();
        let code = tokens.to_string();

        assert!(
            code.contains("\"permanent\""),
            "Should have category = permanent"
        );
        assert!(
            code.contains("\"warning\""),
            "Should have severity = warning for business errors"
        );
    }

    #[test]
    fn test_emit_error_severity_info() {
        let mut ctx = EmitContext::new(false);
        let step = ErrorStep {
            id: "error-severity".to_string(),
            name: Some("Severity Test".to_string()),
            category: ErrorCategory::Permanent,
            code: "INFO_ERROR".to_string(),
            message: "Info level error".to_string(),
            severity: Some(ErrorSeverity::Info),
            context: None,
        };

        let tokens = emit(&step, &mut ctx).unwrap();
        let code = tokens.to_string();

        assert!(code.contains("\"info\""), "Should have severity = info");
    }

    #[test]
    fn test_emit_error_severity_warning() {
        let mut ctx = EmitContext::new(false);
        let step = ErrorStep {
            id: "error-warning".to_string(),
            name: Some("Warning Test".to_string()),
            category: ErrorCategory::Permanent,
            code: "WARN_ERROR".to_string(),
            message: "Warning level error".to_string(),
            severity: Some(ErrorSeverity::Warning),
            context: None,
        };

        let tokens = emit(&step, &mut ctx).unwrap();
        let code = tokens.to_string();

        assert!(
            code.contains("\"warning\""),
            "Should have severity = warning"
        );
    }

    #[test]
    fn test_emit_error_severity_critical() {
        let mut ctx = EmitContext::new(false);
        let step = ErrorStep {
            id: "error-critical".to_string(),
            name: Some("Critical Test".to_string()),
            category: ErrorCategory::Permanent,
            code: "CRIT_ERROR".to_string(),
            message: "Critical error".to_string(),
            severity: Some(ErrorSeverity::Critical),
            context: None,
        };

        let tokens = emit(&step, &mut ctx).unwrap();
        let code = tokens.to_string();

        assert!(
            code.contains("\"critical\""),
            "Should have severity = critical"
        );
    }

    #[test]
    fn test_emit_error_default_severity() {
        let mut ctx = EmitContext::new(false);
        let step = create_error_step(
            "error-default-severity",
            ErrorCategory::Permanent,
            "DEFAULT",
            "Default severity",
        );

        let tokens = emit(&step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Default severity is "error"
        assert!(
            code.contains("\"error\""),
            "Should default to severity = error"
        );
    }

    #[test]
    fn test_emit_error_includes_code_and_message() {
        let mut ctx = EmitContext::new(false);
        let step = create_error_step(
            "error-code-msg",
            ErrorCategory::Permanent,
            "MY_ERROR_CODE",
            "This is my error message",
        );

        let tokens = emit(&step, &mut ctx).unwrap();
        let code = tokens.to_string();

        assert!(code.contains("MY_ERROR_CODE"), "Should include error code");
        assert!(
            code.contains("This is my error message"),
            "Should include error message"
        );
    }

    #[test]
    fn test_emit_error_with_context() {
        let mut ctx = EmitContext::new(false);
        let mut context = HashMap::new();
        context.insert(
            "order_id".to_string(),
            MappingValue::Immediate(ImmediateValue {
                value: serde_json::json!("ORD-123"),
            }),
        );

        let step = ErrorStep {
            id: "error-ctx".to_string(),
            name: Some("With Context".to_string()),
            category: ErrorCategory::Permanent,
            code: "ORDER_ERROR".to_string(),
            message: "Order failed".to_string(),
            severity: None,
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
    fn test_emit_error_empty_context() {
        let mut ctx = EmitContext::new(false);
        let step = ErrorStep {
            id: "error-empty-ctx".to_string(),
            name: Some("Empty Context".to_string()),
            category: ErrorCategory::Permanent,
            code: "EMPTY_CTX".to_string(),
            message: "message".to_string(),
            severity: None,
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
    fn test_emit_error_returns_error_string() {
        let mut ctx = EmitContext::new(false);
        let step = create_error_step(
            "error-return",
            ErrorCategory::Permanent,
            "RETURN_ERROR",
            "Return test",
        );

        let tokens = emit(&step, &mut ctx).unwrap();
        let code = tokens.to_string();

        assert!(
            code.contains("return Err"),
            "Should return Err to terminate workflow"
        );
        assert!(
            code.contains("serde_json :: to_string (& __error_context)"),
            "Should serialize error context as JSON string"
        );
    }

    #[test]
    fn test_emit_error_stores_in_steps_context() {
        let mut ctx = EmitContext::new(false);
        let step = create_error_step(
            "error-store",
            ErrorCategory::Permanent,
            "STORE_ERROR",
            "Store test",
        );

        let tokens = emit(&step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Verify result is stored in steps_context before failing
        assert!(
            code.contains("steps_context . insert"),
            "Should store result in steps_context"
        );
    }

    #[test]
    fn test_emit_error_with_unnamed_step() {
        let mut ctx = EmitContext::new(false);
        let step = ErrorStep {
            id: "error-unnamed".to_string(),
            name: None, // No name
            category: ErrorCategory::Permanent,
            code: "UNNAMED".to_string(),
            message: "unnamed error".to_string(),
            severity: None,
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
    fn test_emit_error_sdk_call() {
        let mut ctx = EmitContext::new(false);
        let step = create_error_step(
            "error-sdk",
            ErrorCategory::Permanent,
            "SDK_ERROR",
            "SDK test",
        );

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

    #[test]
    fn test_emit_error_debug_events_in_debug_mode() {
        let mut ctx = EmitContext::new(true); // debug_mode = true
        let step = create_error_step(
            "error-debug",
            ErrorCategory::Permanent,
            "DEBUG_ERROR",
            "Debug test error",
        );

        let tokens = emit(&step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Error step must emit step_debug_start and step_debug_end events
        // just like all other step types (Agent, Finish, Conditional, etc.)
        assert!(
            code.contains("step_debug_start"),
            "Error step must emit step_debug_start in debug mode so it appears in step summaries"
        );
        assert!(
            code.contains("step_debug_end"),
            "Error step must emit step_debug_end in debug mode so it appears in step summaries"
        );
    }

    #[test]
    fn test_emit_error_no_debug_events_when_not_debug_mode() {
        let mut ctx = EmitContext::new(false); // debug_mode = false
        let step = create_error_step(
            "error-no-debug",
            ErrorCategory::Permanent,
            "NO_DEBUG",
            "No debug test",
        );

        let tokens = emit(&step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // When debug mode is off, no debug events should be emitted
        assert!(
            !code.contains("step_debug_start"),
            "Error step should not emit step_debug_start when debug mode is off"
        );
        assert!(
            !code.contains("step_debug_end"),
            "Error step should not emit step_debug_end when debug mode is off"
        );
    }

    #[test]
    fn test_emit_error_includes_timestamp() {
        let mut ctx = EmitContext::new(false);
        let step = create_error_step(
            "error-ts",
            ErrorCategory::Permanent,
            "TIMESTAMP",
            "Timestamp test",
        );

        let tokens = emit(&step, &mut ctx).unwrap();
        let code = tokens.to_string();

        assert!(
            code.contains("timestamp_ms"),
            "Should include timestamp_ms field"
        );
        assert!(
            code.contains("SystemTime :: now"),
            "Should use current time"
        );
    }
}
