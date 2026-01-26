// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! StartScenario step emitter.
//!
//! The StartScenario step executes a nested child scenario.
//! When a child scenario's ExecutionGraph is available in the EmitContext,
//! it will be recursively emitted and embedded into the parent scenario.
//! The entire child scenario result uses #[durable] macro for checkpoint-based recovery.

use proc_macro2::TokenStream;
use quote::quote;

use super::super::context::EmitContext;
use super::super::mapping;
use super::super::program;
use super::{emit_step_debug_end, emit_step_debug_start};
use runtara_dsl::{ExecutionGraph, StartScenarioStep};

/// Emit code for a StartScenario step.
///
/// If the child scenario's ExecutionGraph is available in the EmitContext,
/// it will be recursively emitted as an embedded function. Otherwise,
/// a placeholder/warning is generated.
pub fn emit(step: &StartScenarioStep, ctx: &mut EmitContext) -> TokenStream {
    let step_id = &step.id;
    let step_name = step.name.as_deref();
    let step_name_display = step_name.unwrap_or("Unnamed");
    let child_scenario_id = &step.child_scenario_id;

    // Get retry configuration with defaults
    let max_retries = step.max_retries.unwrap_or(3);
    let retry_delay = step.retry_delay.unwrap_or(1000);

    // Check if we have the child scenario's graph
    if let Some(child_graph) = ctx.get_child_scenario(step_id).cloned() {
        // We have the child graph - emit embedded version
        emit_with_embedded_child(step, &child_graph, ctx, max_retries, retry_delay)
    } else {
        // No child graph available - emit placeholder
        emit_placeholder(
            step_id,
            step_name,
            step_name_display,
            child_scenario_id,
            ctx,
        )
    }
}

/// Emit a StartScenario step with an embedded child scenario.
fn emit_with_embedded_child(
    step: &StartScenarioStep,
    child_graph: &ExecutionGraph,
    ctx: &mut EmitContext,
    max_retries: u32,
    retry_delay: u64,
) -> TokenStream {
    let step_id = &step.id;
    let step_name = step.name.as_deref();
    let step_name_display = step_name.unwrap_or("Unnamed");
    let child_scenario_id = &step.child_scenario_id;

    // Do all mutable operations first
    let step_var = ctx.declare_step(step_id);
    let source_var = ctx.temp_var("source");
    let child_inputs_var = ctx.temp_var("child_inputs");
    let child_fn_name = ctx.temp_var(&format!(
        "execute_child_{}",
        EmitContext::sanitize_ident(step_id)
    ));
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

    // Generate the embedded child scenario function using shared recursive emitter
    let child_fn_code = program::emit_graph_as_function(&child_fn_name, child_graph, ctx);

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

    // Static base for cache key - will be combined with loop indices at runtime
    let cache_key_base = format!("start_scenario::{}", step_id);

    let max_retries_lit = max_retries;
    let retry_delay_lit = retry_delay;

    quote! {
        // Define the embedded child scenario function
        #child_fn_code

        let #source_var = #build_source;
        let #child_inputs_var = #inputs_code;

        #debug_start

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

        // Define the durable child scenario execution function
        #[durable(max_retries = #max_retries_lit, delay = #retry_delay_lit)]
        async fn #durable_fn_name(
            cache_key: &str,
            child_inputs: serde_json::Value,
            child_scenario_id: &str,
            step_id: &str,
            step_name: &str,
            parent_scope_id: Option<String>,
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
            // BUT we inject _scope_id so scope tracking works within child
            let mut __child_vars = serde_json::Map::new();
            __child_vars.insert("_scope_id".to_string(), serde_json::json!(__child_scope_id.clone()));

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

            // Execute child scenario with cancellation support
            let child_result = match runtara_sdk::with_cancellation(
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
            }.map_err(|e| {
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
                })?;

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

        // Execute the durable child scenario function
        let #step_var = #durable_fn_name(
            &__durable_cache_key,
            #child_inputs_var.clone(),
            #child_scenario_id,
            #step_id,
            #step_name_display,
            __parent_scope_id,
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
    }
}

/// Emit a placeholder for when child scenario is not available.
fn emit_placeholder(
    step_id: &str,
    step_name: Option<&str>,
    step_name_display: &str,
    child_scenario_id: &str,
    ctx: &mut EmitContext,
) -> TokenStream {
    // Do all mutable operations first
    let step_var = ctx.declare_step(step_id);
    let source_var = ctx.temp_var("source");
    let placeholder_inputs_var = ctx.temp_var("placeholder_inputs");

    // Clone immutable references
    let steps_context = ctx.steps_context_var.clone();

    // Build the source for input mapping
    let build_source = mapping::emit_build_source(ctx);

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
        Some(&placeholder_inputs_var),
        None,
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

    quote! {
        let #source_var = #build_source;
        let #placeholder_inputs_var = serde_json::json!({"childScenarioId": #child_scenario_id, "placeholder": true});

        #debug_start

        // Placeholder: child scenario not available at compile time
        let child_result = {
            eprintln!("WARNING: Child scenario {} not embedded - returning empty result", #child_scenario_id);
            serde_json::json!({
                "warning": format!("Child scenario {} was not available at compile time", #child_scenario_id)
            })
        };

        let #step_var = serde_json::json!({
            "stepId": #step_id,
            "stepName": #step_name_display,
            "stepType": "StartScenario",
            "childScenarioId": #child_scenario_id,
            "outputs": child_result
        });

        #debug_end

        #steps_context.insert(#step_id.to_string(), #step_var.clone());

        // Check for cancellation or pause after step completes
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtara_dsl::{ChildVersion, FinishStep, ImmediateValue, MappingValue, Step};
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

    // =============================================================================
    // emit_placeholder tests
    // =============================================================================

    #[test]
    fn test_emit_placeholder_basic() {
        let mut ctx = EmitContext::new(false);
        let tokens = emit_placeholder("step-1", None, "Unnamed", "child-scenario", &mut ctx);

        let code = tokens.to_string();

        // Check for placeholder JSON structure
        assert!(code.contains("childScenarioId"));
        assert!(code.contains("placeholder"));
        assert!(code.contains("child-scenario"));

        // Check for warning message
        assert!(code.contains("WARNING"));
        assert!(code.contains("not embedded"));

        // Check step result is stored in context
        assert!(code.contains("steps_context"));
        assert!(code.contains("insert"));
    }

    #[test]
    fn test_emit_placeholder_with_name() {
        let mut ctx = EmitContext::new(false);
        let tokens = emit_placeholder(
            "step-1",
            Some("My Step"),
            "My Step",
            "child-scenario",
            &mut ctx,
        );

        let code = tokens.to_string();

        // Check step name is used
        assert!(code.contains("My Step"));
    }

    #[test]
    fn test_emit_placeholder_includes_signal_check() {
        let mut ctx = EmitContext::new(false);
        let tokens = emit_placeholder("step-1", None, "Unnamed", "child-scenario", &mut ctx);

        let code = tokens.to_string();

        // Check for signal handling (cancel/pause)
        assert!(code.contains("check_signals"));
        assert!(code.contains("STEP_INTERRUPTED"));
    }

    #[test]
    fn test_emit_placeholder_debug_mode() {
        let mut ctx = EmitContext::new(true);
        let tokens = emit_placeholder("step-1", Some("Test"), "Test", "child-scenario", &mut ctx);

        let code = tokens.to_string();

        // In debug mode, debug events should be emitted
        // (The actual debug emission depends on emit_step_debug_start/end behavior)
        assert!(code.contains("steps_context"));
    }

    // =============================================================================
    // emit tests (main entry point)
    // =============================================================================

    #[test]
    fn test_emit_without_child_graph() {
        let step = create_basic_step("start-child", "child-scenario-id");
        let mut ctx = EmitContext::new(false);

        let tokens = emit(&step, &mut ctx);
        let code = tokens.to_string();

        // Should emit placeholder when no child graph is available
        assert!(code.contains("WARNING"));
        assert!(code.contains("not embedded"));
        assert!(code.contains("child-scenario-id"));
    }

    #[test]
    fn test_emit_with_child_graph() {
        let step = create_named_step("start-child", "Execute Child", "child-scenario-id");

        // Create context with child scenario registered
        let mut child_scenarios = HashMap::new();
        child_scenarios.insert("start-child".to_string(), create_child_graph("Child Graph"));

        let mut ctx = EmitContext::with_child_scenarios(false, child_scenarios, None, None);

        let tokens = emit(&step, &mut ctx);
        let code = tokens.to_string();

        // Should emit embedded version with durable wrapper
        assert!(code.contains("durable"));
        assert!(code.contains("execute_child"));
        assert!(code.contains("child_scenario_inputs"));

        // Should include cache key handling for loop indices
        assert!(code.contains("__durable_cache_key"));
        assert!(code.contains("_loop_indices"));
    }

    #[test]
    fn test_emit_default_retry_config() {
        let step = create_basic_step("start-child", "child-scenario-id");

        let mut child_scenarios = HashMap::new();
        child_scenarios.insert("start-child".to_string(), create_child_graph("Child"));

        let mut ctx = EmitContext::with_child_scenarios(false, child_scenarios, None, None);

        let tokens = emit(&step, &mut ctx);
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
        child_scenarios.insert("start-child".to_string(), create_child_graph("Child"));

        let mut ctx = EmitContext::with_child_scenarios(false, child_scenarios, None, None);

        let tokens = emit(&step, &mut ctx);
        let code = tokens.to_string();

        // Custom retry config should be used
        assert!(code.contains("max_retries = 5"));
        assert!(code.contains("delay = 2000"));
    }

    // =============================================================================
    // emit_with_embedded_child tests
    // =============================================================================

    #[test]
    fn test_emit_with_embedded_child_structure() {
        let step = create_named_step("start-child", "Execute Child", "child-scenario-id");
        let child_graph = create_child_graph("Child Graph");
        let mut ctx = EmitContext::new(false);

        let tokens = emit_with_embedded_child(&step, &child_graph, &mut ctx, 3, 1000);
        let code = tokens.to_string();

        // Check structure of generated code
        // 1. Child function definition
        assert!(code.contains("execute_child"));

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
        let mut ctx = EmitContext::new(false);

        let tokens = emit_with_embedded_child(&step, &child_graph, &mut ctx, 3, 1000);
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
        let mut ctx = EmitContext::new(false);

        let tokens = emit_with_embedded_child(&step, &child_graph, &mut ctx, 3, 1000);
        let code = tokens.to_string();

        // Should use empty object for inputs
        assert!(code.contains("Object"));
        assert!(code.contains("Map :: new"));
    }

    #[test]
    fn test_emit_with_embedded_child_result_structure() {
        let step = create_named_step("start-child", "Test Step", "child-scenario-id");
        let child_graph = create_child_graph("Child");
        let mut ctx = EmitContext::new(false);

        let tokens = emit_with_embedded_child(&step, &child_graph, &mut ctx, 3, 1000);
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
        let mut ctx = EmitContext::new(false);

        let tokens = emit_with_embedded_child(&step, &child_graph, &mut ctx, 3, 1000);
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
            "step-with.special-chars".to_string(),
            create_child_graph("Child"),
        );

        let mut ctx = EmitContext::with_child_scenarios(false, child_scenarios, None, None);

        let tokens = emit(&step, &mut ctx);
        let code = tokens.to_string();

        // Step ID should be sanitized in variable names
        assert!(code.contains("step_with_special_chars"));
    }

    #[test]
    fn test_emit_placeholder_with_special_characters() {
        let mut ctx = EmitContext::new(false);
        let tokens = emit_placeholder(
            "step.with-special",
            None,
            "Unnamed",
            "child/scenario",
            &mut ctx,
        );

        let code = tokens.to_string();

        // Should still work with special characters
        assert!(code.contains("steps_context"));
        assert!(code.contains("child/scenario"));
    }

    // =============================================================================
    // Debug mode tests
    // =============================================================================

    #[test]
    fn test_emit_debug_mode_generates_events() {
        let step = create_named_step("start-child", "Test Step", "child-scenario-id");

        let mut child_scenarios = HashMap::new();
        child_scenarios.insert("start-child".to_string(), create_child_graph("Child"));

        let mut ctx = EmitContext::with_child_scenarios(true, child_scenarios, None, None);

        let tokens = emit(&step, &mut ctx);
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
        child_scenarios.insert("step-1".to_string(), create_child_graph("Child 1"));
        child_scenarios.insert("step-2".to_string(), create_child_graph("Child 2"));

        let mut ctx = EmitContext::with_child_scenarios(false, child_scenarios, None, None);

        let tokens1 = emit(&step1, &mut ctx);
        let tokens2 = emit(&step2, &mut ctx);

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
        child_scenarios.insert("unique-step-id".to_string(), create_child_graph("Child"));

        let mut ctx = EmitContext::with_child_scenarios(false, child_scenarios, None, None);

        let tokens = emit(&step, &mut ctx);
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
        child_scenarios.insert("loop-step".to_string(), create_child_graph("Child"));

        let mut ctx = EmitContext::with_child_scenarios(false, child_scenarios, None, None);

        let tokens = emit(&step, &mut ctx);
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
        let mut ctx = EmitContext::new(false);

        let tokens = emit_with_embedded_child(&step, &child_graph, &mut ctx, 3, 1000);
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
        let mut ctx = EmitContext::new(false);

        let tokens = emit_with_embedded_child(&step, &child_graph, &mut ctx, 3, 1000);
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
    fn test_emit_placeholder_structured_cancellation() {
        let mut ctx = EmitContext::new(false);
        let tokens = emit_placeholder(
            "step-1",
            Some("Test Step"),
            "Test Step",
            "child-scenario",
            &mut ctx,
        );

        let code = tokens.to_string();

        // Placeholder should also emit structured error for interruption (cancel or pause)
        assert!(
            code.contains("STEP_INTERRUPTED"),
            "Should include STEP_INTERRUPTED error code"
        );
        assert!(
            code.contains("serde_json :: to_string"),
            "Should serialize structured error to JSON"
        );
    }

    #[test]
    fn test_emit_child_error_propagation() {
        let step = create_basic_step("start-child", "child-scenario-id");

        let mut child_scenarios = HashMap::new();
        child_scenarios.insert("start-child".to_string(), create_child_graph("Child"));

        let mut ctx = EmitContext::with_child_scenarios(false, child_scenarios, None, None);

        let tokens = emit(&step, &mut ctx);
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
        let mut ctx = EmitContext::new(false);

        let tokens = emit_with_embedded_child(&step, &child_graph, &mut ctx, 3, 1000);
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
        let mut ctx = EmitContext::new(false);

        let tokens = emit_with_embedded_child(&step, &child_graph, &mut ctx, 3, 1000);
        let code = tokens.to_string();

        // Ensure no turbofish null syntax appears in generated code
        assert!(
            !code.contains("null::<") && !code.contains("null :: <"),
            "Generated code must not contain null::<Type> - json! macro doesn't support turbofish"
        );
    }
}
