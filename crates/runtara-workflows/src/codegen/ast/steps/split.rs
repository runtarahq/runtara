// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Split step emitter.
//!
//! The Split step iterates over an array, executing a subgraph for each item.
//! The Split step uses #[resilient] macro to checkpoint its final result.
//! Individual steps within the subgraph checkpoint themselves via runtara-sdk,
//! enabling recovery mid-iteration.

use proc_macro2::TokenStream;
use quote::quote;

use super::super::CodegenError;
use super::super::context::EmitContext;
use super::super::mapping;
use super::super::program;
use super::{
    emit_breakpoint_check, emit_step_debug_end, emit_step_debug_start, emit_step_span_start,
};
use runtara_dsl::{MappingValue, SplitStep};

/// Emit code for a Split step.
pub fn emit(step: &SplitStep, ctx: &mut EmitContext) -> Result<TokenStream, CodegenError> {
    let step_id = &step.id;
    let step_name = step.name.as_deref();
    let step_name_display = step_name.unwrap_or("Unnamed");

    // Get retry configuration with defaults (0 retries for Split by default)
    let max_retries = step
        .config
        .as_ref()
        .and_then(|c| c.max_retries)
        .unwrap_or(0);
    let retry_delay = step
        .config
        .as_ref()
        .and_then(|c| c.retry_delay)
        .unwrap_or(1000);

    // Do all mutable operations first
    let step_var = ctx.declare_step(step_id);
    let source_var = ctx.temp_var("source");
    let split_inputs_var = ctx.temp_var("split_inputs");
    let subgraph_fn_name = ctx.temp_var(&format!(
        "{}_subgraph",
        EmitContext::sanitize_ident(step_id)
    ));
    let durable_fn_name =
        ctx.temp_var(&format!("{}_durable", EmitContext::sanitize_ident(step_id)));

    // Clone immutable references
    let steps_context = ctx.steps_context_var.clone();
    let inputs_var = ctx.inputs_var.clone();

    // Build the source for input mapping
    let build_source = mapping::emit_build_source(ctx);

    // Build inputs from the typed SplitConfig
    let inputs_code = if let Some(ref config) = step.config {
        // Emit mapping code for the value field
        let value_mapping: std::collections::HashMap<String, MappingValue> =
            [("value".to_string(), config.value.clone())]
                .into_iter()
                .collect();

        let mapping_code = mapping::emit_input_mapping(&value_mapping, ctx, &source_var);

        // Generate code to resolve variables mapping if present.
        //
        // CRITICAL: this references `__split_inputs` (the local Value being
        // built), not `inputs`. The interpolated `#vars_mapping_code` itself
        // emits `inputs.as_ref()` against `ctx.inputs_var` ("inputs") to
        // resolve `data.*` / `steps.*` references — that resolves to the
        // OUTER workflow `inputs: Arc<WorkflowInputs>` parameter. If we'd
        // shadowed the outer `inputs` with the local Value, the mapping
        // codegen would call `.as_ref()` on the wrong type and rustc fails
        // with E0599 (caught by smoke_split_with_variables).
        let variables_code = if let Some(ref vars) = config.variables {
            let vars_mapping_code = mapping::emit_input_mapping(vars, ctx, &source_var);
            quote! {
                if let serde_json::Value::Object(ref mut map) = __split_inputs {
                    map.insert("variables".to_string(), #vars_mapping_code);
                }
            }
        } else {
            quote! {}
        };

        // Emit the parallelism, sequential, and dontStopOnFailed as immediate values
        let parallelism = config.parallelism.unwrap_or(0);
        let sequential = config.sequential.unwrap_or(false);
        let dont_stop_on_failed = config.dont_stop_on_failed.unwrap_or(false);
        let allow_null = config.allow_null.unwrap_or(false);
        let convert_single_value = config.convert_single_value.unwrap_or(false);
        let batch_size = config.batch_size.unwrap_or(0);

        quote! {
            {
                let mut __split_inputs = #mapping_code;
                if let serde_json::Value::Object(ref mut map) = __split_inputs {
                    map.insert("parallelism".to_string(), serde_json::json!(#parallelism));
                    map.insert("sequential".to_string(), serde_json::json!(#sequential));
                    map.insert("dontStopOnFailed".to_string(), serde_json::json!(#dont_stop_on_failed));
                    map.insert("allowNull".to_string(), serde_json::json!(#allow_null));
                    map.insert("convertSingleValue".to_string(), serde_json::json!(#convert_single_value));
                    map.insert("batchSize".to_string(), serde_json::json!(#batch_size));
                }
                #variables_code
                __split_inputs
            }
        }
    } else {
        quote! { serde_json::Value::Object(serde_json::Map::new()) }
    };

    // Generate the subgraph function using shared recursive emitter
    let subgraph_code = program::emit_graph_as_function(&subgraph_fn_name, &step.subgraph, ctx)?;

    // Serialize config to JSON for debug events
    let config_json = step
        .config
        .as_ref()
        .and_then(|c| serde_json::to_string(c).ok());

    // Clone workflow inputs var for debug events (to access _loop_indices)
    let workflow_inputs_var = inputs_var.clone();

    // Split creates a scope - use sc_{step_id} as its scope_id
    let split_scope_id = format!("sc_{}", step_id);

    // Generate debug event emissions with the Split's own scope_id
    let debug_start = emit_step_debug_start(
        ctx,
        step_id,
        step_name,
        "Split",
        Some(&split_inputs_var),
        config_json.as_deref(),
        Some(&workflow_inputs_var),
        Some(&split_scope_id),
    );
    let debug_end = emit_step_debug_end(
        ctx,
        step_id,
        step_name,
        "Split",
        Some(&step_var),
        Some(&workflow_inputs_var),
        Some(&split_scope_id),
    );

    // Generate tracing span for OpenTelemetry
    let span_def = emit_step_span_start(step_id, step_name, "Split");

    // Breakpoint check after input resolution — includes resolved inputs in the event
    let breakpoint_check = if step.breakpoint.unwrap_or(false) {
        emit_breakpoint_check(step_id, step_name, "Split", ctx, Some(&split_inputs_var))
    } else {
        quote! {}
    };

    // Static base for cache key - will be combined with prefix/workflow_id at runtime
    let cache_key_base = format!("split::{}", step_id);

    // Generate the resilient function with configurable retry settings
    let max_retries_lit = max_retries;
    let retry_delay_lit = retry_delay;
    let durable_lit = ctx.durable && step.durable.unwrap_or(true);

    // Serialize input/output schemas for runtime validation. Empty schemas mean
    // "no validation" — we skip the per-iteration check entirely.
    let has_input_schema = !step.input_schema.is_empty();
    let has_output_schema = !step.output_schema.is_empty();
    let input_schema_json =
        serde_json::to_string(&step.input_schema).unwrap_or_else(|_| "{}".to_string());
    let output_schema_json =
        serde_json::to_string(&step.output_schema).unwrap_or_else(|_| "{}".to_string());

    // The validator is only emitted when at least one schema is non-empty —
    // otherwise the closure and its types are dead and would warn.
    let validator_def = if has_input_schema || has_output_schema {
        quote! {
            // Permissive schema check: required fields must be present and
            // type-compatible; extra fields are allowed.
            let __validate_required_fields = |
                value: &serde_json::Value,
                schema: &serde_json::Value,
                ctx: &str,
            | -> std::result::Result<(), String> {
                let schema_obj = match schema.as_object() {
                    Some(o) if !o.is_empty() => o,
                    _ => return Ok(()),
                };
                let value_obj = match value.as_object() {
                    Some(o) => o,
                    None => {
                        let actual = match value {
                            serde_json::Value::Null => "null",
                            serde_json::Value::Bool(_) => "boolean",
                            serde_json::Value::Number(_) => "number",
                            serde_json::Value::String(_) => "string",
                            serde_json::Value::Array(_) => "array",
                            serde_json::Value::Object(_) => "object",
                        };
                        return Err(format!("{}: expected object, got {}", ctx, actual));
                    }
                };
                let mut missing: Vec<String> = Vec::new();
                let mut wrong_type: Vec<String> = Vec::new();
                for (field_name, field_schema) in schema_obj {
                    let required = field_schema.get("required").and_then(|v| v.as_bool()).unwrap_or(false);
                    let field_type = field_schema.get("type").and_then(|v| v.as_str()).unwrap_or("");
                    match value_obj.get(field_name) {
                        None => {
                            if required {
                                missing.push(field_name.clone());
                            }
                        }
                        Some(actual_value) => {
                            if !field_type.is_empty() && !actual_value.is_null() {
                                let matches = match field_type {
                                    "string" => actual_value.is_string(),
                                    "integer" => actual_value.is_i64() || actual_value.is_u64(),
                                    "number" => actual_value.is_number(),
                                    "boolean" => actual_value.is_boolean(),
                                    "array" => actual_value.is_array(),
                                    "object" => actual_value.is_object(),
                                    _ => true,
                                };
                                if !matches {
                                    let actual_type = match actual_value {
                                        serde_json::Value::Null => "null",
                                        serde_json::Value::Bool(_) => "boolean",
                                        serde_json::Value::Number(_) => "number",
                                        serde_json::Value::String(_) => "string",
                                        serde_json::Value::Array(_) => "array",
                                        serde_json::Value::Object(_) => "object",
                                    };
                                    wrong_type.push(format!(
                                        "'{}' (expected {}, got {})",
                                        field_name, field_type, actual_type
                                    ));
                                }
                            }
                        }
                    }
                }
                if missing.is_empty() && wrong_type.is_empty() {
                    return Ok(());
                }
                let mut parts: Vec<String> = Vec::new();
                if !missing.is_empty() {
                    let mut got: Vec<String> = value_obj.keys().cloned().collect();
                    got.sort();
                    parts.push(format!(
                        "required field(s) [{}] missing (got fields: [{}])",
                        missing.join(", "), got.join(", ")
                    ));
                }
                if !wrong_type.is_empty() {
                    parts.push(format!("type mismatches: {}", wrong_type.join(", ")));
                }
                Err(format!("{}: {}", ctx, parts.join("; ")))
            };
        }
    } else {
        quote! {}
    };

    let input_schema_setup = if has_input_schema {
        quote! {
            let __split_input_schema: serde_json::Value =
                serde_json::from_str(#input_schema_json)
                    .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));
        }
    } else {
        quote! {}
    };

    let output_schema_setup = if has_output_schema {
        quote! {
            let __split_output_schema: serde_json::Value =
                serde_json::from_str(#output_schema_json)
                    .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));
        }
    } else {
        quote! {}
    };

    // Sequential-path checks. These return Err strings that the caller routes
    // through the existing dont_stop_on_failed branch.
    let seq_input_check = if has_input_schema {
        quote! {
            if let Err(__err) = __validate_required_fields(
                item,
                &__split_input_schema,
                &format!("Split '{}' iteration {}: input", step_id, idx),
            ) {
                if dont_stop_on_failed {
                    errors.push(serde_json::json!({"error": __err, "index": idx}));
                    return Ok::<_, String>(());
                } else {
                    return Err(__err);
                }
            }
        }
    } else {
        quote! {}
    };

    let seq_output_check = if has_output_schema {
        quote! {
            if let Err(__err) = __validate_required_fields(
                &__result_value,
                &__split_output_schema,
                &format!("Split '{}' iteration {}: output", step_id, idx),
            ) {
                if dont_stop_on_failed {
                    errors.push(serde_json::json!({"error": __err, "index": idx}));
                } else {
                    return Err(__err);
                }
            } else {
                results.push(__result_value);
            }
        }
    } else {
        quote! { results.push(__result_value); }
    };

    // Parallel-path checks. Inside the spawn closure we only have access to
    // the schema via Arc (so the closures stay 'static-friendly). We turn
    // a validation failure into Err(...) and let the post-join loop dispatch
    // it through dont_stop_on_failed like any other iteration error.
    let par_input_setup = if has_input_schema {
        quote! {
            let __input_schema_arc = std::sync::Arc::new(__split_input_schema.clone());
        }
    } else {
        quote! {}
    };
    let par_output_setup = if has_output_schema {
        quote! {
            let __output_schema_arc = std::sync::Arc::new(__split_output_schema.clone());
        }
    } else {
        quote! {}
    };
    let par_input_clone = if has_input_schema {
        quote! { let __input_schema_arc = __input_schema_arc.clone(); }
    } else {
        quote! {}
    };
    let par_output_clone = if has_output_schema {
        quote! { let __output_schema_arc = __output_schema_arc.clone(); }
    } else {
        quote! {}
    };
    let par_input_check = if has_input_schema {
        quote! {
            // Inline validator (matches the sequential one above) so the
            // spawned thread doesn't borrow the outer closure.
            let __validate = |value: &serde_json::Value, schema: &serde_json::Value, ctx: &str| -> std::result::Result<(), String> {
                let schema_obj = match schema.as_object() {
                    Some(o) if !o.is_empty() => o,
                    _ => return Ok(()),
                };
                let value_obj = match value.as_object() {
                    Some(o) => o,
                    None => return Err(format!("{}: expected object", ctx)),
                };
                let mut missing: Vec<String> = Vec::new();
                let mut wrong_type: Vec<String> = Vec::new();
                for (field_name, field_schema) in schema_obj {
                    let required = field_schema.get("required").and_then(|v| v.as_bool()).unwrap_or(false);
                    let field_type = field_schema.get("type").and_then(|v| v.as_str()).unwrap_or("");
                    match value_obj.get(field_name) {
                        None => if required { missing.push(field_name.clone()); }
                        Some(av) => {
                            if !field_type.is_empty() && !av.is_null() {
                                let ok = match field_type {
                                    "string" => av.is_string(),
                                    "integer" => av.is_i64() || av.is_u64(),
                                    "number" => av.is_number(),
                                    "boolean" => av.is_boolean(),
                                    "array" => av.is_array(),
                                    "object" => av.is_object(),
                                    _ => true,
                                };
                                if !ok { wrong_type.push(format!("'{}' (expected {})", field_name, field_type)); }
                            }
                        }
                    }
                }
                if missing.is_empty() && wrong_type.is_empty() { return Ok(()); }
                let mut parts: Vec<String> = Vec::new();
                if !missing.is_empty() {
                    let mut got: Vec<String> = value_obj.keys().cloned().collect();
                    got.sort();
                    parts.push(format!("required field(s) [{}] missing (got fields: [{}])", missing.join(", "), got.join(", ")));
                }
                if !wrong_type.is_empty() { parts.push(format!("type mismatches: {}", wrong_type.join(", "))); }
                Err(format!("{}: {}", ctx, parts.join("; ")))
            };
            if let Err(e) = __validate(&item, &*__input_schema_arc, &format!("Split '{}' iteration {}: input", step_id, idx)) {
                return (idx, Err(e));
            }
        }
    } else {
        quote! {}
    };
    let par_output_check = if has_output_schema {
        quote! {
            // Reuse the same inline validator shape used for input.
            let __validate = |value: &serde_json::Value, schema: &serde_json::Value, ctx: &str| -> std::result::Result<(), String> {
                let schema_obj = match schema.as_object() {
                    Some(o) if !o.is_empty() => o,
                    _ => return Ok(()),
                };
                let value_obj = match value.as_object() {
                    Some(o) => o,
                    None => return Err(format!("{}: expected object", ctx)),
                };
                let mut missing: Vec<String> = Vec::new();
                let mut wrong_type: Vec<String> = Vec::new();
                for (field_name, field_schema) in schema_obj {
                    let required = field_schema.get("required").and_then(|v| v.as_bool()).unwrap_or(false);
                    let field_type = field_schema.get("type").and_then(|v| v.as_str()).unwrap_or("");
                    match value_obj.get(field_name) {
                        None => if required { missing.push(field_name.clone()); }
                        Some(av) => {
                            if !field_type.is_empty() && !av.is_null() {
                                let ok = match field_type {
                                    "string" => av.is_string(),
                                    "integer" => av.is_i64() || av.is_u64(),
                                    "number" => av.is_number(),
                                    "boolean" => av.is_boolean(),
                                    "array" => av.is_array(),
                                    "object" => av.is_object(),
                                    _ => true,
                                };
                                if !ok { wrong_type.push(format!("'{}' (expected {})", field_name, field_type)); }
                            }
                        }
                    }
                }
                if missing.is_empty() && wrong_type.is_empty() { return Ok(()); }
                let mut parts: Vec<String> = Vec::new();
                if !missing.is_empty() {
                    let mut got: Vec<String> = value_obj.keys().cloned().collect();
                    got.sort();
                    parts.push(format!("required field(s) [{}] missing (got fields: [{}])", missing.join(", "), got.join(", ")));
                }
                if !wrong_type.is_empty() { parts.push(format!("type mismatches: {}", wrong_type.join(", "))); }
                Err(format!("{}: {}", ctx, parts.join("; ")))
            };
            let __out_value = match result {
                Ok(v) => v,
                Err(e) => return (idx, Err(e)),
            };
            if let Err(e) = __validate(&__out_value, &*__output_schema_arc, &format!("Split '{}' iteration {}: output", step_id, idx)) {
                return (idx, Err(e));
            }
            let result: std::result::Result<serde_json::Value, String> = Ok(__out_value);
        }
    } else {
        quote! {}
    };

    Ok(quote! {
        let #source_var = #build_source;
        let #split_inputs_var = #inputs_code;

        // Breakpoint (after input resolution, before execution)
        #breakpoint_check

        // Build cache key dynamically, including prefix and loop indices
        let __split_cache_key = {
            // Get prefix from parent context (set by EmbedWorkflow)
            let prefix = (*#inputs_var.variables)
                .as_object()
                .and_then(|vars| vars.get("_cache_key_prefix"))
                .and_then(|v| v.as_str())
                .unwrap_or("");

            let base = #cache_key_base;
            let indices_suffix = (*#inputs_var.variables)
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
                // No cache prefix - use _workflow_id to prevent collisions between
                // independent workflows running the same split steps
                let workflow_id = (*#inputs_var.variables)
                    .as_object()
                    .and_then(|vars| vars.get("_workflow_id"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("root");
                format!("{}::{}{}", workflow_id, base, indices_suffix)
            } else {
                format!("{}::{}{}", prefix, base, indices_suffix)
            }
        };

        // Define tracing span for this step
        #span_def

        // Wrap step execution in span scope
        __step_span.in_scope(|| -> std::result::Result<(), String> {
            #debug_start

            // Extract split configuration
        let allow_null = #split_inputs_var.get("allowNull")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let convert_single_value = #split_inputs_var.get("convertSingleValue")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let mut split_array = match #split_inputs_var.get("value") {
            Some(serde_json::Value::Array(arr)) => arr.clone(),
            Some(serde_json::Value::Null) | None => {
                if allow_null {
                    vec![]
                } else {
                    return Err(format!(
                        "Split step '{}' received null value. Set 'allowNull: true' to allow empty iterations, or use 'transform/ensure-array' agent.",
                        #step_id
                    ));
                }
            }
            Some(other) => {
                if convert_single_value {
                    vec![other.clone()]
                } else {
                    return Err(format!(
                        "Split step '{}' expected array, got {}. Set 'convertSingleValue: true' to auto-wrap, or use 'transform/ensure-array' agent.",
                        #step_id,
                        match other {
                            serde_json::Value::Object(_) => "object",
                            serde_json::Value::String(_) => "string",
                            serde_json::Value::Number(_) => "number",
                            serde_json::Value::Bool(_) => "boolean",
                            _ => "unknown",
                        }
                    ));
                }
            }
        };

        // Apply batching: when batchSize > 0, chunk elements into sub-arrays so
        // each iteration receives a batch instead of a single element.
        // [1,2,3,4,5] with batchSize=2 becomes [[1,2],[3,4],[5]].
        let batch_size = #split_inputs_var.get("batchSize")
            .and_then(|v| v.as_i64())
            .unwrap_or(0) as usize;
        if batch_size > 0 {
            split_array = split_array
                .chunks(batch_size)
                .map(|chunk| serde_json::Value::Array(chunk.to_vec()))
                .collect();
        }

        let parallelism = #split_inputs_var.get("parallelism")
            .and_then(|v| v.as_i64())
            .unwrap_or(0) as usize;
        let sequential = #split_inputs_var.get("sequential")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let dont_stop_on_failed = #split_inputs_var.get("dontStopOnFailed")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        // Extract extra variables from input mapping (for passing config to subgraphs)
        let extra_variables = #split_inputs_var.get("variables")
            .cloned()
            .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));

        // Define the subgraph function
        #subgraph_code

        // Define the resilient split execution function
        #[resilient(durable = #durable_lit, max_retries = #max_retries_lit, delay = #retry_delay_lit)]
        fn #durable_fn_name(
            cache_key: &str,
            split_array: Vec<serde_json::Value>,
            variables_base: serde_json::Value,
            extra_variables: serde_json::Value,
            dont_stop_on_failed: bool,
            parallelism: usize,
            sequential: bool,
            step_id: &str,
            step_name: &str,
        ) -> std::result::Result<serde_json::Value, String> {
            let mut results: Vec<serde_json::Value> = Vec::with_capacity(split_array.len());
            let mut errors: Vec<serde_json::Value> = Vec::new();

            // Per-iteration schema validation setup. These blocks expand only
            // when input_schema or output_schema is non-empty on the SplitStep.
            #input_schema_setup
            #output_schema_setup
            #validator_def

            // Helper to build iteration inputs
            let build_iteration_inputs = |idx: usize, item: &serde_json::Value, variables_base: &serde_json::Value, extra_variables: &serde_json::Value| {
                let mut merged_vars = match variables_base {
                    serde_json::Value::Object(base) => base.clone(),
                    _ => serde_json::Map::new(),
                };
                if let serde_json::Value::Object(extra) = extra_variables {
                    for (k, v) in extra {
                        merged_vars.insert(k.clone(), v.clone());
                    }
                }

                // Build cumulative loop indices array for cache key uniqueness in nested loops
                let parent_indices = merged_vars.get("_loop_indices")
                    .and_then(|v| v.as_array())
                    .cloned()
                    .unwrap_or_default();
                let mut all_indices = parent_indices;
                all_indices.push(serde_json::json!(idx));
                merged_vars.insert("_loop_indices".to_string(), serde_json::json!(all_indices));

                // Inject iteration index as _index (0-based) for backward compatibility
                merged_vars.insert("_index".to_string(), serde_json::json!(idx));

                // Generate scope ID for this iteration
                let __iteration_scope_id = {
                    let parent_scope = merged_vars.get("_scope_id")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());

                    if let Some(parent) = parent_scope {
                        format!("{}_{}_{}", parent, step_id, idx)
                    } else {
                        format!("sc_{}_{}", step_id, idx)
                    }
                };

                // Inject _scope_id into subgraph variables (iteration-specific for cache key uniqueness)
                merged_vars.insert("_scope_id".to_string(), serde_json::json!(__iteration_scope_id));

                // Inner steps use the Split's scope (sc_{step_id}) as their parent, NOT the iteration scope.
                let __split_scope_id = format!("sc_{}", step_id);
                WorkflowInputs {
                    data: Arc::new(item.clone()),
                    variables: Arc::new(serde_json::Value::Object(merged_vars)),
                    parent_scope_id: Some(__split_scope_id),
                }
            };

            if parallelism <= 1 || cfg!(target_family = "wasm") {
                // Sequential execution (always used for WASM, or when parallelism <= 1)
                for (idx, item) in split_array.iter().enumerate() {
                    let __iter_span = tracing::info_span!(
                        "split.iteration",
                        step.id = step_id,
                        iteration.index = idx,
                        otel.kind = "INTERNAL"
                    );

                    __iter_span.in_scope(|| -> std::result::Result<(), String> {
                        if runtara_sdk::is_cancelled() {
                            return Err(format!("Split step {} cancelled before iteration {}", step_id, idx));
                        }

                        // Input schema check (no-op when input_schema is empty).
                        #seq_input_check

                        let subgraph_inputs = build_iteration_inputs(idx, item, &variables_base, &extra_variables);

                        match #subgraph_fn_name(Arc::new(subgraph_inputs)) {
                            Ok(__result_value) => {
                                // Output schema check (no-op when output_schema
                                // is empty; pushes the value into results on
                                // success, routes errors via dont_stop_on_failed).
                                #seq_output_check
                            }
                            Err(e) => {
                                if dont_stop_on_failed {
                                    errors.push(serde_json::json!({"error": e, "index": idx}));
                                } else {
                                    tracing::error!("Split iteration {} failed: {}", idx, e);
                                    return Err(format!("Split step failed at index {}: {}", idx, e));
                                }
                            }
                        }

                        {
                            let mut __sdk = sdk().lock().unwrap();
                            if let Err(e) = __sdk.check_signals() {
                                return Err(format!("Split step {} at iteration {}: {}", step_id, idx, e));
                            }
                        }

                        Ok::<_, String>(())
                    })?;
                }
            } else {
                // Parallel execution via OS threads (native only, not available in WASM)
                use std::sync::atomic::{AtomicBool, Ordering};

                let cancel_token = Arc::new(AtomicBool::new(false));
                let variables_base = Arc::new(variables_base);
                let extra_variables = Arc::new(extra_variables);
                let step_id_owned = step_id.to_string();

                // Wrap schemas in Arcs so each spawned thread gets its own
                // cheap reference. Only emitted when the schema is non-empty.
                #par_input_setup
                #par_output_setup

                let thread_results: Vec<(usize, std::result::Result<serde_json::Value, String>)> = std::thread::scope(|s| {
                    let handles: Vec<_> = split_array.iter().enumerate()
                        .map(|(idx, item)| {
                            let cancel_token = cancel_token.clone();
                            let variables_base = variables_base.clone();
                            let extra_variables = extra_variables.clone();
                            let step_id = step_id_owned.clone();
                            let item = item.clone();
                            #par_input_clone
                            #par_output_clone

                            s.spawn(move || {
                                let __iter_span = tracing::info_span!(
                                    "split.iteration",
                                    step.id = %step_id,
                                    iteration.index = idx,
                                    otel.kind = "INTERNAL"
                                );

                                __iter_span.in_scope(|| {
                                    if cancel_token.load(Ordering::Relaxed) || runtara_sdk::is_cancelled() {
                                        return (idx, Err("Cancelled".to_string()));
                                    }

                                    // Input schema check (no-op when not configured).
                                    #par_input_check

                                    let subgraph_inputs = build_iteration_inputs(idx, &item, &variables_base, &extra_variables);
                                    let result = #subgraph_fn_name(Arc::new(subgraph_inputs));

                                    {
                                        let mut __sdk = sdk().lock().unwrap();
                                        if let Err(e) = __sdk.check_signals() {
                                            cancel_token.store(true, Ordering::Relaxed);
                                            return (idx, Err(format!("{}", e)));
                                        }
                                    }

                                    // Output schema check. When configured this
                                    // unwraps `result`, validates, and rebinds.
                                    #par_output_check

                                    (idx, result)
                                })
                            })
                        })
                        .collect();

                    handles.into_iter()
                        .map(|h| h.join().unwrap())
                        .collect()
                });

                let mut sorted_results = thread_results;
                sorted_results.sort_by_key(|(idx, _)| *idx);

                for (idx, result) in sorted_results {
                    match result {
                        Ok(value) => results.push(value),
                        Err(e) => {
                            if e.starts_with("Cancelled") || e.starts_with("Aborted") {
                                continue;
                            }
                            if dont_stop_on_failed {
                                errors.push(serde_json::json!({"error": e, "index": idx}));
                            } else {
                                tracing::error!("Split iteration {} failed: {}", idx, e);
                                return Err(format!("Split step failed at index {}: {}", idx, e));
                            }
                        }
                    }
                }
            }

            let step_result = if dont_stop_on_failed {
                serde_json::json!({
                    "stepId": step_id,
                    "stepName": step_name,
                    "stepType": "Split",
                    "data": {
                        "success": &results,
                        "error": &errors,
                        "aborted": [],
                        "unknown": [],
                        "skipped": []
                    },
                    "stats": {
                        "success": results.len(),
                        "error": errors.len(),
                        "aborted": 0,
                        "unknown": 0,
                        "skipped": 0,
                        "total": split_array.len()
                    },
                    "outputs": &results
                })
            } else {
                __step_output_envelope(
                    step_id,
                    step_name,
                    "Split",
                    &serde_json::Value::Array(results.clone()),
                )
            };

            Ok(step_result)
        }

        // Execute the durable split function
        let variables_base = (*#inputs_var.variables).clone();
        let #step_var = #durable_fn_name(
            &__split_cache_key,
            split_array,
            variables_base,
            extra_variables,
            dont_stop_on_failed,
            parallelism,
            sequential,
            #step_id,
            #step_name_display,
        )?;

            #debug_end

            #steps_context.insert(#step_id.to_string(), #step_var.clone());

            Ok::<_, String>(())
        })?;
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtara_dsl::{
        ExecutionGraph, FinishStep, ImmediateValue, MappingValue, ReferenceValue, SchemaField,
        SchemaFieldType, SplitConfig, Step,
    };
    use std::collections::HashMap;

    /// Helper to create a minimal ExecutionGraph with just a Finish step
    fn create_minimal_graph(entry_point: &str) -> ExecutionGraph {
        let mut steps = HashMap::new();
        steps.insert(
            entry_point.to_string(),
            Step::Finish(FinishStep {
                id: entry_point.to_string(),
                name: Some("Finish".to_string()),
                input_mapping: None,
                breakpoint: None,
            }),
        );

        ExecutionGraph {
            name: None,
            description: None,
            entry_point: entry_point.to_string(),
            steps,
            execution_plan: vec![],
            variables: HashMap::new(),
            input_schema: HashMap::new(),
            output_schema: HashMap::new(),
            notes: None,
            nodes: None,
            edges: None,
            ..Default::default()
        }
    }

    /// Helper to create a split step with array reference
    fn create_split_step(step_id: &str, array_ref: &str) -> SplitStep {
        SplitStep {
            id: step_id.to_string(),
            name: Some("Test Split".to_string()),
            config: Some(SplitConfig {
                value: MappingValue::Reference(ReferenceValue {
                    value: array_ref.to_string(),
                    type_hint: None,
                    default: None,
                }),
                variables: None,
                parallelism: None,
                sequential: None,
                dont_stop_on_failed: None,
                max_retries: None,
                retry_delay: None,
                timeout: None,
                allow_null: None,
                convert_single_value: None,
                batch_size: None,
            }),
            subgraph: Box::new(create_minimal_graph("finish")),
            input_schema: HashMap::new(),
            output_schema: HashMap::new(),
            breakpoint: None,
            durable: None,
        }
    }

    #[test]
    fn test_emit_basic_split_structure() {
        let mut ctx = EmitContext::new(false);
        let split_step = create_split_step("split-1", "data.items");

        let tokens = emit(&split_step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Verify basic structure
        assert!(code.contains("split-1"), "Should contain step ID");
        assert!(code.contains("split_array"), "Should extract split array");
        assert!(code.contains("_durable"), "Should have durable function");
    }

    #[test]
    fn test_emit_split_default_config() {
        let mut ctx = EmitContext::new(false);
        let split_step = SplitStep {
            id: "split-default".to_string(),
            name: None,
            config: None, // No config
            subgraph: Box::new(create_minimal_graph("finish")),
            input_schema: HashMap::new(),
            output_schema: HashMap::new(),
            breakpoint: None,
            durable: None,
        };

        let tokens = emit(&split_step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Should have empty object as inputs when no config
        assert!(
            code.contains("serde_json :: Value :: Object"),
            "Should create empty object when no config"
        );
    }

    #[test]
    fn test_emit_split_parallelism_config() {
        let mut ctx = EmitContext::new(false);
        let split_step = SplitStep {
            id: "split-parallel".to_string(),
            name: Some("Parallel Split".to_string()),
            config: Some(SplitConfig {
                value: MappingValue::Reference(ReferenceValue {
                    value: "data.items".to_string(),
                    type_hint: None,
                    default: None,
                }),
                variables: None,
                parallelism: Some(4),
                sequential: Some(false),
                dont_stop_on_failed: Some(true),
                max_retries: None,
                retry_delay: None,
                timeout: None,
                allow_null: None,
                convert_single_value: None,
                batch_size: None,
            }),
            subgraph: Box::new(create_minimal_graph("finish")),
            input_schema: HashMap::new(),
            output_schema: HashMap::new(),
            breakpoint: None,
            durable: None,
        };

        let tokens = emit(&split_step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Verify parallelism config is included
        assert!(
            code.contains("\"parallelism\""),
            "Should include parallelism config"
        );
        assert!(
            code.contains("\"sequential\""),
            "Should include sequential config"
        );
        assert!(
            code.contains("\"dontStopOnFailed\""),
            "Should include dontStopOnFailed config"
        );
    }

    #[test]
    fn test_emit_split_variables_mapping() {
        let mut ctx = EmitContext::new(false);

        let mut variables = HashMap::new();
        variables.insert(
            "customVar".to_string(),
            MappingValue::Immediate(ImmediateValue {
                value: serde_json::json!("test-value"),
            }),
        );

        let split_step = SplitStep {
            id: "split-vars".to_string(),
            name: Some("Split with Variables".to_string()),
            config: Some(SplitConfig {
                value: MappingValue::Reference(ReferenceValue {
                    value: "data.items".to_string(),
                    type_hint: None,
                    default: None,
                }),
                variables: Some(variables),
                parallelism: None,
                sequential: None,
                dont_stop_on_failed: None,
                max_retries: None,
                retry_delay: None,
                timeout: None,
                allow_null: None,
                convert_single_value: None,
                batch_size: None,
            }),
            subgraph: Box::new(create_minimal_graph("finish")),
            input_schema: HashMap::new(),
            output_schema: HashMap::new(),
            breakpoint: None,
            durable: None,
        };

        let tokens = emit(&split_step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Verify variables mapping is included
        assert!(
            code.contains("\"variables\""),
            "Should include variables mapping"
        );
    }

    #[test]
    fn test_emit_split_retry_config() {
        let mut ctx = EmitContext::new(false);
        let split_step = SplitStep {
            id: "split-retry".to_string(),
            name: Some("Retry Split".to_string()),
            config: Some(SplitConfig {
                value: MappingValue::Reference(ReferenceValue {
                    value: "data.items".to_string(),
                    type_hint: None,
                    default: None,
                }),
                variables: None,
                parallelism: None,
                sequential: None,
                dont_stop_on_failed: None,
                max_retries: Some(5),
                retry_delay: Some(2000),
                timeout: None,
                allow_null: None,
                convert_single_value: None,
                batch_size: None,
            }),
            subgraph: Box::new(create_minimal_graph("finish")),
            input_schema: HashMap::new(),
            output_schema: HashMap::new(),
            breakpoint: None,
            durable: None,
        };

        let tokens = emit(&split_step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Verify retry config in resilient macro
        assert!(
            code.contains("max_retries = 5"),
            "Should include custom max_retries"
        );
        assert!(
            code.contains("delay = 2000"),
            "Should include custom retry delay"
        );
    }

    #[test]
    fn test_emit_split_durable_function() {
        let mut ctx = EmitContext::new(false);
        let split_step = create_split_step("split-durable", "data.items");

        let tokens = emit(&split_step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Verify resilient function is generated
        // Token stream formats attributes as "# [resilient" with spaces
        assert!(
            code.contains("# [resilient") || code.contains("#[resilient"),
            "Should have resilient macro"
        );
        assert!(code.contains("fn "), "Should be a function");
        assert!(
            code.contains("cache_key"),
            "Should have cache_key parameter"
        );
    }

    #[test]
    fn test_emit_split_loop_indices() {
        let mut ctx = EmitContext::new(false);
        let split_step = create_split_step("split-indices", "data.items");

        let tokens = emit(&split_step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Verify _loop_indices is tracked for nested loops
        assert!(
            code.contains("_loop_indices"),
            "Should track _loop_indices for cache key uniqueness"
        );
        assert!(
            code.contains("_index"),
            "Should inject _index for backward compatibility"
        );
    }

    #[test]
    fn test_emit_split_subgraph_function() {
        let mut ctx = EmitContext::new(false);
        let split_step = create_split_step("split-subgraph", "data.items");

        let tokens = emit(&split_step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Verify subgraph function is generated
        assert!(
            code.contains("_subgraph"),
            "Should generate subgraph function"
        );
        assert!(
            code.contains("WorkflowInputs"),
            "Should use WorkflowInputs for subgraph"
        );
    }

    #[test]
    fn test_emit_split_error_handling() {
        let mut ctx = EmitContext::new(false);
        let split_step = SplitStep {
            id: "split-errors".to_string(),
            name: Some("Error Handling Split".to_string()),
            config: Some(SplitConfig {
                value: MappingValue::Reference(ReferenceValue {
                    value: "data.items".to_string(),
                    type_hint: None,
                    default: None,
                }),
                variables: None,
                parallelism: None,
                sequential: None,
                dont_stop_on_failed: Some(true),
                max_retries: None,
                retry_delay: None,
                timeout: None,
                allow_null: None,
                convert_single_value: None,
                batch_size: None,
            }),
            subgraph: Box::new(create_minimal_graph("finish")),
            input_schema: HashMap::new(),
            output_schema: HashMap::new(),
            breakpoint: None,
            durable: None,
        };

        let tokens = emit(&split_step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Verify error handling structure
        assert!(
            code.contains("dont_stop_on_failed"),
            "Should check dont_stop_on_failed flag"
        );
        assert!(code.contains("errors"), "Should track errors array");
    }

    #[test]
    fn test_emit_split_output_structure() {
        let mut ctx = EmitContext::new(false);
        let split_step = create_split_step("split-output", "data.items");

        let tokens = emit(&split_step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Verify output JSON structure is built via shared helper.
        assert!(
            code.contains("__step_output_envelope"),
            "Should build output envelope"
        );
        assert!(code.contains("\"Split\""), "Should have stepType = Split");
    }

    #[test]
    fn test_emit_split_signal_check() {
        let mut ctx = EmitContext::new(false);
        let split_step = create_split_step("split-cancel", "data.items");

        let tokens = emit(&split_step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Verify signals (cancel/pause) are checked
        assert!(
            code.contains("check_signals"),
            "Should check for signals (cancel/pause) after each iteration"
        );
    }

    #[test]
    fn test_emit_split_stores_in_steps_context() {
        let mut ctx = EmitContext::new(false);
        let split_step = create_split_step("split-store", "data.items");

        let tokens = emit(&split_step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Verify result is stored in steps_context
        assert!(
            code.contains("steps_context . insert"),
            "Should store result in steps_context"
        );
    }

    #[test]
    fn test_emit_split_track_events_enabled() {
        let mut ctx = EmitContext::new(true); // debug mode ON
        let split_step = create_split_step("split-debug", "data.items");

        let tokens = emit(&split_step, &mut ctx).unwrap();
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
    fn test_emit_split_with_immediate_array() {
        let mut ctx = EmitContext::new(false);
        let split_step = SplitStep {
            id: "split-immediate".to_string(),
            name: Some("Immediate Array Split".to_string()),
            config: Some(SplitConfig {
                value: MappingValue::Immediate(ImmediateValue {
                    value: serde_json::json!([1, 2, 3, 4, 5]),
                }),
                variables: None,
                parallelism: None,
                sequential: None,
                dont_stop_on_failed: None,
                max_retries: None,
                retry_delay: None,
                timeout: None,
                allow_null: None,
                convert_single_value: None,
                batch_size: None,
            }),
            subgraph: Box::new(create_minimal_graph("finish")),
            input_schema: HashMap::new(),
            output_schema: HashMap::new(),
            breakpoint: None,
            durable: None,
        };

        let tokens = emit(&split_step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Verify immediate value is used
        assert!(
            code.contains("1") && code.contains("2") && code.contains("3"),
            "Should include immediate array values"
        );
    }

    #[test]
    fn test_emit_split_cache_key() {
        let mut ctx = EmitContext::new(false);
        let split_step = create_split_step("my-split-step", "data.items");

        let tokens = emit(&split_step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Verify cache key format
        assert!(
            code.contains("split::my-split-step"),
            "Should have cache key with split:: prefix and step ID"
        );
    }

    #[test]
    fn test_emit_split_with_unnamed_step() {
        let mut ctx = EmitContext::new(false);
        let split_step = SplitStep {
            id: "split-unnamed".to_string(),
            name: None, // No name
            config: Some(SplitConfig {
                value: MappingValue::Reference(ReferenceValue {
                    value: "data.items".to_string(),
                    type_hint: None,
                    default: None,
                }),
                variables: None,
                parallelism: None,
                sequential: None,
                dont_stop_on_failed: None,
                max_retries: None,
                retry_delay: None,
                timeout: None,
                allow_null: None,
                convert_single_value: None,
                batch_size: None,
            }),
            subgraph: Box::new(create_minimal_graph("finish")),
            input_schema: HashMap::new(),
            output_schema: HashMap::new(),
            breakpoint: None,
            durable: None,
        };

        let tokens = emit(&split_step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Should use "Unnamed" as display name
        assert!(
            code.contains("\"Unnamed\""),
            "Should use 'Unnamed' for unnamed steps"
        );
    }

    #[test]
    fn test_emit_split_parallel_execution_code() {
        let mut ctx = EmitContext::new(false);
        let split_step = SplitStep {
            id: "split-parallel".to_string(),
            name: Some("Parallel Split".to_string()),
            config: Some(SplitConfig {
                value: MappingValue::Reference(ReferenceValue {
                    value: "data.items".to_string(),
                    type_hint: None,
                    default: None,
                }),
                variables: None,
                parallelism: Some(10),
                sequential: Some(false),
                dont_stop_on_failed: None,
                max_retries: None,
                retry_delay: None,
                timeout: None,
                allow_null: None,
                convert_single_value: None,
                batch_size: None,
            }),
            subgraph: Box::new(create_minimal_graph("finish")),
            input_schema: HashMap::new(),
            output_schema: HashMap::new(),
            breakpoint: None,
            durable: None,
        };

        let tokens = emit(&split_step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Verify parallel execution code is generated
        assert!(
            code.contains("if parallelism <= 1"),
            "Should have conditional for parallel vs sequential execution"
        );
        // Native: parallel via std::thread::scope; WASM: sequential fallback
        assert!(
            code.contains("thread :: scope"),
            "Should use std::thread::scope for parallel execution on native"
        );
        assert!(
            code.contains("target_family = \"wasm\""),
            "Should have WASM sequential fallback"
        );
    }

    #[test]
    fn test_emit_split_parallel_with_sequential_flag() {
        let mut ctx = EmitContext::new(false);
        let split_step = SplitStep {
            id: "split-seq-parallel".to_string(),
            name: Some("Sequential Parallel Split".to_string()),
            config: Some(SplitConfig {
                value: MappingValue::Reference(ReferenceValue {
                    value: "data.items".to_string(),
                    type_hint: None,
                    default: None,
                }),
                variables: None,
                parallelism: Some(5),
                sequential: Some(true), // Sequential ordering with parallelism
                dont_stop_on_failed: None,
                max_retries: None,
                retry_delay: None,
                timeout: None,
                allow_null: None,
                convert_single_value: None,
                batch_size: None,
            }),
            subgraph: Box::new(create_minimal_graph("finish")),
            input_schema: HashMap::new(),
            output_schema: HashMap::new(),
            breakpoint: None,
            durable: None,
        };

        let tokens = emit(&split_step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Verify the code includes parallelism handling
        assert!(
            code.contains("parallelism"),
            "Should include parallelism config"
        );
    }

    #[test]
    fn test_emit_split_parallel_cancellation_tokens() {
        let mut ctx = EmitContext::new(false);
        let split_step = SplitStep {
            id: "split-cancel-parallel".to_string(),
            name: Some("Cancellable Parallel Split".to_string()),
            config: Some(SplitConfig {
                value: MappingValue::Reference(ReferenceValue {
                    value: "data.items".to_string(),
                    type_hint: None,
                    default: None,
                }),
                variables: None,
                parallelism: Some(10),
                sequential: None,
                dont_stop_on_failed: Some(false),
                max_retries: None,
                retry_delay: None,
                timeout: None,
                allow_null: None,
                convert_single_value: None,
                batch_size: None,
            }),
            subgraph: Box::new(create_minimal_graph("finish")),
            input_schema: HashMap::new(),
            output_schema: HashMap::new(),
            breakpoint: None,
            durable: None,
        };

        let tokens = emit(&split_step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Verify cancellation token is present for parallel path
        assert!(
            code.contains("cancel_token"),
            "Should have cancel_token for parallel execution"
        );
        assert!(
            code.contains("AtomicBool"),
            "Should use AtomicBool for thread-safe cancellation"
        );
    }

    #[test]
    fn test_emit_split_parallel_result_sorting() {
        let mut ctx = EmitContext::new(false);
        let split_step = SplitStep {
            id: "split-sort".to_string(),
            name: Some("Sorted Results Split".to_string()),
            config: Some(SplitConfig {
                value: MappingValue::Reference(ReferenceValue {
                    value: "data.items".to_string(),
                    type_hint: None,
                    default: None,
                }),
                variables: None,
                parallelism: Some(10),
                sequential: Some(false),
                dont_stop_on_failed: None,
                max_retries: None,
                retry_delay: None,
                timeout: None,
                allow_null: None,
                convert_single_value: None,
                batch_size: None,
            }),
            subgraph: Box::new(create_minimal_graph("finish")),
            input_schema: HashMap::new(),
            output_schema: HashMap::new(),
            breakpoint: None,
            durable: None,
        };

        let tokens = emit(&split_step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Verify results are sorted by index after collection
        assert!(
            code.contains("sort_by_key"),
            "Should sort results by index to maintain original order"
        );
    }

    #[test]
    fn test_emit_split_parallel_passes_parameters() {
        let mut ctx = EmitContext::new(false);
        let split_step = SplitStep {
            id: "split-params".to_string(),
            name: Some("Parameterized Split".to_string()),
            config: Some(SplitConfig {
                value: MappingValue::Reference(ReferenceValue {
                    value: "data.items".to_string(),
                    type_hint: None,
                    default: None,
                }),
                variables: None,
                parallelism: Some(20),
                sequential: Some(true),
                dont_stop_on_failed: Some(true),
                max_retries: None,
                retry_delay: None,
                timeout: None,
                allow_null: None,
                convert_single_value: None,
                batch_size: None,
            }),
            subgraph: Box::new(create_minimal_graph("finish")),
            input_schema: HashMap::new(),
            output_schema: HashMap::new(),
            breakpoint: None,
            durable: None,
        };

        let tokens = emit(&split_step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Verify parallelism and sequential are passed to durable function
        assert!(
            code.contains("parallelism : usize"),
            "Should have parallelism parameter in durable function"
        );
        assert!(
            code.contains("sequential : bool"),
            "Should have sequential parameter in durable function"
        );
    }

    #[test]
    fn test_emit_split_zero_parallelism_is_sequential() {
        let mut ctx = EmitContext::new(false);
        let split_step = SplitStep {
            id: "split-zero".to_string(),
            name: Some("Zero Parallelism Split".to_string()),
            config: Some(SplitConfig {
                value: MappingValue::Reference(ReferenceValue {
                    value: "data.items".to_string(),
                    type_hint: None,
                    default: None,
                }),
                variables: None,
                parallelism: Some(0), // Should be treated as sequential
                sequential: None,
                dont_stop_on_failed: None,
                max_retries: None,
                retry_delay: None,
                timeout: None,
                allow_null: None,
                convert_single_value: None,
                batch_size: None,
            }),
            subgraph: Box::new(create_minimal_graph("finish")),
            input_schema: HashMap::new(),
            output_schema: HashMap::new(),
            breakpoint: None,
            durable: None,
        };

        let tokens = emit(&split_step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Verify the conditional check treats 0 and 1 as sequential
        assert!(
            code.contains("if parallelism <= 1"),
            "Should treat parallelism 0 and 1 as sequential execution"
        );
    }

    // ==========================================================================
    // Cache Key Collision Prevention Tests
    // ==========================================================================

    #[test]
    fn test_emit_split_uses_workflow_id_when_prefix_empty() {
        let mut ctx = EmitContext::new(false);
        let split_step = SplitStep {
            id: "split-cache-test".to_string(),
            name: Some("Cache Test Split".to_string()),
            config: Some(SplitConfig {
                value: MappingValue::Reference(ReferenceValue {
                    value: "data.items".to_string(),
                    type_hint: None,
                    default: None,
                }),
                variables: None,
                parallelism: None,
                sequential: None,
                dont_stop_on_failed: None,
                max_retries: None,
                retry_delay: None,
                timeout: None,
                allow_null: None,
                convert_single_value: None,
                batch_size: None,
            }),
            subgraph: Box::new(create_minimal_graph("finish")),
            input_schema: HashMap::new(),
            output_schema: HashMap::new(),
            breakpoint: None,
            durable: None,
        };

        let tokens = emit(&split_step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Verify the generated code checks for _workflow_id when prefix is empty
        assert!(
            code.contains("_workflow_id"),
            "Should check for _workflow_id in variables"
        );

        // Verify the fallback logic: use workflow_id when prefix is empty
        assert!(
            code.contains("if prefix . is_empty ()"),
            "Should have condition to check if prefix is empty"
        );

        // Verify it uses workflow_id in cache key format
        assert!(
            code.contains(r#"unwrap_or ("root")"#),
            "Should fallback to 'root' if _workflow_id not found"
        );
    }

    #[test]
    fn test_emit_split_cache_key_includes_loop_indices() {
        let mut ctx = EmitContext::new(false);
        let split_step = SplitStep {
            id: "split-indices".to_string(),
            name: Some("Loop Indices Split".to_string()),
            config: Some(SplitConfig {
                value: MappingValue::Reference(ReferenceValue {
                    value: "data.items".to_string(),
                    type_hint: None,
                    default: None,
                }),
                variables: None,
                parallelism: None,
                sequential: None,
                dont_stop_on_failed: None,
                max_retries: None,
                retry_delay: None,
                timeout: None,
                allow_null: None,
                convert_single_value: None,
                batch_size: None,
            }),
            subgraph: Box::new(create_minimal_graph("finish")),
            input_schema: HashMap::new(),
            output_schema: HashMap::new(),
            breakpoint: None,
            durable: None,
        };

        let tokens = emit(&split_step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Verify loop indices are extracted from variables
        assert!(
            code.contains("_loop_indices"),
            "Should check for _loop_indices in variables"
        );

        // Verify loop indices suffix format
        assert!(
            code.contains("indices_suffix"),
            "Should build indices_suffix from _loop_indices"
        );
    }

    #[test]
    fn test_emit_split_cache_key_uses_prefix_when_available() {
        let mut ctx = EmitContext::new(false);
        let split_step = SplitStep {
            id: "split-prefix".to_string(),
            name: Some("Prefix Split".to_string()),
            config: Some(SplitConfig {
                value: MappingValue::Reference(ReferenceValue {
                    value: "data.items".to_string(),
                    type_hint: None,
                    default: None,
                }),
                variables: None,
                parallelism: None,
                sequential: None,
                dont_stop_on_failed: None,
                max_retries: None,
                retry_delay: None,
                timeout: None,
                allow_null: None,
                convert_single_value: None,
                batch_size: None,
            }),
            subgraph: Box::new(create_minimal_graph("finish")),
            input_schema: HashMap::new(),
            output_schema: HashMap::new(),
            breakpoint: None,
            durable: None,
        };

        let tokens = emit(&split_step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Verify the code extracts _cache_key_prefix
        assert!(
            code.contains("_cache_key_prefix"),
            "Should check for _cache_key_prefix in variables"
        );

        // Verify both prefix and workflow_id paths exist in the format! calls
        // The else branch uses prefix when it's not empty
        assert!(
            code.contains(r#"format ! ("{}::{}{}""#),
            "Should use format with prefix/workflow_id, base, and indices_suffix"
        );
    }

    // ==========================================================================
    // Allow Null and Convert Single Value Config Tests
    // ==========================================================================

    #[test]
    fn test_emit_split_allow_null_default_false() {
        let mut ctx = EmitContext::new(false);
        let split_step = create_split_step("split-null-default", "data.items");

        let tokens = emit(&split_step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Verify allowNull is set to false by default
        assert!(
            code.contains(r#""allowNull""#),
            "Should include allowNull config"
        );
        // Default should be false - error path should exist
        assert!(
            code.contains("allow_null"),
            "Should have allow_null variable"
        );
    }

    #[test]
    fn test_emit_split_allow_null_enabled() {
        let mut ctx = EmitContext::new(false);
        let split_step = SplitStep {
            id: "split-allow-null".to_string(),
            name: Some("Allow Null Split".to_string()),
            config: Some(SplitConfig {
                value: MappingValue::Reference(ReferenceValue {
                    value: "data.items".to_string(),
                    type_hint: None,
                    default: None,
                }),
                variables: None,
                parallelism: None,
                sequential: None,
                dont_stop_on_failed: None,
                max_retries: None,
                retry_delay: None,
                timeout: None,
                allow_null: Some(true),
                convert_single_value: None,
                batch_size: None,
            }),
            subgraph: Box::new(create_minimal_graph("finish")),
            input_schema: HashMap::new(),
            output_schema: HashMap::new(),
            breakpoint: None,
            durable: None,
        };

        let tokens = emit(&split_step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Verify allowNull is set to true
        assert!(
            code.contains(r#""allowNull" . to_string () , serde_json :: json ! (true)"#),
            "Should set allowNull to true in generated code"
        );
    }

    #[test]
    fn test_emit_split_convert_single_value_default_false() {
        let mut ctx = EmitContext::new(false);
        let split_step = create_split_step("split-convert-default", "data.items");

        let tokens = emit(&split_step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Verify convertSingleValue is included
        assert!(
            code.contains(r#""convertSingleValue""#),
            "Should include convertSingleValue config"
        );
        assert!(
            code.contains("convert_single_value"),
            "Should have convert_single_value variable"
        );
    }

    #[test]
    fn test_emit_split_convert_single_value_enabled() {
        let mut ctx = EmitContext::new(false);
        let split_step = SplitStep {
            id: "split-convert".to_string(),
            name: Some("Convert Single Value Split".to_string()),
            config: Some(SplitConfig {
                value: MappingValue::Reference(ReferenceValue {
                    value: "data.singleItem".to_string(),
                    type_hint: None,
                    default: None,
                }),
                variables: None,
                parallelism: None,
                sequential: None,
                dont_stop_on_failed: None,
                max_retries: None,
                retry_delay: None,
                timeout: None,
                allow_null: None,
                convert_single_value: Some(true),
                batch_size: None,
            }),
            subgraph: Box::new(create_minimal_graph("finish")),
            input_schema: HashMap::new(),
            output_schema: HashMap::new(),
            breakpoint: None,
            durable: None,
        };

        let tokens = emit(&split_step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Verify convertSingleValue is set to true
        assert!(
            code.contains(r#""convertSingleValue" . to_string () , serde_json :: json ! (true)"#),
            "Should set convertSingleValue to true in generated code"
        );
    }

    #[test]
    fn test_emit_split_both_allow_null_and_convert_enabled() {
        let mut ctx = EmitContext::new(false);
        let split_step = SplitStep {
            id: "split-both".to_string(),
            name: Some("Both Options Split".to_string()),
            config: Some(SplitConfig {
                value: MappingValue::Reference(ReferenceValue {
                    value: "data.maybeArray".to_string(),
                    type_hint: None,
                    default: None,
                }),
                variables: None,
                parallelism: None,
                sequential: None,
                dont_stop_on_failed: None,
                max_retries: None,
                retry_delay: None,
                timeout: None,
                allow_null: Some(true),
                convert_single_value: Some(true),
                batch_size: None,
            }),
            subgraph: Box::new(create_minimal_graph("finish")),
            input_schema: HashMap::new(),
            output_schema: HashMap::new(),
            breakpoint: None,
            durable: None,
        };

        let tokens = emit(&split_step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Verify both options are set to true
        assert!(
            code.contains(r#""allowNull" . to_string () , serde_json :: json ! (true)"#),
            "Should set allowNull to true"
        );
        assert!(
            code.contains(r#""convertSingleValue" . to_string () , serde_json :: json ! (true)"#),
            "Should set convertSingleValue to true"
        );
    }

    #[test]
    fn test_emit_split_batch_size_default_zero() {
        let mut ctx = EmitContext::new(false);
        let split_step = create_split_step("split-batch-default", "data.items");

        let tokens = emit(&split_step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // batchSize defaults to 0 (no batching)
        assert!(
            code.contains(r#""batchSize""#),
            "Should include batchSize key in inputs"
        );
        assert!(
            code.contains("batch_size > 0"),
            "Should have batching guarded by batch_size > 0"
        );
        assert!(
            code.contains("chunks (batch_size)"),
            "Should chunk split_array when batching is active"
        );
    }

    #[test]
    fn test_emit_split_batch_size_set() {
        let mut ctx = EmitContext::new(false);
        let split_step = SplitStep {
            id: "split-batch-2".to_string(),
            name: Some("Batched Split".to_string()),
            config: Some(SplitConfig {
                value: MappingValue::Reference(ReferenceValue {
                    value: "data.items".to_string(),
                    type_hint: None,
                    default: None,
                }),
                variables: None,
                parallelism: None,
                sequential: None,
                dont_stop_on_failed: None,
                max_retries: None,
                retry_delay: None,
                timeout: None,
                allow_null: None,
                convert_single_value: None,
                batch_size: Some(2),
            }),
            subgraph: Box::new(create_minimal_graph("finish")),
            input_schema: HashMap::new(),
            output_schema: HashMap::new(),
            breakpoint: None,
            durable: None,
        };

        let tokens = emit(&split_step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // batchSize immediate value plumbed through
        assert!(
            code.contains(r#""batchSize" . to_string () , serde_json :: json ! (2u32)"#),
            "Should emit batchSize = 2 immediate value"
        );
    }

    #[test]
    fn test_emit_split_null_handling_code_structure() {
        let mut ctx = EmitContext::new(false);
        let split_step = create_split_step("split-null-check", "data.items");

        let tokens = emit(&split_step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Verify the null handling code is generated
        assert!(code.contains("Null"), "Should have null handling code");
        // Verify error message references ensure-array
        assert!(
            code.contains("ensure-array"),
            "Error message should reference ensure-array agent"
        );
    }

    #[test]
    fn test_emit_split_single_value_handling_code_structure() {
        let mut ctx = EmitContext::new(false);
        let split_step = create_split_step("split-single-check", "data.items");

        let tokens = emit(&split_step, &mut ctx).unwrap();
        let code = tokens.to_string();

        // Verify single value handling mentions type name
        assert!(
            code.contains("object") || code.contains("string") || code.contains("number"),
            "Should detect value type in error handling"
        );
    }

    // ========================================================================
    // Schema validation codegen tests — Split now emits per-iteration
    // input/output schema checks when the corresponding schema is non-empty.
    // ========================================================================

    fn schema_with_required(field: &str, ty: SchemaFieldType) -> HashMap<String, SchemaField> {
        let mut s = HashMap::new();
        s.insert(
            field.to_string(),
            SchemaField {
                field_type: ty,
                description: None,
                required: true,
                default: None,
                example: None,
                items: None,
                enum_values: None,
                label: None,
                placeholder: None,
                order: None,
                format: None,
                min: None,
                max: None,
                pattern: None,
                properties: None,
                visible_when: None,
            },
        );
        s
    }

    fn split_with_schemas(
        id: &str,
        input_schema: HashMap<String, SchemaField>,
        output_schema: HashMap<String, SchemaField>,
    ) -> SplitStep {
        SplitStep {
            id: id.to_string(),
            name: Some("With Schemas".to_string()),
            config: Some(SplitConfig {
                value: MappingValue::Reference(ReferenceValue {
                    value: "data.items".to_string(),
                    type_hint: None,
                    default: None,
                }),
                variables: None,
                parallelism: None,
                sequential: None,
                dont_stop_on_failed: None,
                max_retries: None,
                retry_delay: None,
                timeout: None,
                allow_null: None,
                convert_single_value: None,
                batch_size: None,
            }),
            subgraph: Box::new(create_minimal_graph("finish")),
            input_schema,
            output_schema,
            breakpoint: None,
            durable: None,
        }
    }

    #[test]
    fn test_emit_split_no_schemas_skips_validator() {
        // Sanity: when neither schema is set, the validator is not emitted —
        // we don't want to pay code-size for nothing.
        let mut ctx = EmitContext::new(false);
        let split_step = create_split_step("split-no-schema", "data.items");
        let code = emit(&split_step, &mut ctx).unwrap().to_string();
        assert!(
            !code.contains("__validate_required_fields"),
            "validator must not be emitted when both schemas are empty"
        );
        assert!(
            !code.contains("__split_input_schema"),
            "no input schema literal when input_schema is empty"
        );
        assert!(
            !code.contains("__split_output_schema"),
            "no output schema literal when output_schema is empty"
        );
    }

    #[test]
    fn test_emit_split_input_schema_emits_check_and_literal() {
        let mut ctx = EmitContext::new(false);
        let split_step = split_with_schemas(
            "split-with-input",
            schema_with_required("sku", SchemaFieldType::String),
            HashMap::new(),
        );
        let code = emit(&split_step, &mut ctx).unwrap().to_string();

        // The schema is serialized as a JSON literal at codegen time and
        // parsed once inside the durable function. The token stream prints
        // string literals with escaped quotes, so look for the field name
        // as a bare substring rather than matching exact quoting.
        assert!(
            code.contains("__split_input_schema"),
            "input schema literal should be present"
        );
        assert!(
            code.contains("sku"),
            "the required field name should appear in the embedded schema literal"
        );
        // The validator helper is emitted once per Split that needs it.
        assert!(
            code.contains("__validate_required_fields"),
            "validator helper must be defined when a schema is set"
        );
        // Sequential path uses it before the subgraph call.
        assert!(
            code.contains("Split '{}' iteration {}: input")
                || code.contains("\"Split '{}' iteration {}: input\""),
            "input check must produce a clearly-labeled error context"
        );
    }

    #[test]
    fn test_emit_split_output_schema_emits_check_and_literal() {
        let mut ctx = EmitContext::new(false);
        let split_step = split_with_schemas(
            "split-with-output",
            HashMap::new(),
            schema_with_required("row", SchemaFieldType::Object),
        );
        let code = emit(&split_step, &mut ctx).unwrap().to_string();

        assert!(
            code.contains("__split_output_schema"),
            "output schema literal should be present"
        );
        assert!(
            code.contains("row"),
            "the required output field name should appear in the embedded schema literal"
        );
        assert!(
            code.contains("__validate_required_fields"),
            "validator helper must be defined when a schema is set"
        );
        // The output check must route into either results.push() (success) or
        // the dont_stop_on_failed branch (failure) — both are wired by the
        // emitter into the `Ok(__result_value)` arm.
        assert!(
            code.contains("__result_value"),
            "output check must run on the subgraph's result value"
        );
        assert!(
            code.contains("Split '{}' iteration {}: output")
                || code.contains("\"Split '{}' iteration {}: output\""),
            "output check must produce a clearly-labeled error context"
        );
    }

    #[test]
    fn test_emit_split_both_schemas_share_validator() {
        let mut ctx = EmitContext::new(false);
        let split_step = split_with_schemas(
            "split-both",
            schema_with_required("sku", SchemaFieldType::String),
            schema_with_required("row", SchemaFieldType::Object),
        );
        let code = emit(&split_step, &mut ctx).unwrap().to_string();
        assert!(code.contains("__split_input_schema"));
        assert!(code.contains("__split_output_schema"));
        // The validator is defined exactly once even when both schemas are
        // set — split keeps a single shared closure.
        assert_eq!(
            code.matches("let __validate_required_fields").count(),
            1,
            "validator should be defined exactly once",
        );
    }
}
