// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! StartScenario step emitter.
//!
//! The StartScenario step executes a nested child scenario.
//! When a child scenario's ExecutionGraph is available in the EmitContext,
//! it will be recursively emitted and embedded into the parent scenario.
//! The entire child scenario result uses #[durable] macro for checkpoint-based recovery.

use proc_macro2::{Ident, TokenStream};
use quote::quote;

use super::super::CodegenError;
use super::super::context::EmitContext;
use super::super::mapping;
use super::super::program;
use super::{
    emit_child_scenario_span_start, emit_step_debug_end, emit_step_debug_start,
    emit_step_span_start,
};
use runtara_dsl::{ExecutionGraph, StartScenarioStep};

/// Emit code for a StartScenario step.
///
/// If the child scenario's ExecutionGraph is available in the EmitContext,
/// it will be recursively emitted as an embedded function. Otherwise,
/// returns a compilation error.
///
/// # Errors
///
/// Returns `CodegenError::MissingChildScenario` if the child scenario is not found
/// in the EmitContext. This ensures fail-fast at compile time rather than silent
/// runtime failures.
pub fn emit(step: &StartScenarioStep, ctx: &mut EmitContext) -> Result<TokenStream, CodegenError> {
    let step_id = &step.id;
    let child_scenario_id = &step.child_scenario_id;

    // Get retry configuration with defaults
    let max_retries = step.max_retries.unwrap_or(3);
    let retry_delay = step.retry_delay.unwrap_or(1000);

    // Look up the child scenario reference (scenario_id, version)
    let (scenario_id, version) = ctx.step_to_child_ref.get(step_id).cloned().ok_or_else(|| {
        CodegenError::MissingChildScenario {
            step_id: step_id.clone(),
            child_scenario_id: child_scenario_id.clone(),
        }
    })?;

    // Check if we have the child scenario's graph
    let child_graph = ctx
        .get_child_scenario(&scenario_id, version)
        .cloned()
        .ok_or_else(|| CodegenError::MissingChildScenario {
            step_id: step_id.clone(),
            child_scenario_id: child_scenario_id.clone(),
        })?;

    // Get or create shared function name (tracks deduplication)
    let (shared_fn_name, already_emitted) = ctx.get_or_create_child_fn(&scenario_id, version);

    emit_with_embedded_child(
        step,
        &child_graph,
        ctx,
        max_retries,
        retry_delay,
        &shared_fn_name,
        already_emitted,
    )
}

/// Emit a StartScenario step with an embedded child scenario.
///
/// If `already_emitted` is true, the shared child function was already emitted by
/// a previous StartScenario step, so we only emit the call-site-specific wrapper.
fn emit_with_embedded_child(
    step: &StartScenarioStep,
    child_graph: &ExecutionGraph,
    ctx: &mut EmitContext,
    max_retries: u32,
    retry_delay: u64,
    shared_fn_name: &Ident,
    already_emitted: bool,
) -> Result<TokenStream, CodegenError> {
    let step_id = &step.id;
    let step_name = step.name.as_deref();
    let step_name_display = step_name.unwrap_or("Unnamed");
    let child_scenario_id = &step.child_scenario_id;

    // Do all mutable operations first
    let step_var = ctx.declare_step(step_id);
    let source_var = ctx.temp_var("source");
    let child_inputs_var = ctx.temp_var("child_inputs");
    // Use the shared function name for deduplication
    let child_fn_name = shared_fn_name.clone();
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

    // Generate input mapping for child scenario
    let inputs_code = if let Some(ref input_mapping) = step.input_mapping {
        if !input_mapping.is_empty() {
            let mapping_code = mapping::emit_input_mapping(input_mapping, ctx, &source_var);
            quote! { #mapping_code }
        } else {
            quote! { serde_json::Value::Object(serde_json::Map::new()) }
        }
    } else {
        quote! { serde_json::Value::Object(serde_json::Map::new()) }
    };

    // Generate embedded schema for runtime validation (if child has required fields)
    let runtime_validation_code = {
        let required_fields: Vec<_> = child_graph
            .input_schema
            .iter()
            .filter(|(_, field)| field.required)
            .map(|(name, field)| {
                let name_str = name.as_str();
                let type_str = format!("{:?}", field.field_type);
                let desc = field.description.as_deref();
                let desc_token = if let Some(d) = desc {
                    quote! { Some(#d) }
                } else {
                    quote! { None }
                };
                quote! {
                    runtara_workflow_stdlib::RequiredField {
                        name: #name_str,
                        field_type: #type_str,
                        description: #desc_token,
                    }
                }
            })
            .collect();

        if required_fields.is_empty() {
            quote! {}
        } else {
            let step_id_str = step_id.as_str();
            let child_id_str = child_scenario_id.as_str();
            quote! {
                // Runtime validation of child inputs
                {
                    static CHILD_SCHEMA: runtara_workflow_stdlib::ChildInputSchema =
                        runtara_workflow_stdlib::ChildInputSchema {
                            required_fields: &[#(#required_fields),*],
                        };
                    runtara_workflow_stdlib::validate_child_inputs(
                        #step_id_str,
                        #child_id_str,
                        &child_inputs,
                        &CHILD_SCHEMA,
                    ).map_err(|e| e.to_string())?;
                }
            }
        }
    };

    // Only emit the shared child function if this is the first reference
    let child_fn_code = if already_emitted {
        // Function already emitted by a previous StartScenario step - just reference it
        quote! {}
    } else {
        // First reference - emit the shared function definition
        program::emit_graph_as_function(&child_fn_name, child_graph, ctx)?
    };

    // Get the scenario inputs variable to access _loop_indices at runtime
    let scenario_inputs_var = ctx.inputs_var.clone();

    // StartScenario creates a scope - use sc_{step_id} as its scope_id
    let start_scenario_scope_id = format!("sc_{}", step_id);

    // Generate debug event emissions with the StartScenario's own scope_id
    let debug_start = emit_step_debug_start(
        ctx,
        step_id,
        step_name,
        "StartScenario",
        Some(&child_inputs_var),
        input_mapping_json.as_deref(),
        Some(&scenario_inputs_var),
        Some(&start_scenario_scope_id),
    );
    let debug_end = emit_step_debug_end(
        ctx,
        step_id,
        step_name,
        "StartScenario",
        Some(&step_var),
        Some(&scenario_inputs_var),
        Some(&start_scenario_scope_id),
    );

    // Generate tracing spans for OpenTelemetry
    let span_def = emit_step_span_start(step_id, step_name, "StartScenario");
    let child_span_def = emit_child_scenario_span_start(step_id, child_scenario_id);

    // Static base for cache key - will be combined with loop indices at runtime
    let cache_key_base = format!("start_scenario::{}", step_id);

    let max_retries_lit = max_retries;
    let retry_delay_lit = retry_delay;

    Ok(quote! {
        // Define the embedded child scenario function
        #child_fn_code

        let #source_var = #build_source;
        let #child_inputs_var = #inputs_code;

        // Define tracing span for this step
        #span_def

        // Wrap step execution in async block instrumented with span
        async {
            #debug_start

            // Build cache key dynamically, including prefix and loop indices
        let __durable_cache_key = {
            // Get prefix from parent context (set by parent StartScenario)
            let prefix = (*#scenario_inputs_var.variables)
                .as_object()
                .and_then(|vars| vars.get("_cache_key_prefix"))
                .and_then(|v| v.as_str())
                .unwrap_or("");

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

            if prefix.is_empty() {
                format!("{}{}", base, indices_suffix)
            } else {
                format!("{}::{}{}", prefix, base, indices_suffix)
            }
        };

        // Define the durable child scenario execution function
        #[durable(max_retries = #max_retries_lit, delay = #retry_delay_lit)]
        async fn #durable_fn_name(
            cache_key: &str,
            child_inputs: serde_json::Value,
            child_scenario_id: &str,
            step_id: &str,
            step_name: &str,
            parent_scope_id: Option<String>,
            parent_cache_prefix: Option<String>,
            loop_indices_suffix: String,
            parent_scenario_id: Option<String>,
        ) -> std::result::Result<serde_json::Value, String> {
            // Generate scope ID for this child scenario execution
            let __child_scope_id = if let Some(ref parent) = parent_scope_id {
                format!("{}_{}", parent, step_id)
            } else {
                format!("sc_{}", step_id)
            };

            // Prepare child scenario inputs
            // All mapped inputs become child's data (myParam1 -> data.myParam1)
            // Child variables are always isolated - never inherited from parent
            // BUT we inject _scope_id, _cache_key_prefix, and _scenario_id so scope tracking and
            // checkpoint cache keys work correctly within child
            let mut __child_vars = serde_json::Map::new();
            __child_vars.insert("_scope_id".to_string(), serde_json::json!(__child_scope_id.clone()));

            // Propagate _scenario_id to child so it's available at all nesting levels
            // This ensures Agent steps in deeply nested scenarios can access the root scenario identity
            if let Some(ref sid) = parent_scenario_id {
                __child_vars.insert("_scenario_id".to_string(), serde_json::json!(sid));
            }

            // Build cache key prefix for child scenario
            // Inherits parent's prefix (if any) and appends this step's identity + loop indices.
            // When there's no parent prefix (top-level scenario), we include the parent's
            // _scenario_id to prevent cache collisions between independent scenarios that
            // happen to use the same step_id for their StartScenario steps.
            let __child_cache_prefix = {
                match &parent_cache_prefix {
                    Some(p) if !p.is_empty() => format!("{}__{}{}",  p, step_id, loop_indices_suffix),
                    _ => {
                        // No parent prefix - this is a top-level StartScenario.
                        // Include the scenario's unique ID to prevent cache collisions.
                        let scenario_id = parent_scenario_id.as_deref().unwrap_or("root");
                        format!("{}::{}{}",  scenario_id, step_id, loop_indices_suffix)
                    }
                }
            };
            __child_vars.insert("_cache_key_prefix".to_string(), serde_json::json!(__child_cache_prefix));

            // Runtime validation of child inputs against schema
            #runtime_validation_code

            // Inner steps use the child scenario scope as their parent.
            // This ensures `root_scopes_only` filter correctly excludes them
            // (they have non-null parent_scope_id = the child scenario scope).
            let child_scenario_inputs = ScenarioInputs {
                data: Arc::new(child_inputs),
                variables: Arc::new(serde_json::Value::Object(__child_vars)),
                parent_scope_id: Some(__child_scope_id.clone()),
            };

            // Check for interruption (cancel/pause) before executing child scenario
            if runtara_sdk::is_cancelled() {
                let structured_error = serde_json::json!({
                    "stepId": step_id,
                    "stepName": step_name,
                    "stepType": "StartScenario",
                    "code": "STEP_INTERRUPTED",
                    "message": format!("StartScenario step {} interrupted before execution", step_id),
                    "category": "transient",
                    "severity": "info",
                    "childScenarioId": child_scenario_id
                });
                return Err(serde_json::to_string(&structured_error).unwrap_or_else(|_| {
                    format!("StartScenario step {} interrupted", step_id)
                }));
            }

            // Create child scenario span for tracing
            #child_span_def

            // Execute child scenario with cancellation support, instrumented with child span
            let child_result = async {
                match runtara_sdk::with_cancellation(
                    #child_fn_name(Arc::new(child_scenario_inputs))
                ).await {
                    Ok(result) => result,
                    Err(interrupt_err) => {
                        let structured_error = serde_json::json!({
                            "stepId": step_id,
                            "stepName": step_name,
                            "stepType": "StartScenario",
                            "code": "STEP_INTERRUPTED",
                            "message": format!("StartScenario step {} interrupted during execution", step_id),
                            "category": "transient",
                            "severity": "info",
                            "childScenarioId": child_scenario_id,
                            "interruptionReason": interrupt_err
                        });
                        return Err(serde_json::to_string(&structured_error).unwrap_or_else(|_| {
                            format!("StartScenario step {} interrupted", step_id)
                        }));
                    }
                }.map_err(|e: String| {
                    // Try to parse the child error as structured JSON
                    let child_error: serde_json::Value = serde_json::from_str(&e)
                        .unwrap_or_else(|_| serde_json::json!({
                            "message": e,
                            "code": null,
                            "category": "unknown",
                            "severity": "error"
                        }));

                    // Check if this is an Error step - if so, propagate it directly
                    // Error steps have stepType: "Error" and represent explicit business errors
                    // that should bubble up unchanged to the parent scenario
                    if child_error.get("stepType").and_then(|v| v.as_str()) == Some("Error") {
                        // Propagate the error as-is (it's already a valid JSON string)
                        return e;
                    }

                    // For other errors (agent failures, etc.), wrap with CHILD_SCENARIO_FAILED
                    let structured_error = serde_json::json!({
                        "stepId": step_id,
                        "stepName": step_name,
                        "stepType": "StartScenario",
                        "code": "CHILD_SCENARIO_FAILED",
                        "message": format!("Child scenario {} failed", child_scenario_id),
                        "category": child_error.get("category").and_then(|v| v.as_str()).unwrap_or("transient"),
                        "severity": child_error.get("severity").and_then(|v| v.as_str()).unwrap_or("error"),
                        "childScenarioId": child_scenario_id,
                        "childError": child_error
                    });
                    serde_json::to_string(&structured_error).unwrap_or_else(|_| {
                        format!("Child scenario {} failed: {}", child_scenario_id, e)
                    })
                })
            }.instrument(__child_span).await?;

            let result = serde_json::json!({
                "stepId": step_id,
                "stepName": step_name,
                "stepType": "StartScenario",
                "childScenarioId": child_scenario_id,
                "outputs": child_result
            });

            Ok(result)
        }

        // Get parent_scope_id from parent scenario's variables
        let __parent_scope_id = (*#scenario_inputs_var.variables)
            .as_object()
            .and_then(|vars| vars.get("_scope_id"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        // Get parent's cache key prefix from scenario variables
        let __parent_cache_prefix = (*#scenario_inputs_var.variables)
            .as_object()
            .and_then(|vars| vars.get("_cache_key_prefix"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        // Get loop indices suffix for this StartScenario step
        let __loop_indices_suffix = (*#scenario_inputs_var.variables)
            .as_object()
            .and_then(|vars| vars.get("_loop_indices"))
            .and_then(|v| v.as_array())
            .filter(|arr| !arr.is_empty())
            .map(|arr| {
                let indices: Vec<String> = arr.iter().map(|v| v.to_string()).collect();
                format!("[{}]", indices.join(","))
            })
            .unwrap_or_default();

        // Get parent's scenario ID for cache key uniqueness (prevents collision
        // between independent top-level scenarios with same step IDs)
        let __parent_scenario_id = (*#scenario_inputs_var.variables)
            .as_object()
            .and_then(|vars| vars.get("_scenario_id"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        // Execute the durable child scenario function
        let #step_var = #durable_fn_name(
            &__durable_cache_key,
            #child_inputs_var.clone(),
            #child_scenario_id,
            #step_id,
            #step_name_display,
            __parent_scope_id,
            __parent_cache_prefix,
            __loop_indices_suffix,
            __parent_scenario_id,
        ).await?;

            #debug_end

            #steps_context.insert(#step_id.to_string(), #step_var.clone());

            // Check for cancellation or pause after child scenario completes
            {
                let mut __sdk = sdk().lock().await;
                if let Err(e) = __sdk.check_signals().await {
                    let structured_error = serde_json::json!({
                        "stepId": #step_id,
                        "stepName": #step_name_display,
                        "stepType": "StartScenario",
                        "code": "STEP_INTERRUPTED",
                        "message": format!("StartScenario step {} interrupted: {}", #step_id, e),
                        "category": "transient",
                        "severity": "info",
                        "childScenarioId": #child_scenario_id,
                        "reason": e.to_string()
                    });
                    return Err(serde_json::to_string(&structured_error).unwrap_or_else(|_| {
                        format!("StartScenario step {}: {}", #step_id, e)
                    }));
                }
            }

            Ok::<_, String>(())
        }.instrument(__step_span).await?;
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    use runtara_dsl::{
        ChildVersion, FinishStep, ImmediateValue, MappingValue, SchemaField, SchemaFieldType, Step,
    };
    use std::collections::HashMap;

    // =============================================================================
    // Helper functions
    // =============================================================================

    fn create_basic_step(id: &str, child_scenario_id: &str) -> StartScenarioStep {
        StartScenarioStep {
            id: id.to_string(),
            name: None,
            child_scenario_id: child_scenario_id.to_string(),
            child_version: ChildVersion::Latest("latest".to_string()),
            input_mapping: None,
            max_retries: None,
            retry_delay: None,
            timeout: None,
        }
    }

    fn create_named_step(id: &str, name: &str, child_scenario_id: &str) -> StartScenarioStep {
        StartScenarioStep {
            id: id.to_string(),
            name: Some(name.to_string()),
            child_scenario_id: child_scenario_id.to_string(),
            child_version: ChildVersion::Latest("latest".to_string()),
            input_mapping: None,
            max_retries: None,
            retry_delay: None,
            timeout: None,
        }
    }

    fn create_child_graph(name: &str) -> ExecutionGraph {
        let mut steps = HashMap::new();
        steps.insert(
            "finish".to_string(),
            Step::Finish(FinishStep {
                id: "finish".to_string(),
                name: Some("Finish".to_string()),
                input_mapping: None,
            }),
        );
        ExecutionGraph {
            name: Some(name.to_string()),
            description: None,
            steps,
            entry_point: "finish".to_string(),
            execution_plan: vec![],
            variables: HashMap::new(),
            input_schema: HashMap::new(),
            output_schema: HashMap::new(),
            notes: None,
            nodes: None,
            edges: None,
        }
    }

    /// Helper to create a context with child scenarios properly configured.
    /// This allows tests to use the public `emit()` function which handles deduplication.
    fn create_ctx_with_child(
        step_id: &str,
        child_scenario_id: &str,
        child_graph: ExecutionGraph,
        debug_mode: bool,
    ) -> EmitContext {
        let version = 1;
        let mut child_scenarios = HashMap::new();
        child_scenarios.insert(format!("{}::{}", child_scenario_id, version), child_graph);

        let mut step_to_child_ref = HashMap::new();
        step_to_child_ref.insert(
            step_id.to_string(),
            (child_scenario_id.to_string(), version),
        );

        EmitContext::with_child_scenarios(
            debug_mode,
            child_scenarios,
            step_to_child_ref,
            None,
            None,
        )
    }

    // =============================================================================
    // emit tests (main entry point)
    // =============================================================================

    #[test]
    fn test_emit_without_child_graph_returns_error() {
        let step = create_basic_step("start-child", "child-scenario-id");
        let mut ctx = EmitContext::new(false);

        let result = emit(&step, &mut ctx);

        // Should return error when no child graph is available
        assert!(result.is_err());
        let err = result.unwrap_err();
        match err {
            CodegenError::MissingChildScenario {
                step_id,
                child_scenario_id,
            } => {
                assert_eq!(step_id, "start-child");
                assert_eq!(child_scenario_id, "child-scenario-id");
            }
        }
    }

    #[test]
    fn test_emit_with_child_graph() {
        let step = create_named_step("start-child", "Execute Child", "child-scenario-id");

        // Create context with child scenario registered
        // Key format: "scenario_id::version"
        let mut child_scenarios = HashMap::new();
        child_scenarios.insert(
            "child-scenario-id::1".to_string(),
            create_child_graph("Child Graph"),
        );

        // step_to_child_ref maps step_id -> (scenario_id, version)
        let mut step_to_child_ref = HashMap::new();
        step_to_child_ref.insert(
            "start-child".to_string(),
            ("child-scenario-id".to_string(), 1),
        );

        let mut ctx = EmitContext::with_child_scenarios(
            false,
            child_scenarios,
            step_to_child_ref,
            None,
            None,
        );

        let tokens = emit(&step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Should emit embedded version with durable wrapper
        assert!(code.contains("durable"));
        // Child function name is now deterministic: child_{scenario_id}_{version}
        assert!(code.contains("child_child_scenario_id_1"));
        assert!(code.contains("child_scenario_inputs"));

        // Should include cache key handling for loop indices
        assert!(code.contains("__durable_cache_key"));
        assert!(code.contains("_loop_indices"));
    }

    #[test]
    fn test_emit_default_retry_config() {
        let step = create_basic_step("start-child", "child-scenario-id");

        let mut child_scenarios = HashMap::new();
        child_scenarios.insert(
            "child-scenario-id::1".to_string(),
            create_child_graph("Child"),
        );

        let mut step_to_child_ref = HashMap::new();
        step_to_child_ref.insert(
            "start-child".to_string(),
            ("child-scenario-id".to_string(), 1),
        );

        let mut ctx = EmitContext::with_child_scenarios(
            false,
            child_scenarios,
            step_to_child_ref,
            None,
            None,
        );

        let tokens = emit(&step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Default max_retries is 3
        assert!(code.contains("max_retries = 3"));
        // Default retry_delay is 1000
        assert!(code.contains("delay = 1000"));
    }

    #[test]
    fn test_emit_custom_retry_config() {
        let mut step = create_basic_step("start-child", "child-scenario-id");
        step.max_retries = Some(5);
        step.retry_delay = Some(2000);

        let mut child_scenarios = HashMap::new();
        child_scenarios.insert(
            "child-scenario-id::1".to_string(),
            create_child_graph("Child"),
        );

        let mut step_to_child_ref = HashMap::new();
        step_to_child_ref.insert(
            "start-child".to_string(),
            ("child-scenario-id".to_string(), 1),
        );

        let mut ctx = EmitContext::with_child_scenarios(
            false,
            child_scenarios,
            step_to_child_ref,
            None,
            None,
        );

        let tokens = emit(&step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Custom retry config should be used
        assert!(code.contains("max_retries = 5"));
        assert!(code.contains("delay = 2000"));
    }

    // =============================================================================
    // emit_with_embedded_child tests (via emit())
    // =============================================================================

    #[test]
    fn test_emit_with_embedded_child_structure() {
        let step = create_named_step("start-child", "Execute Child", "child-scenario-id");
        let child_graph = create_child_graph("Child Graph");
        let mut ctx = create_ctx_with_child("start-child", "child-scenario-id", child_graph, false);

        let tokens = emit(&step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Check structure of generated code
        // 1. Child function definition
        assert!(code.contains("child_child_scenario_id"));

        // 2. Durable wrapper function
        assert!(code.contains("durable"));
        assert!(code.contains("async fn"));

        // 3. ScenarioInputs creation for child
        assert!(code.contains("ScenarioInputs"));
        assert!(code.contains("data"));
        assert!(code.contains("variables"));

        // 4. Cache key with loop indices support
        assert!(code.contains("start_scenario::"));
        assert!(code.contains("_loop_indices"));

        // 5. Step result stored in context
        assert!(code.contains("steps_context"));
        assert!(code.contains("insert"));
    }

    #[test]
    fn test_emit_with_embedded_child_input_mapping() {
        let mut step = create_basic_step("start-child", "child-scenario-id");

        // Add input mapping
        let mut mapping = HashMap::new();
        mapping.insert(
            "childParam".to_string(),
            MappingValue::Immediate(ImmediateValue {
                value: serde_json::json!("test-value"),
            }),
        );
        step.input_mapping = Some(mapping);

        let child_graph = create_child_graph("Child");
        let mut ctx = create_ctx_with_child("start-child", "child-scenario-id", child_graph, false);

        let tokens = emit(&step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Input mapping should be processed
        assert!(code.contains("child_inputs"));
        assert!(code.contains("test-value"));
    }

    #[test]
    fn test_emit_with_embedded_child_empty_input_mapping() {
        let mut step = create_basic_step("start-child", "child-scenario-id");
        step.input_mapping = Some(HashMap::new()); // Empty mapping

        let child_graph = create_child_graph("Child");
        let mut ctx = create_ctx_with_child("start-child", "child-scenario-id", child_graph, false);

        let tokens = emit(&step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Should use empty object for inputs
        assert!(code.contains("Object"));
        assert!(code.contains("Map :: new"));
    }

    #[test]
    fn test_emit_with_embedded_child_result_structure() {
        let step = create_named_step("start-child", "Test Step", "child-scenario-id");
        let child_graph = create_child_graph("Child");
        let mut ctx = create_ctx_with_child("start-child", "child-scenario-id", child_graph, false);

        let tokens = emit(&step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Result JSON should have expected structure
        assert!(code.contains("stepId"));
        assert!(code.contains("stepName"));
        assert!(code.contains("stepType"));
        assert!(code.contains("StartScenario"));
        assert!(code.contains("childScenarioId"));
        assert!(code.contains("outputs"));
    }

    #[test]
    fn test_emit_with_embedded_child_signal_check() {
        let step = create_basic_step("start-child", "child-scenario-id");
        let child_graph = create_child_graph("Child");
        let mut ctx = create_ctx_with_child("start-child", "child-scenario-id", child_graph, false);

        let tokens = emit(&step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Should check for signals (cancel/pause) after child completes
        assert!(code.contains("check_signals"));
        assert!(code.contains("STEP_INTERRUPTED"));
    }

    // =============================================================================
    // Step ID sanitization tests
    // =============================================================================

    #[test]
    fn test_emit_with_special_characters_in_step_id() {
        let step = create_basic_step("step-with.special-chars", "child-scenario-id");

        let mut child_scenarios = HashMap::new();
        child_scenarios.insert(
            "child-scenario-id::1".to_string(),
            create_child_graph("Child"),
        );

        let mut step_to_child_ref = HashMap::new();
        step_to_child_ref.insert(
            "step-with.special-chars".to_string(),
            ("child-scenario-id".to_string(), 1),
        );

        let mut ctx = EmitContext::with_child_scenarios(
            false,
            child_scenarios,
            step_to_child_ref,
            None,
            None,
        );

        let tokens = emit(&step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Step ID should be sanitized in variable names
        assert!(code.contains("step_with_special_chars"));
    }

    // =============================================================================
    // Debug mode tests
    // =============================================================================

    #[test]
    fn test_emit_debug_mode_generates_events() {
        let step = create_named_step("start-child", "Test Step", "child-scenario-id");

        let mut child_scenarios = HashMap::new();
        child_scenarios.insert(
            "child-scenario-id::1".to_string(),
            create_child_graph("Child"),
        );

        let mut step_to_child_ref = HashMap::new();
        step_to_child_ref.insert(
            "start-child".to_string(),
            ("child-scenario-id".to_string(), 1),
        );

        let mut ctx =
            EmitContext::with_child_scenarios(true, child_scenarios, step_to_child_ref, None, None);

        let tokens = emit(&step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Debug events depend on emit_step_debug_start/end
        // The code should still contain the core functionality
        assert!(code.contains("steps_context"));
        assert!(code.contains("durable"));
    }

    // =============================================================================
    // Variable naming uniqueness tests
    // =============================================================================

    #[test]
    fn test_emit_generates_unique_variable_names() {
        let step1 = create_basic_step("step-1", "child-1");
        let step2 = create_basic_step("step-2", "child-2");

        let mut child_scenarios = HashMap::new();
        child_scenarios.insert("child-1::1".to_string(), create_child_graph("Child 1"));
        child_scenarios.insert("child-2::1".to_string(), create_child_graph("Child 2"));

        let mut step_to_child_ref = HashMap::new();
        step_to_child_ref.insert("step-1".to_string(), ("child-1".to_string(), 1));
        step_to_child_ref.insert("step-2".to_string(), ("child-2".to_string(), 1));

        let mut ctx = EmitContext::with_child_scenarios(
            false,
            child_scenarios,
            step_to_child_ref,
            None,
            None,
        );

        let tokens1 = emit(&step1, &mut ctx).unwrap();
        let tokens2 = emit(&step2, &mut ctx).unwrap();

        let code1 = tokens1.to_string();
        let code2 = tokens2.to_string();

        // Each step should have unique variable names
        assert!(code1.contains("step_step_1"));
        assert!(code2.contains("step_step_2"));
    }

    // =============================================================================
    // Cache key tests
    // =============================================================================

    #[test]
    fn test_emit_cache_key_includes_step_id() {
        let step = create_basic_step("unique-step-id", "child-scenario-id");

        let mut child_scenarios = HashMap::new();
        child_scenarios.insert(
            "child-scenario-id::1".to_string(),
            create_child_graph("Child"),
        );

        let mut step_to_child_ref = HashMap::new();
        step_to_child_ref.insert(
            "unique-step-id".to_string(),
            ("child-scenario-id".to_string(), 1),
        );

        let mut ctx = EmitContext::with_child_scenarios(
            false,
            child_scenarios,
            step_to_child_ref,
            None,
            None,
        );

        let tokens = emit(&step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Cache key should include the step ID (as a string literal in the code)
        assert!(code.contains("start_scenario"));
        assert!(code.contains("unique-step-id"));
        assert!(code.contains("__durable_cache_key"));
    }

    #[test]
    fn test_emit_cache_key_loop_indices_handling() {
        let step = create_basic_step("loop-step", "child-scenario-id");

        let mut child_scenarios = HashMap::new();
        child_scenarios.insert(
            "child-scenario-id::1".to_string(),
            create_child_graph("Child"),
        );

        let mut step_to_child_ref = HashMap::new();
        step_to_child_ref.insert(
            "loop-step".to_string(),
            ("child-scenario-id".to_string(), 1),
        );

        let mut ctx = EmitContext::with_child_scenarios(
            false,
            child_scenarios,
            step_to_child_ref,
            None,
            None,
        );

        let tokens = emit(&step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Should handle loop indices in cache key
        assert!(code.contains("_loop_indices"));
        assert!(code.contains("indices_suffix"));
        // The format! macro becomes code using std::format! or similar in TokenStream
        assert!(code.contains("base") && code.contains("indices_suffix"));
    }

    // =============================================================================
    // Structured error tests
    // =============================================================================

    #[test]
    fn test_emit_with_embedded_child_structured_error() {
        let step = create_named_step("start-child", "Execute Child", "child-scenario-id");
        let child_graph = create_child_graph("Child Graph");
        let mut ctx = create_ctx_with_child("start-child", "child-scenario-id", child_graph, false);

        let tokens = emit(&step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Should emit structured error for child scenario failures
        assert!(
            code.contains("CHILD_SCENARIO_FAILED"),
            "Should include CHILD_SCENARIO_FAILED error code"
        );
        assert!(
            code.contains("structured_error"),
            "Should build structured error"
        );
        assert!(
            code.contains("childError"),
            "Should include child error details"
        );
        assert!(code.contains("category"), "Should include error category");
        assert!(code.contains("severity"), "Should include error severity");
    }

    #[test]
    fn test_emit_with_embedded_child_structured_cancellation() {
        let step = create_named_step("start-child", "Execute Child", "child-scenario-id");
        let child_graph = create_child_graph("Child Graph");
        let mut ctx = create_ctx_with_child("start-child", "child-scenario-id", child_graph, false);

        let tokens = emit(&step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Should emit structured error for interruption (cancel or pause)
        assert!(
            code.contains("STEP_INTERRUPTED"),
            "Should include STEP_INTERRUPTED error code"
        );
        assert!(
            code.contains("interruptionReason"),
            "Should include interruption reason"
        );
        assert!(
            code.contains("\"transient\""),
            "Interruption should be transient category"
        );
        assert!(
            code.contains("\"info\""),
            "Interruption should be info severity"
        );
    }

    #[test]
    fn test_emit_child_error_propagation() {
        let step = create_basic_step("start-child", "child-scenario-id");

        let mut child_scenarios = HashMap::new();
        child_scenarios.insert(
            "child-scenario-id::1".to_string(),
            create_child_graph("Child"),
        );

        let mut step_to_child_ref = HashMap::new();
        step_to_child_ref.insert(
            "start-child".to_string(),
            ("child-scenario-id".to_string(), 1),
        );

        let mut ctx = EmitContext::with_child_scenarios(
            false,
            child_scenarios,
            step_to_child_ref,
            None,
            None,
        );

        let tokens = emit(&step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Should parse child error as structured JSON
        assert!(
            code.contains("serde_json :: from_str"),
            "Should parse child error as JSON"
        );
        // Should propagate child error category
        assert!(
            code.contains("child_error . get (\"category\")"),
            "Should extract child error category"
        );
        // Should propagate child error severity
        assert!(
            code.contains("child_error . get (\"severity\")"),
            "Should extract child error severity"
        );
    }

    #[test]
    fn test_emit_error_step_propagation() {
        // Test that Error step errors from child scenarios propagate directly
        // without being wrapped in CHILD_SCENARIO_FAILED
        let step = create_named_step("start-child", "Execute Child", "child-scenario-id");
        let child_graph = create_child_graph("Child Graph");
        let mut ctx = create_ctx_with_child("start-child", "child-scenario-id", child_graph, false);

        let tokens = emit(&step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Should check for stepType: "Error" to identify Error step errors
        assert!(
            code.contains("child_error . get (\"stepType\")"),
            "Should check stepType field"
        );
        assert!(
            code.contains("Some (\"Error\")"),
            "Should compare stepType to 'Error'"
        );
        // Should return the error as-is when it's an Error step
        assert!(
            code.contains("return e"),
            "Should propagate Error step errors directly"
        );
    }

    #[test]
    fn test_emit_no_turbofish_null_in_json_macro() {
        // Regression test: ensure null::<Type> is not used inside serde_json::json! macro
        // The json! macro doesn't support turbofish syntax on null, causing:
        // "error: no rules expected `::` in macro call"
        let step = create_named_step("start-child", "Execute Child", "child-scenario-id");
        let child_graph = create_child_graph("Child Graph");
        let mut ctx = create_ctx_with_child("start-child", "child-scenario-id", child_graph, false);

        let tokens = emit(&step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Ensure no turbofish null syntax appears in generated code
        assert!(
            !code.contains("null::<") && !code.contains("null :: <"),
            "Generated code must not contain null::<Type> - json! macro doesn't support turbofish"
        );
    }

    // =============================================================================
    // Cache key prefix tests
    // =============================================================================

    #[test]
    fn test_emit_with_embedded_child_sets_cache_key_prefix() {
        let step = create_named_step("start-child", "Execute Child", "child-scenario-id");
        let child_graph = create_child_graph("Child Graph");
        let mut ctx = create_ctx_with_child("start-child", "child-scenario-id", child_graph, false);

        let tokens = emit(&step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Verify _cache_key_prefix is set in child vars
        assert!(
            code.contains("_cache_key_prefix"),
            "Generated code must set _cache_key_prefix in child variables"
        );

        // Verify prefix is built from step_id
        assert!(
            code.contains("__child_cache_prefix"),
            "Generated code must build __child_cache_prefix"
        );
    }

    #[test]
    fn test_emit_with_embedded_child_reads_parent_prefix() {
        let step = create_named_step("start-child", "Execute Child", "child-scenario-id");
        let child_graph = create_child_graph("Child Graph");
        let mut ctx = create_ctx_with_child("start-child", "child-scenario-id", child_graph, false);

        let tokens = emit(&step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Verify parent prefix is read from variables
        assert!(
            code.contains("__parent_cache_prefix"),
            "Generated code must extract __parent_cache_prefix from parent variables"
        );
    }

    #[test]
    fn test_emit_own_cache_key_includes_prefix() {
        let step = create_named_step("start-child", "Execute Child", "child-scenario-id");
        let child_graph = create_child_graph("Child Graph");
        let mut ctx = create_ctx_with_child("start-child", "child-scenario-id", child_graph, false);

        let tokens = emit(&step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Verify StartScenario's own cache key reads _cache_key_prefix
        // The code should contain the prefix reading logic for the durable cache key
        assert!(
            code.contains("_cache_key_prefix") && code.contains("__durable_cache_key"),
            "StartScenario's own cache key must include parent prefix"
        );
    }

    // =============================================================================
    // Cache key collision prevention tests (scenario_id propagation)
    // =============================================================================

    #[test]
    fn test_emit_extracts_parent_scenario_id() {
        // Verifies that _scenario_id is extracted from parent's variables
        let step = create_named_step("call-child", "Call Child", "child-scenario-id");
        let child_graph = create_child_graph("Child Graph");
        let mut ctx = create_ctx_with_child("call-child", "child-scenario-id", child_graph, false);

        let tokens = emit(&step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Should extract __parent_scenario_id from parent's variables
        assert!(
            code.contains("__parent_scenario_id"),
            "Should extract __parent_scenario_id from parent variables"
        );
        assert!(
            code.contains("_scenario_id"),
            "Should read _scenario_id from variables"
        );
    }

    #[test]
    fn test_emit_propagates_scenario_id_to_child() {
        // Verifies that _scenario_id is propagated to child's variables
        let step = create_named_step("call-child", "Call Child", "child-scenario-id");
        let child_graph = create_child_graph("Child Graph");
        let mut ctx = create_ctx_with_child("call-child", "child-scenario-id", child_graph, false);

        let tokens = emit(&step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Should propagate _scenario_id to __child_vars
        assert!(
            code.contains("__child_vars . insert (\"_scenario_id\""),
            "Should propagate _scenario_id to child variables"
        );
    }

    #[test]
    fn test_emit_uses_scenario_id_in_fallback_prefix() {
        // Verifies that when there's no parent prefix, _scenario_id is used
        let step = create_named_step("call-child", "Call Child", "child-scenario-id");
        let child_graph = create_child_graph("Child Graph");
        let mut ctx = create_ctx_with_child("call-child", "child-scenario-id", child_graph, false);

        let tokens = emit(&step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // When no parent prefix, should use parent_scenario_id for unique cache key
        assert!(
            code.contains("parent_scenario_id . as_deref ()"),
            "Should use parent_scenario_id when no parent prefix"
        );
        // Should have fallback to "root" if no scenario_id
        assert!(
            code.contains("unwrap_or (\"root\")"),
            "Should fallback to 'root' if no scenario_id"
        );
    }

    #[test]
    fn test_emit_passes_scenario_id_to_durable_function() {
        // Verifies that parent_scenario_id is passed as a parameter to the durable function
        let step = create_named_step("call-child", "Call Child", "child-scenario-id");
        let child_graph = create_child_graph("Child Graph");
        let mut ctx = create_ctx_with_child("call-child", "child-scenario-id", child_graph, false);

        let tokens = emit(&step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Durable function should have parent_scenario_id parameter
        assert!(
            code.contains("parent_scenario_id : Option < String >"),
            "Durable function should have parent_scenario_id parameter"
        );
        // Should pass __parent_scenario_id when calling durable function
        assert!(
            code.contains("__parent_scenario_id"),
            "Should pass __parent_scenario_id to durable function"
        );
    }

    #[test]
    fn test_cache_key_prefix_format_with_scenario_id() {
        // Verifies the cache prefix format includes scenario_id when no parent prefix
        let step = create_named_step("process-files", "Process Files", "child-scenario-id");
        let child_graph = create_child_graph("Child Graph");
        let mut ctx =
            create_ctx_with_child("process-files", "child-scenario-id", child_graph, false);

        let tokens = emit(&step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // The format should be: format!("{}::{}{}", scenario_id, step_id, loop_indices_suffix)
        // This ensures different parent scenarios produce different prefixes
        assert!(
            code.contains("\"{}::{}{}\"") && code.contains("scenario_id"),
            "Fallback prefix format should use scenario_id::step_id::indices pattern"
        );
    }

    #[test]
    fn test_cache_key_prefix_format_with_parent_prefix() {
        // Verifies the cache prefix format when parent prefix exists
        let step = create_named_step("nested-call", "Nested Call", "child-scenario-id");
        let child_graph = create_child_graph("Child Graph");
        let mut ctx = create_ctx_with_child("nested-call", "child-scenario-id", child_graph, false);

        let tokens = emit(&step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // When parent prefix exists, format is: format!("{}__{}{}",  p, step_id, loop_indices_suffix)
        assert!(
            code.contains("\"{}__{}{}\""),
            "Nested prefix format should use parent__step_id__indices pattern"
        );
    }

    #[test]
    fn test_scenario_collision_prevention_complete_chain() {
        // This test verifies the complete chain of scenario_id handling:
        // 1. Root scenario injects _scenario_id
        // 2. StartScenario reads parent's _scenario_id
        // 3. StartScenario propagates _scenario_id to child
        // 4. Child uses _scenario_id for cache key prefix when no parent prefix
        //
        // This prevents collisions like:
        //   Orchestrator -> A -> D (cache key includes "A's path")
        //   Orchestrator -> B -> D (cache key includes "B's path")

        let step = create_named_step("call-shared-child", "Call Shared Child", "shared-child");
        let child_graph = create_child_graph("Shared Child");
        let mut ctx =
            create_ctx_with_child("call-shared-child", "shared-child", child_graph, false);

        let tokens = emit(&step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // All required elements for collision prevention must be present:

        // 1. Extract parent's scenario_id
        assert!(
            code.contains("vars . get (\"_scenario_id\")"),
            "Must read _scenario_id from parent variables"
        );

        // 2. Propagate to child
        assert!(
            code.contains("__child_vars . insert (\"_scenario_id\""),
            "Must propagate _scenario_id to child"
        );

        // 3. Use in cache prefix fallback
        assert!(
            code.contains("parent_scenario_id . as_deref ()"),
            "Must use parent_scenario_id in fallback"
        );

        // 4. Different format for nested (with prefix) vs top-level (without prefix)
        assert!(
            code.contains("Some (p) if ! p . is_empty ()"),
            "Must check for existing parent prefix"
        );
    }

    // =============================================================================
    // Deduplication tests - verify same child scenario emitted once
    // =============================================================================

    #[test]
    fn test_deduplication_same_child_called_twice() {
        // Two StartScenario steps calling the same child scenario
        let step1 = create_basic_step("call-child-1", "shared-child");
        let step2 = create_basic_step("call-child-2", "shared-child");

        // Create child scenario graph
        let child_graph = create_child_graph("Shared Child");
        let mut child_scenarios = HashMap::new();
        child_scenarios.insert("shared-child::1".to_string(), child_graph);

        let mut step_to_child_ref = HashMap::new();
        step_to_child_ref.insert("call-child-1".to_string(), ("shared-child".to_string(), 1));
        step_to_child_ref.insert("call-child-2".to_string(), ("shared-child".to_string(), 1));

        let mut ctx = EmitContext::with_child_scenarios(
            false,
            child_scenarios,
            step_to_child_ref,
            None,
            None,
        );

        // Emit first step
        let code1 = emit(&step1, &mut ctx).expect("Should emit step 1");
        let code1_str = code1.to_string();

        // Emit second step
        let code2 = emit(&step2, &mut ctx).expect("Should emit step 2");
        let code2_str = code2.to_string();

        // First emission should contain the function definition
        assert!(
            code1_str.contains("async fn child_shared_child_1"),
            "First step should define the shared function"
        );

        // Second emission should NOT contain the function definition
        assert!(
            !code2_str.contains("async fn child_shared_child_1"),
            "Second step should NOT redefine the shared function"
        );

        // Both should reference the same function name in their durable wrappers
        assert!(
            code1_str.contains("child_shared_child_1"),
            "First step should call the shared function"
        );
        assert!(
            code2_str.contains("child_shared_child_1"),
            "Second step should call the shared function"
        );
    }

    #[test]
    fn test_deduplication_different_children_both_emitted() {
        let step1 = create_basic_step("call-child-a", "child-a");
        let step2 = create_basic_step("call-child-b", "child-b");

        let mut child_scenarios = HashMap::new();
        child_scenarios.insert("child-a::1".to_string(), create_child_graph("Child A"));
        child_scenarios.insert("child-b::1".to_string(), create_child_graph("Child B"));

        let mut step_to_child_ref = HashMap::new();
        step_to_child_ref.insert("call-child-a".to_string(), ("child-a".to_string(), 1));
        step_to_child_ref.insert("call-child-b".to_string(), ("child-b".to_string(), 1));

        let mut ctx = EmitContext::with_child_scenarios(
            false,
            child_scenarios,
            step_to_child_ref,
            None,
            None,
        );

        let code1 = emit(&step1, &mut ctx).expect("Should emit step 1");
        let code2 = emit(&step2, &mut ctx).expect("Should emit step 2");

        // Both should emit their respective functions (different children)
        assert!(
            code1.to_string().contains("async fn child_child_a_1"),
            "First step should define child_a function"
        );
        assert!(
            code2.to_string().contains("async fn child_child_b_1"),
            "Second step should define child_b function"
        );
    }

    #[test]
    fn test_deduplication_different_versions_both_emitted() {
        let step1 = create_basic_step("call-v1", "my-child");
        let step2 = create_basic_step("call-v2", "my-child");

        let mut child_scenarios = HashMap::new();
        child_scenarios.insert("my-child::1".to_string(), create_child_graph("Child v1"));
        child_scenarios.insert("my-child::2".to_string(), create_child_graph("Child v2"));

        let mut step_to_child_ref = HashMap::new();
        step_to_child_ref.insert("call-v1".to_string(), ("my-child".to_string(), 1));
        step_to_child_ref.insert("call-v2".to_string(), ("my-child".to_string(), 2));

        let mut ctx = EmitContext::with_child_scenarios(
            false,
            child_scenarios,
            step_to_child_ref,
            None,
            None,
        );

        let code1 = emit(&step1, &mut ctx).expect("Should emit step 1");
        let code2 = emit(&step2, &mut ctx).expect("Should emit step 2");

        // Different versions = different functions
        assert!(
            code1.to_string().contains("async fn child_my_child_1"),
            "First step should define version 1 function"
        );
        assert!(
            code2.to_string().contains("async fn child_my_child_2"),
            "Second step should define version 2 function"
        );
    }

    #[test]
    fn test_deduplication_three_calls_same_child() {
        // Three steps calling the same child - function should be emitted only once
        let step1 = create_basic_step("call-1", "shared");
        let step2 = create_basic_step("call-2", "shared");
        let step3 = create_basic_step("call-3", "shared");

        let mut child_scenarios = HashMap::new();
        child_scenarios.insert("shared::1".to_string(), create_child_graph("Shared"));

        let mut step_to_child_ref = HashMap::new();
        step_to_child_ref.insert("call-1".to_string(), ("shared".to_string(), 1));
        step_to_child_ref.insert("call-2".to_string(), ("shared".to_string(), 1));
        step_to_child_ref.insert("call-3".to_string(), ("shared".to_string(), 1));

        let mut ctx = EmitContext::with_child_scenarios(
            false,
            child_scenarios,
            step_to_child_ref,
            None,
            None,
        );

        let code1 = emit(&step1, &mut ctx)
            .expect("Should emit step 1")
            .to_string();
        let code2 = emit(&step2, &mut ctx)
            .expect("Should emit step 2")
            .to_string();
        let code3 = emit(&step3, &mut ctx)
            .expect("Should emit step 3")
            .to_string();

        // Only first should have the function definition
        assert!(code1.contains("async fn child_shared_1"));
        assert!(!code2.contains("async fn child_shared_1"));
        assert!(!code3.contains("async fn child_shared_1"));

        // All should call it
        assert!(code1.contains("child_shared_1"));
        assert!(code2.contains("child_shared_1"));
        assert!(code3.contains("child_shared_1"));
    }

    // =============================================================================
    // Generated code validation tests
    // These tests verify the generated code is valid Rust that will compile.
    // =============================================================================

    /// Validates that generated TokenStream is valid Rust syntax using syn.
    /// This catches syntax errors early but won't catch type inference issues.
    fn validate_syntax(tokens: &TokenStream) -> Result<(), String> {
        let code = tokens.to_string();
        // Parse as a statement sequence (what we generate for step code)
        syn::parse_str::<syn::File>(&format!("fn __validate() {{ {} }}", code))
            .map(|_| ())
            .map_err(|e| format!("Syntax error in generated code: {}", e))
    }

    #[test]
    fn test_generated_code_is_valid_syntax() {
        let step = create_named_step("start-child", "Execute Child", "child-scenario-id");
        let child_graph = create_child_graph("Child Graph");
        let mut ctx = create_ctx_with_child("start-child", "child-scenario-id", child_graph, false);

        let tokens = emit(&step, &mut ctx).expect("Should emit step");

        validate_syntax(&tokens).expect("Generated code should be valid Rust syntax");
    }

    #[test]
    fn test_generated_code_with_input_mapping_is_valid_syntax() {
        let mut step = create_basic_step("start-child", "child-scenario-id");
        let mut mapping = HashMap::new();
        mapping.insert(
            "param1".to_string(),
            MappingValue::Immediate(ImmediateValue {
                value: serde_json::json!("test"),
            }),
        );
        step.input_mapping = Some(mapping);

        let child_graph = create_child_graph("Child");
        let mut ctx = create_ctx_with_child("start-child", "child-scenario-id", child_graph, false);

        let tokens = emit(&step, &mut ctx).expect("Should emit step");

        validate_syntax(&tokens).expect("Generated code with input mapping should be valid syntax");
    }

    /// Regression test: Ensure map_err closure has explicit String type annotation.
    /// Without this, the compiler may infer `str` instead of `String`, causing:
    /// "error[E0277]: the size for values of type `str` cannot be known at compilation time"
    #[test]
    fn test_map_err_closure_has_explicit_type() {
        let step = create_basic_step("start-child", "child-scenario-id");
        let child_graph = create_child_graph("Child");
        let mut ctx = create_ctx_with_child("start-child", "child-scenario-id", child_graph, false);

        let tokens = emit(&step, &mut ctx).expect("Should emit step");
        let code = tokens.to_string();

        // The map_err closure MUST have explicit String type to avoid type inference issues
        // Pattern: .map_err(|e: String| { ... })
        assert!(
            code.contains("map_err (| e : String |")
                || code.contains("map_err(|e: String|")
                || code.contains("map_err (| e : String |"),
            "map_err closure must have explicit |e: String| type annotation to avoid type inference errors. \
             Found code: {}",
            // Show a snippet around map_err for debugging
            code.find("map_err")
                .map(|i| &code[i.saturating_sub(20)..code.len().min(i + 100)])
                .unwrap_or("map_err not found")
        );
    }

    /// Regression test: Ensure no turbofish syntax in json! macro calls.
    /// The serde_json::json! macro doesn't support turbofish on null.
    #[test]
    fn test_no_turbofish_in_json_macro() {
        let step = create_basic_step("start-child", "child-scenario-id");
        let child_graph = create_child_graph("Child");
        let mut ctx = create_ctx_with_child("start-child", "child-scenario-id", child_graph, false);

        let tokens = emit(&step, &mut ctx).expect("Should emit step");
        let code = tokens.to_string();

        assert!(
            !code.contains("null::<") && !code.contains("null :: <"),
            "Generated code must not contain null::<Type> - json! macro doesn't support turbofish"
        );
    }

    // =============================================================================
    // Runtime validation tests
    // =============================================================================

    fn create_child_graph_with_schema(
        name: &str,
        input_schema: HashMap<String, SchemaField>,
    ) -> ExecutionGraph {
        let mut steps = HashMap::new();
        steps.insert(
            "finish".to_string(),
            Step::Finish(FinishStep {
                id: "finish".to_string(),
                name: Some("Finish".to_string()),
                input_mapping: None,
            }),
        );
        ExecutionGraph {
            name: Some(name.to_string()),
            description: None,
            steps,
            entry_point: "finish".to_string(),
            execution_plan: vec![],
            variables: HashMap::new(),
            input_schema,
            output_schema: HashMap::new(),
            notes: None,
            nodes: None,
            edges: None,
        }
    }

    #[test]
    fn test_emit_no_validation_when_no_required_fields() {
        let step = create_basic_step("start-child", "child-scenario-id");
        // Child graph has no input schema (no required fields)
        let child_graph = create_child_graph("Child");
        let mut ctx = create_ctx_with_child("start-child", "child-scenario-id", child_graph, false);

        let tokens = emit(&step, &mut ctx).expect("Should emit step");
        let code = tokens.to_string();

        // Should NOT contain validation code when no required fields
        assert!(
            !code.contains("validate_child_inputs"),
            "Should not generate validation when no required fields"
        );
        assert!(
            !code.contains("ChildInputSchema"),
            "Should not generate schema when no required fields"
        );
    }

    #[test]
    fn test_emit_validation_when_required_fields_exist() {
        let step = create_basic_step("start-child", "child-scenario-id");

        // Create child graph with required fields
        let mut input_schema = HashMap::new();
        input_schema.insert(
            "orderId".to_string(),
            SchemaField {
                field_type: SchemaFieldType::String,
                description: Some("The order ID".to_string()),
                required: true,
                default: None,
                example: None,
                items: None,
                enum_values: None,
            },
        );
        input_schema.insert(
            "amount".to_string(),
            SchemaField {
                field_type: SchemaFieldType::Number,
                description: None,
                required: true,
                default: None,
                example: None,
                items: None,
                enum_values: None,
            },
        );
        input_schema.insert(
            "optionalField".to_string(),
            SchemaField {
                field_type: SchemaFieldType::String,
                description: Some("Optional field".to_string()),
                required: false,
                default: None,
                example: None,
                items: None,
                enum_values: None,
            },
        );

        let child_graph = create_child_graph_with_schema("Child", input_schema);

        let mut child_scenarios = HashMap::new();
        child_scenarios.insert("child-scenario-id::1".to_string(), child_graph);

        let mut step_to_child_ref = HashMap::new();
        step_to_child_ref.insert(
            "start-child".to_string(),
            ("child-scenario-id".to_string(), 1),
        );

        let mut ctx = EmitContext::with_child_scenarios(
            false,
            child_scenarios,
            step_to_child_ref,
            None,
            None,
        );

        let tokens = emit(&step, &mut ctx).expect("Should emit step");
        let code = tokens.to_string();

        // Should contain validation code
        assert!(
            code.contains("validate_child_inputs"),
            "Should generate validate_child_inputs call"
        );
        assert!(
            code.contains("ChildInputSchema"),
            "Should generate ChildInputSchema"
        );
        assert!(
            code.contains("RequiredField"),
            "Should generate RequiredField entries"
        );

        // Should include required fields (orderId and amount) but not optional field
        assert!(
            code.contains("orderId"),
            "Should include required field orderId"
        );
        assert!(
            code.contains("amount"),
            "Should include required field amount"
        );
        // optionalField should NOT appear in validation schema
        assert!(
            !code.contains("optionalField"),
            "Should not include optional field"
        );
    }

    #[test]
    fn test_emit_validation_includes_field_metadata() {
        let step = create_basic_step("validate-step", "child-with-schema");

        let mut input_schema = HashMap::new();
        input_schema.insert(
            "userId".to_string(),
            SchemaField {
                field_type: SchemaFieldType::Integer,
                description: Some("User identifier".to_string()),
                required: true,
                default: None,
                example: None,
                items: None,
                enum_values: None,
            },
        );

        let child_graph = create_child_graph_with_schema("Child With Schema", input_schema);

        let mut child_scenarios = HashMap::new();
        child_scenarios.insert("child-with-schema::1".to_string(), child_graph);

        let mut step_to_child_ref = HashMap::new();
        step_to_child_ref.insert(
            "validate-step".to_string(),
            ("child-with-schema".to_string(), 1),
        );

        let mut ctx = EmitContext::with_child_scenarios(
            false,
            child_scenarios,
            step_to_child_ref,
            None,
            None,
        );

        let tokens = emit(&step, &mut ctx).expect("Should emit step");
        let code = tokens.to_string();

        // Should include the step ID and child scenario ID for error messages
        assert!(
            code.contains("\"validate-step\""),
            "Should include step ID for error messages"
        );
        assert!(
            code.contains("\"child-with-schema\""),
            "Should include child scenario ID for error messages"
        );

        // Should include field type
        assert!(
            code.contains("Integer"),
            "Should include field type in generated schema"
        );

        // Should include description
        assert!(
            code.contains("User identifier"),
            "Should include field description in generated schema"
        );
    }

    #[test]
    fn test_emit_validation_with_no_description() {
        let step = create_basic_step("start-child", "child-scenario-id");

        let mut input_schema = HashMap::new();
        input_schema.insert(
            "fieldNoDesc".to_string(),
            SchemaField {
                field_type: SchemaFieldType::Boolean,
                description: None,
                required: true,
                default: None,
                example: None,
                items: None,
                enum_values: None,
            },
        );

        let child_graph = create_child_graph_with_schema("Child", input_schema);

        let mut child_scenarios = HashMap::new();
        child_scenarios.insert("child-scenario-id::1".to_string(), child_graph);

        let mut step_to_child_ref = HashMap::new();
        step_to_child_ref.insert(
            "start-child".to_string(),
            ("child-scenario-id".to_string(), 1),
        );

        let mut ctx = EmitContext::with_child_scenarios(
            false,
            child_scenarios,
            step_to_child_ref,
            None,
            None,
        );

        let tokens = emit(&step, &mut ctx).expect("Should emit step");
        let code = tokens.to_string();

        // Should handle None description properly
        assert!(
            code.contains("description : None"),
            "Should generate None for missing description"
        );
    }

    #[test]
    fn test_emit_validation_generated_code_is_valid_syntax() {
        let step = create_basic_step("start-child", "child-scenario-id");

        let mut input_schema = HashMap::new();
        input_schema.insert(
            "name".to_string(),
            SchemaField {
                field_type: SchemaFieldType::String,
                description: Some("Name field".to_string()),
                required: true,
                default: None,
                example: None,
                items: None,
                enum_values: None,
            },
        );
        input_schema.insert(
            "count".to_string(),
            SchemaField {
                field_type: SchemaFieldType::Integer,
                description: None,
                required: true,
                default: None,
                example: None,
                items: None,
                enum_values: None,
            },
        );

        let child_graph = create_child_graph_with_schema("Child", input_schema);

        let mut child_scenarios = HashMap::new();
        child_scenarios.insert("child-scenario-id::1".to_string(), child_graph);

        let mut step_to_child_ref = HashMap::new();
        step_to_child_ref.insert(
            "start-child".to_string(),
            ("child-scenario-id".to_string(), 1),
        );

        let mut ctx = EmitContext::with_child_scenarios(
            false,
            child_scenarios,
            step_to_child_ref,
            None,
            None,
        );

        let tokens = emit(&step, &mut ctx).expect("Should emit step");

        // Validate syntax using syn parser
        validate_syntax(&tokens)
            .expect("Generated code with validation should be valid Rust syntax");
    }
}
