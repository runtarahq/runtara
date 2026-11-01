// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Runtara Workflow Standard Library
//!
//! The manifest evaluator behind `runtara:workflow-stdlib/json@1.0.0`. A
//! direct-emitted workflow carries its graph as a JSON manifest and calls
//! into this component for every pure decision it has to make: resolving a
//! reference path, applying an input mapping, rendering a template,
//! evaluating a condition, deriving Split/While iteration state, and
//! validating step inputs.
//!
//! Everything here is pure: JSON in, JSON out, plus an interning value
//! store. Durability (registration, checkpointing, signals, heartbeats)
//! is the workflow API's (`runtara:api`), and
//! agent calls go out over each
//! agent's own WIT interface (`runtara:agent-<id>/capabilities@1.0.0`),
//! bound at `wac compose` time. This crate therefore has no HTTP client,
//! no SDK dependency, and no target-specific backends.
//!
//! Built two ways:
//! - `--features direct-component` for `wasm32-wasip2`, the component that
//!   ships in the bundle and is composed into every workflow.
//! - as a plain rlib (no features) for `runtara-workflows`, whose
//!   authoring-time validator shares [`reference_path`] so it tokenizes
//!   paths exactly the way the runtime does.

#[cfg(all(target_arch = "wasm32", feature = "direct-component"))]
#[allow(warnings)]
mod bindings {
    // Generated at compile time by the wit-bindgen macro (no committed
    // bindings.rs, no cargo-component).
    wit_bindgen::generate!({
        // `json` uses `runtara:agent/types.error-info`, so its package comes
        // first.
        path: ["../runtara-wit/wit/agent", "../runtara-wit/wit/workflow-stdlib"],
        world: "runtara:workflow-stdlib/workflow-stdlib",
        generate_all,
    });
}

// Condition evaluation for Conditional / While steps.
pub mod conditions;

// Case selection and output shaping for Switch steps.
pub mod switch_helpers;

// Re-exported so consumers can name the serde types this crate's public
// signatures use without adding their own serde dependency.
pub use serde;
pub use serde_json;

// Template rendering for MappingValue::Template.
pub mod template;

// The manifest itself: parsing, the value store, and every guest
// function exported over the WIT interface.
pub mod direct_json;

mod context;

// Reference-path tokenization, shared with the authoring-time validator so both
// agree on what a path's segments are.
pub mod reference_path;

// EmbedWorkflow child input validation.
pub mod child_input_validation;

// Agent capability input validation.
pub mod agent_input_validation;

// Re-export input validation at the crate root.
pub use child_input_validation::{
    ChildInputSchema, ChildInputValidationError, RequiredField, validate_child_inputs,
};

pub use agent_input_validation::{
    AgentInputValidationError, RequiredAgentInput, validate_agent_inputs,
};

#[cfg(all(target_arch = "wasm32", feature = "direct-component"))]
mod component {
    use super::bindings::exports::runtara::workflow_stdlib::json::{
        AgentRetryError, ErrorInfo, Guest, InvokeError, WaitInstancesProgress,
    };
    use super::context::{self, MANIFEST};
    use super::direct_json::{self, DirectJsonManifest};

    struct Component;

    fn with_manifest<T>(
        call: impl FnOnce(&DirectJsonManifest) -> Result<T, String>,
    ) -> Result<T, String> {
        MANIFEST.with(|slot| {
            let slot = slot.borrow();
            let manifest = slot
                .as_ref()
                .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
            call(manifest)
        })
    }

    impl Guest for Component {
        fn init_manifest(prior: u64, manifest: Vec<u8>) -> Result<u64, String> {
            let id = context::open(prior);
            context::run(id, || {
                // Start each run with an empty interning arena so a reused component
                // instance never resolves a previous run's handles.
                direct_json::reset_value_store();
                let manifest = DirectJsonManifest::parse(&manifest)?;
                MANIFEST.with(|slot| {
                    *slot.borrow_mut() = Some(manifest);
                });
                Ok(id)
            })
            .unwrap_or_else(|| Err(context::unknown(id)))
        }

        fn value_store_retain(
            ctx: u64,
            parent_source: Vec<u8>,
            survivor: Vec<u8>,
        ) -> Result<(), String> {
            context::run(ctx, || {
                direct_json::value_store_retain(&[parent_source.as_slice(), survivor.as_slice()]);
                Ok(())
            })
            .unwrap_or_else(|| Err(context::unknown(ctx)))
        }

        fn value_store_scope(ctx: u64) -> u64 {
            context::run(ctx, direct_json::value_store_scope).unwrap_or_default()
        }

        fn value_store_retain_scoped(
            ctx: u64,
            parent_source: Vec<u8>,
            survivor: Vec<u8>,
            scope: u64,
        ) -> Result<(), String> {
            context::run(ctx, || {
                direct_json::value_store_retain_scoped(
                    &[parent_source.as_slice(), survivor.as_slice()],
                    scope,
                );
                Ok(())
            })
            .unwrap_or_else(|| Err(context::unknown(ctx)))
        }

        fn build_source(
            ctx: u64,
            data: Vec<u8>,
            variables: Vec<u8>,
            steps: Vec<u8>,
        ) -> Result<Vec<u8>, String> {
            context::run(ctx, || direct_json::build_source(&data, &variables, &steps))
                .unwrap_or_else(|| Err(context::unknown(ctx)))
        }

        fn apply_mapping(ctx: u64, mapping_id: u32, source: Vec<u8>) -> Result<Vec<u8>, String> {
            context::run(ctx, || {
                MANIFEST.with(|slot| {
                    let slot = slot.borrow();
                    let manifest = slot
                        .as_ref()
                        .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                    manifest.apply_mapping(mapping_id, &source)
                })
            })
            .unwrap_or_else(|| Err(context::unknown(ctx)))
        }

        fn eval_condition(ctx: u64, condition_id: u32, source: Vec<u8>) -> Result<bool, String> {
            context::run(ctx, || {
                MANIFEST.with(|slot| {
                    let slot = slot.borrow();
                    let manifest = slot
                        .as_ref()
                        .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                    manifest.eval_condition(condition_id, &source)
                })
            })
            .unwrap_or_else(|| Err(context::unknown(ctx)))
        }

        fn conditional_output(
            ctx: u64,
            condition_id: u32,
            source: Vec<u8>,
            decision: bool,
        ) -> Result<Vec<u8>, String> {
            context::run(ctx, || {
                MANIFEST.with(|slot| {
                    let slot = slot.borrow();
                    let manifest = slot
                        .as_ref()
                        .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                    manifest.conditional_output(condition_id, &source, decision)
                })
            })
            .unwrap_or_else(|| Err(context::unknown(ctx)))
        }

        fn process_switch(ctx: u64, switch_id: u32, source: Vec<u8>) -> Result<String, String> {
            context::run(ctx, || {
                MANIFEST.with(|slot| {
                    let slot = slot.borrow();
                    let manifest = slot
                        .as_ref()
                        .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                    manifest.process_switch(switch_id, &source)
                })
            })
            .unwrap_or_else(|| Err(context::unknown(ctx)))
        }

        fn value_switch(ctx: u64, switch_id: u32, source: Vec<u8>) -> Result<Vec<u8>, String> {
            context::run(ctx, || {
                MANIFEST.with(|slot| {
                    let slot = slot.borrow();
                    let manifest = slot
                        .as_ref()
                        .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                    manifest.value_switch(switch_id, &source)
                })
            })
            .unwrap_or_else(|| Err(context::unknown(ctx)))
        }

        fn split_items(ctx: u64, split_id: u32, source: Vec<u8>) -> Result<Vec<u8>, String> {
            context::run(ctx, || {
                MANIFEST.with(|slot| {
                    let slot = slot.borrow();
                    let manifest = slot
                        .as_ref()
                        .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                    manifest.split_items(split_id, &source)
                })
            })
            .unwrap_or_else(|| Err(context::unknown(ctx)))
        }

        fn split_item_count(ctx: u64, split_id: u32, source: Vec<u8>) -> Result<u32, String> {
            context::run(ctx, || {
                MANIFEST.with(|slot| {
                    let slot = slot.borrow();
                    let manifest = slot
                        .as_ref()
                        .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                    manifest.split_item_count(split_id, &source)
                })
            })
            .unwrap_or_else(|| Err(context::unknown(ctx)))
        }

        fn split_item(
            ctx: u64,
            split_id: u32,
            source: Vec<u8>,
            index: u32,
        ) -> Result<Vec<u8>, String> {
            context::run(ctx, || {
                MANIFEST.with(|slot| {
                    let slot = slot.borrow();
                    let manifest = slot
                        .as_ref()
                        .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                    manifest.split_item(split_id, &source, index)
                })
            })
            .unwrap_or_else(|| Err(context::unknown(ctx)))
        }

        fn split_iteration_variables(
            ctx: u64,
            split_id: u32,
            source: Vec<u8>,
            item: Vec<u8>,
            index: u32,
        ) -> Result<Vec<u8>, String> {
            context::run(ctx, || {
                MANIFEST.with(|slot| {
                    let slot = slot.borrow();
                    let manifest = slot
                        .as_ref()
                        .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                    manifest.split_iteration_variables(split_id, &source, &item, index)
                })
            })
            .unwrap_or_else(|| Err(context::unknown(ctx)))
        }

        fn split_validate_input(
            ctx: u64,
            split_id: u32,
            item: Vec<u8>,
            index: u32,
        ) -> Result<(), String> {
            context::run(ctx, || {
                MANIFEST.with(|slot| {
                    let slot = slot.borrow();
                    let manifest = slot
                        .as_ref()
                        .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                    manifest.split_validate_input(split_id, &item, index)
                })
            })
            .unwrap_or_else(|| Err(context::unknown(ctx)))
        }

        fn split_validate_output(
            ctx: u64,
            split_id: u32,
            output: Vec<u8>,
            index: u32,
        ) -> Result<(), String> {
            context::run(ctx, || {
                MANIFEST.with(|slot| {
                    let slot = slot.borrow();
                    let manifest = slot
                        .as_ref()
                        .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                    manifest.split_validate_output(split_id, &output, index)
                })
            })
            .unwrap_or_else(|| Err(context::unknown(ctx)))
        }

        fn split_initial_results(ctx: u64, split_id: u32) -> Result<Vec<u8>, String> {
            context::run(ctx, || {
                MANIFEST.with(|slot| {
                    let slot = slot.borrow();
                    let manifest = slot
                        .as_ref()
                        .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                    manifest.split_initial_results(split_id)
                })
            })
            .unwrap_or_else(|| Err(context::unknown(ctx)))
        }

        fn split_append_output(
            ctx: u64,
            split_id: u32,
            results: Vec<u8>,
            output: Vec<u8>,
        ) -> Result<Vec<u8>, String> {
            context::run(ctx, || {
                MANIFEST.with(|slot| {
                    let slot = slot.borrow();
                    let manifest = slot
                        .as_ref()
                        .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                    manifest.split_append_output(split_id, &results, &output)
                })
            })
            .unwrap_or_else(|| Err(context::unknown(ctx)))
        }

        fn split_append_error(
            ctx: u64,
            split_id: u32,
            results: Vec<u8>,
            error: String,
            index: u32,
        ) -> Result<Vec<u8>, String> {
            context::run(ctx, || {
                MANIFEST.with(|slot| {
                    let slot = slot.borrow();
                    let manifest = slot
                        .as_ref()
                        .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                    manifest.split_append_error(split_id, &results, error, index)
                })
            })
            .unwrap_or_else(|| Err(context::unknown(ctx)))
        }

        fn split_output(
            ctx: u64,
            split_id: u32,
            source: Vec<u8>,
            results: Vec<u8>,
        ) -> Result<Vec<u8>, String> {
            context::run(ctx, || {
                MANIFEST.with(|slot| {
                    let slot = slot.borrow();
                    let manifest = slot
                        .as_ref()
                        .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                    manifest.split_output(split_id, &source, &results)
                })
            })
            .unwrap_or_else(|| Err(context::unknown(ctx)))
        }

        fn split_cache_key(ctx: u64, split_id: u32, source: Vec<u8>) -> Result<Vec<u8>, String> {
            context::run(ctx, || {
                MANIFEST.with(|slot| {
                    let slot = slot.borrow();
                    let manifest = slot
                        .as_ref()
                        .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                    manifest.split_cache_key(split_id, &source)
                })
            })
            .unwrap_or_else(|| Err(context::unknown(ctx)))
        }

        fn split_result(
            ctx: u64,
            split_id: u32,
            source: Vec<u8>,
            results: Vec<u8>,
        ) -> Result<Vec<u8>, String> {
            context::run(ctx, || {
                MANIFEST.with(|slot| {
                    let slot = slot.borrow();
                    let manifest = slot
                        .as_ref()
                        .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                    manifest.split_result(split_id, &source, &results)
                })
            })
            .unwrap_or_else(|| Err(context::unknown(ctx)))
        }

        fn split_output_from_result(
            ctx: u64,
            split_id: u32,
            source: Vec<u8>,
            step_result: Vec<u8>,
        ) -> Result<Vec<u8>, String> {
            context::run(ctx, || {
                MANIFEST.with(|slot| {
                    let slot = slot.borrow();
                    let manifest = slot
                        .as_ref()
                        .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                    manifest.split_output_from_result(split_id, &source, &step_result)
                })
            })
            .unwrap_or_else(|| Err(context::unknown(ctx)))
        }

        fn while_max_iterations(ctx: u64, while_id: u32) -> Result<u32, String> {
            context::run(ctx, || {
                MANIFEST.with(|slot| {
                    let slot = slot.borrow();
                    let manifest = slot
                        .as_ref()
                        .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                    manifest.while_max_iterations(while_id)
                })
            })
            .unwrap_or_else(|| Err(context::unknown(ctx)))
        }

        fn while_initial_state(ctx: u64, while_id: u32) -> Result<Vec<u8>, String> {
            context::run(ctx, || {
                MANIFEST.with(|slot| {
                    let slot = slot.borrow();
                    let manifest = slot
                        .as_ref()
                        .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                    manifest.while_initial_state(while_id)
                })
            })
            .unwrap_or_else(|| Err(context::unknown(ctx)))
        }

        fn while_condition_source(
            ctx: u64,
            while_id: u32,
            source: Vec<u8>,
            state: Vec<u8>,
        ) -> Result<Vec<u8>, String> {
            context::run(ctx, || {
                MANIFEST.with(|slot| {
                    let slot = slot.borrow();
                    let manifest = slot
                        .as_ref()
                        .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                    manifest.while_condition_source(while_id, &source, &state)
                })
            })
            .unwrap_or_else(|| Err(context::unknown(ctx)))
        }

        fn while_condition(ctx: u64, while_id: u32, source: Vec<u8>) -> Result<bool, String> {
            context::run(ctx, || {
                MANIFEST.with(|slot| {
                    let slot = slot.borrow();
                    let manifest = slot
                        .as_ref()
                        .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                    manifest.while_condition(while_id, &source)
                })
            })
            .unwrap_or_else(|| Err(context::unknown(ctx)))
        }

        fn while_iteration_variables(
            ctx: u64,
            while_id: u32,
            source: Vec<u8>,
            state: Vec<u8>,
        ) -> Result<Vec<u8>, String> {
            context::run(ctx, || {
                MANIFEST.with(|slot| {
                    let slot = slot.borrow();
                    let manifest = slot
                        .as_ref()
                        .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                    manifest.while_iteration_variables(while_id, &source, &state)
                })
            })
            .unwrap_or_else(|| Err(context::unknown(ctx)))
        }

        fn while_advance_state(
            ctx: u64,
            while_id: u32,
            state: Vec<u8>,
            output: Vec<u8>,
        ) -> Result<Vec<u8>, String> {
            context::run(ctx, || {
                MANIFEST.with(|slot| {
                    let slot = slot.borrow();
                    let manifest = slot
                        .as_ref()
                        .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                    manifest.while_advance_state(while_id, &state, &output)
                })
            })
            .unwrap_or_else(|| Err(context::unknown(ctx)))
        }

        fn while_output(
            ctx: u64,
            while_id: u32,
            source: Vec<u8>,
            state: Vec<u8>,
        ) -> Result<Vec<u8>, String> {
            context::run(ctx, || {
                MANIFEST.with(|slot| {
                    let slot = slot.borrow();
                    let manifest = slot
                        .as_ref()
                        .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                    manifest.while_output(while_id, &source, &state)
                })
            })
            .unwrap_or_else(|| Err(context::unknown(ctx)))
        }

        fn while_cache_key(ctx: u64, while_id: u32, source: Vec<u8>) -> Result<Vec<u8>, String> {
            context::run(ctx, || {
                MANIFEST.with(|slot| {
                    let slot = slot.borrow();
                    let manifest = slot
                        .as_ref()
                        .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                    manifest.while_cache_key(while_id, &source)
                })
            })
            .unwrap_or_else(|| Err(context::unknown(ctx)))
        }

        fn while_result(
            ctx: u64,
            while_id: u32,
            source: Vec<u8>,
            state: Vec<u8>,
        ) -> Result<Vec<u8>, String> {
            context::run(ctx, || {
                MANIFEST.with(|slot| {
                    let slot = slot.borrow();
                    let manifest = slot
                        .as_ref()
                        .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                    manifest.while_result(while_id, &source, &state)
                })
            })
            .unwrap_or_else(|| Err(context::unknown(ctx)))
        }

        fn while_output_from_result(
            ctx: u64,
            while_id: u32,
            source: Vec<u8>,
            step_result: Vec<u8>,
        ) -> Result<Vec<u8>, String> {
            context::run(ctx, || {
                MANIFEST.with(|slot| {
                    let slot = slot.borrow();
                    let manifest = slot
                        .as_ref()
                        .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                    manifest.while_output_from_result(while_id, &source, &step_result)
                })
            })
            .unwrap_or_else(|| Err(context::unknown(ctx)))
        }

        fn filter(ctx: u64, filter_id: u32, source: Vec<u8>) -> Result<Vec<u8>, String> {
            context::run(ctx, || {
                MANIFEST.with(|slot| {
                    let slot = slot.borrow();
                    let manifest = slot
                        .as_ref()
                        .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                    manifest.filter(filter_id, &source)
                })
            })
            .unwrap_or_else(|| Err(context::unknown(ctx)))
        }

        fn log_event(ctx: u64, log_id: u32, source: Vec<u8>) -> Result<Vec<u8>, String> {
            context::run(ctx, || {
                MANIFEST.with(|slot| {
                    let slot = slot.borrow();
                    let manifest = slot
                        .as_ref()
                        .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                    manifest.log_event(log_id, &source)
                })
            })
            .unwrap_or_else(|| Err(context::unknown(ctx)))
        }

        fn log(ctx: u64, log_id: u32, source: Vec<u8>) -> Result<Vec<u8>, String> {
            context::run(ctx, || {
                MANIFEST.with(|slot| {
                    let slot = slot.borrow();
                    let manifest = slot
                        .as_ref()
                        .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                    manifest.log(log_id, &source)
                })
            })
            .unwrap_or_else(|| Err(context::unknown(ctx)))
        }

        fn error_event(ctx: u64, error_id: u32, source: Vec<u8>) -> Result<Vec<u8>, String> {
            context::run(ctx, || {
                MANIFEST.with(|slot| {
                    let slot = slot.borrow();
                    let manifest = slot
                        .as_ref()
                        .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                    manifest.error_event(error_id, &source)
                })
            })
            .unwrap_or_else(|| Err(context::unknown(ctx)))
        }

        fn error(ctx: u64, error_id: u32, source: Vec<u8>) -> Result<Vec<u8>, String> {
            context::run(ctx, || {
                MANIFEST.with(|slot| {
                    let slot = slot.borrow();
                    let manifest = slot
                        .as_ref()
                        .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                    manifest.error(error_id, &source)
                })
            })
            .unwrap_or_else(|| Err(context::unknown(ctx)))
        }

        fn error_steps(
            ctx: u64,
            step_id: String,
            error: Vec<u8>,
            steps: Vec<u8>,
        ) -> Result<Vec<u8>, String> {
            context::run(ctx, || direct_json::error_steps(&step_id, &error, &steps))
                .unwrap_or_else(|| Err(context::unknown(ctx)))
        }

        fn group_by(ctx: u64, group_id: u32, source: Vec<u8>) -> Result<Vec<u8>, String> {
            context::run(ctx, || {
                MANIFEST.with(|slot| {
                    let slot = slot.borrow();
                    let manifest = slot
                        .as_ref()
                        .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                    manifest.group_by(group_id, &source)
                })
            })
            .unwrap_or_else(|| Err(context::unknown(ctx)))
        }

        fn delay_duration_ms(ctx: u64, delay_id: u32, source: Vec<u8>) -> Result<u64, String> {
            context::run(ctx, || {
                MANIFEST.with(|slot| {
                    let slot = slot.borrow();
                    let manifest = slot
                        .as_ref()
                        .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                    manifest.delay_duration_ms(delay_id, &source)
                })
            })
            .unwrap_or_else(|| Err(context::unknown(ctx)))
        }

        fn delay(
            ctx: u64,
            delay_id: u32,
            source: Vec<u8>,
            duration_ms: u64,
        ) -> Result<Vec<u8>, String> {
            context::run(ctx, || {
                MANIFEST.with(|slot| {
                    let slot = slot.borrow();
                    let manifest = slot
                        .as_ref()
                        .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                    manifest.delay(delay_id, &source, duration_ms)
                })
            })
            .unwrap_or_else(|| Err(context::unknown(ctx)))
        }

        fn loop_deadline_key(
            ctx: u64,
            step_id: String,
            source: Vec<u8>,
            complete: bool,
        ) -> Result<Vec<u8>, String> {
            context::run(ctx, || {
                MANIFEST.with(|slot| {
                    let slot = slot.borrow();
                    let manifest = slot
                        .as_ref()
                        .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                    manifest
                        .loop_deadline_key(&step_id, &source, complete)
                        .map(String::into_bytes)
                })
            })
            .unwrap_or_else(|| Err(context::unknown(ctx)))
        }

        fn delay_sleep_key(ctx: u64, step_id: String, source: Vec<u8>) -> Result<Vec<u8>, String> {
            context::run(ctx, || {
                MANIFEST.with(|slot| {
                    let slot = slot.borrow();
                    let manifest = slot
                        .as_ref()
                        .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                    manifest
                        .delay_sleep_key(&step_id, &source)
                        .map(String::into_bytes)
                })
            })
            .unwrap_or_else(|| Err(context::unknown(ctx)))
        }

        fn invoke_error_fields(ctx: u64, error: Vec<u8>) -> Result<InvokeError, String> {
            context::run(ctx, || {
                let fields = direct_json::invoke_error_fields(&error);
                Ok(InvokeError {
                    code: fields.code,
                    message: fields.message,
                    category: fields.category,
                    severity: fields.severity,
                    retryable: fields.retryable,
                    retry_after_ms: fields.retry_after_ms,
                    attributes: fields.attributes,
                    details: fields.details,
                })
            })
            .unwrap_or_else(|| Err(context::unknown(ctx)))
        }

        fn breakpoint_key(ctx: u64, step_id: String, source: Vec<u8>) -> Result<String, String> {
            context::run(ctx, || {
                MANIFEST.with(|slot| {
                    let slot = slot.borrow();
                    let manifest = slot
                        .as_ref()
                        .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                    manifest.breakpoint_key(&step_id, &source)
                })
            })
            .unwrap_or_else(|| Err(context::unknown(ctx)))
        }

        fn breakpoint_event(ctx: u64, step_id: String, source: Vec<u8>) -> Result<Vec<u8>, String> {
            context::run(ctx, || {
                MANIFEST.with(|slot| {
                    let slot = slot.borrow();
                    let manifest = slot
                        .as_ref()
                        .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                    manifest.breakpoint_event(&step_id, &source)
                })
            })
            .unwrap_or_else(|| Err(context::unknown(ctx)))
        }

        fn wait_signal_id(
            ctx: u64,
            step_id: String,
            instance_id: String,
            source: Vec<u8>,
        ) -> Result<String, String> {
            context::run(ctx, || {
                MANIFEST.with(|slot| {
                    let slot = slot.borrow();
                    let manifest = slot
                        .as_ref()
                        .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                    manifest.wait_signal_id(&step_id, &instance_id, &source)
                })
            })
            .unwrap_or_else(|| Err(context::unknown(ctx)))
        }

        fn wait_timeout_ms(
            ctx: u64,
            step_id: String,
            source: Vec<u8>,
        ) -> Result<Option<u64>, String> {
            context::run(ctx, || {
                MANIFEST.with(|slot| {
                    let slot = slot.borrow();
                    let manifest = slot
                        .as_ref()
                        .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                    manifest.wait_timeout_ms(&step_id, &source)
                })
            })
            .unwrap_or_else(|| Err(context::unknown(ctx)))
        }

        fn wait_timeout_error(
            ctx: u64,
            step_id: String,
            signal_id: String,
            timeout_ms: u64,
        ) -> Result<Vec<u8>, String> {
            context::run(ctx, || {
                MANIFEST.with(|slot| {
                    let slot = slot.borrow();
                    let manifest = slot
                        .as_ref()
                        .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                    manifest.wait_timeout_error(&step_id, &signal_id, timeout_ms)
                })
            })
            .unwrap_or_else(|| Err(context::unknown(ctx)))
        }

        fn wait_timeout_error_envelope(
            ctx: u64,
            step_id: String,
            signal_id: String,
            timeout_ms: u64,
        ) -> Result<Vec<u8>, String> {
            context::run(ctx, || {
                MANIFEST.with(|slot| {
                    let slot = slot.borrow();
                    let manifest = slot
                        .as_ref()
                        .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                    manifest.wait_timeout_error_envelope(&step_id, &signal_id, timeout_ms)
                })
            })
            .unwrap_or_else(|| Err(context::unknown(ctx)))
        }

        fn wait_closed_error(
            ctx: u64,
            step_id: String,
            signal_id: String,
            reason: String,
            timeout_ms: Option<u64>,
        ) -> Result<Vec<u8>, String> {
            context::run(ctx, || {
                MANIFEST.with(|slot| {
                    let slot = slot.borrow();
                    let manifest = slot
                        .as_ref()
                        .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                    manifest.wait_closed_error(&step_id, &signal_id, &reason, timeout_ms)
                })
            })
            .unwrap_or_else(|| Err(context::unknown(ctx)))
        }

        fn wait_on_wait_variables(
            ctx: u64,
            step_id: String,
            instance_id: String,
            signal_id: String,
            source: Vec<u8>,
        ) -> Result<Vec<u8>, String> {
            context::run(ctx, || {
                MANIFEST.with(|slot| {
                    let slot = slot.borrow();
                    let manifest = slot
                        .as_ref()
                        .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                    manifest.wait_on_wait_variables(&step_id, &instance_id, &signal_id, &source)
                })
            })
            .unwrap_or_else(|| Err(context::unknown(ctx)))
        }

        fn wait_on_wait_error(
            ctx: u64,
            step_id: String,
            error: Vec<u8>,
        ) -> Result<Vec<u8>, String> {
            context::run(ctx, || {
                MANIFEST.with(|slot| {
                    let slot = slot.borrow();
                    let manifest = slot
                        .as_ref()
                        .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                    manifest.wait_on_wait_error(&step_id, &error)
                })
            })
            .unwrap_or_else(|| Err(context::unknown(ctx)))
        }

        fn wait_poll_interval_ms(ctx: u64, step_id: String) -> Result<u64, String> {
            context::run(ctx, || {
                MANIFEST.with(|slot| {
                    let slot = slot.borrow();
                    let manifest = slot
                        .as_ref()
                        .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                    manifest.wait_poll_interval_ms(&step_id)
                })
            })
            .unwrap_or_else(|| Err(context::unknown(ctx)))
        }

        fn wait_poll_interval_ms_scoped(
            ctx: u64,
            step_id: String,
            source: Vec<u8>,
        ) -> Result<u64, String> {
            context::run(ctx, || {
                MANIFEST.with(|slot| {
                    let slot = slot.borrow();
                    let manifest = slot
                        .as_ref()
                        .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                    manifest.wait_poll_interval_ms_scoped(&step_id, &source)
                })
            })
            .unwrap_or_else(|| Err(context::unknown(ctx)))
        }

        fn wait_event(
            ctx: u64,
            step_id: String,
            signal_id: String,
            source: Vec<u8>,
        ) -> Result<Vec<u8>, String> {
            context::run(ctx, || {
                MANIFEST.with(|slot| {
                    let slot = slot.borrow();
                    let manifest = slot
                        .as_ref()
                        .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                    manifest.wait_event(&step_id, &signal_id, &source)
                })
            })
            .unwrap_or_else(|| Err(context::unknown(ctx)))
        }

        fn wait_debug_start(
            ctx: u64,
            step_id: String,
            signal_id: String,
            timeout_ms: Option<u64>,
            source: Vec<u8>,
        ) -> Result<Vec<u8>, String> {
            context::run(ctx, || {
                MANIFEST.with(|slot| {
                    let slot = slot.borrow();
                    let manifest = slot
                        .as_ref()
                        .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                    manifest.wait_debug_start(&step_id, &signal_id, timeout_ms, &source)
                })
            })
            .unwrap_or_else(|| Err(context::unknown(ctx)))
        }

        fn wait_output(
            ctx: u64,
            step_id: String,
            signal_id: String,
            signal_payload: Vec<u8>,
            source: Vec<u8>,
        ) -> Result<Vec<u8>, String> {
            context::run(ctx, || {
                MANIFEST.with(|slot| {
                    let slot = slot.borrow();
                    let manifest = slot
                        .as_ref()
                        .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                    manifest.wait_output(&step_id, &signal_id, &signal_payload, &source)
                })
            })
            .unwrap_or_else(|| Err(context::unknown(ctx)))
        }

        fn wait_instances_key(
            ctx: u64,
            step_id: String,
            source: Vec<u8>,
        ) -> Result<String, String> {
            context::run(ctx, || {
                MANIFEST.with(|slot| {
                    let slot = slot.borrow();
                    let manifest = slot
                        .as_ref()
                        .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                    manifest.wait_instances_key(&step_id, &source)
                })
            })
            .unwrap_or_else(|| Err(context::unknown(ctx)))
        }

        fn wait_instances_request(
            ctx: u64,
            step_id: String,
            source: Vec<u8>,
            now_ms: u64,
        ) -> Result<Vec<u8>, String> {
            context::run(ctx, || {
                MANIFEST.with(|slot| {
                    let slot = slot.borrow();
                    let manifest = slot
                        .as_ref()
                        .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                    manifest.wait_instances_request(&step_id, &source, now_ms)
                })
            })
            .unwrap_or_else(|| Err(context::unknown(ctx)))
        }

        fn wait_instances_state(ctx: u64, wait: Vec<u8>) -> Result<WaitInstancesProgress, String> {
            context::run(ctx, || {
                MANIFEST.with(|slot| {
                    let slot = slot.borrow();
                    let manifest = slot
                        .as_ref()
                        .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                    let (pending, deadline_ms) = manifest.wait_instances_state(&wait)?;
                    Ok(WaitInstancesProgress {
                        pending,
                        deadline_ms,
                    })
                })
            })
            .unwrap_or_else(|| Err(context::unknown(ctx)))
        }

        fn wait_instances_output(
            ctx: u64,
            step_id: String,
            wait: Vec<u8>,
            source: Vec<u8>,
        ) -> Result<Vec<u8>, String> {
            context::run(ctx, || {
                MANIFEST.with(|slot| {
                    let slot = slot.borrow();
                    let manifest = slot
                        .as_ref()
                        .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                    manifest.wait_instances_output(&step_id, &wait, &source)
                })
            })
            .unwrap_or_else(|| Err(context::unknown(ctx)))
        }

        fn wait_instances_error(
            ctx: u64,
            step_id: String,
            error: Vec<u8>,
        ) -> Result<Vec<u8>, String> {
            context::run(ctx, || {
                MANIFEST.with(|slot| {
                    let slot = slot.borrow();
                    let manifest = slot
                        .as_ref()
                        .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                    manifest.wait_instances_error(&step_id, &error)
                })
            })
            .unwrap_or_else(|| Err(context::unknown(ctx)))
        }

        fn state_key(
            ctx: u64,
            step_id: String,
            source: Vec<u8>,
            local: bool,
        ) -> Result<String, String> {
            context::run(ctx, || {
                with_manifest(|manifest| manifest.state_key(&step_id, &source, local))
            })
            .unwrap_or_else(|| Err(context::unknown(ctx)))
        }

        fn state_patch(ctx: u64, step_id: String, source: Vec<u8>) -> Result<Vec<u8>, String> {
            context::run(ctx, || {
                with_manifest(|manifest| manifest.state_patch(&step_id, &source))
            })
            .unwrap_or_else(|| Err(context::unknown(ctx)))
        }

        fn state_local_set(ctx: u64, step_id: String, source: Vec<u8>) -> Result<Vec<u8>, String> {
            context::run(ctx, || {
                with_manifest(|manifest| manifest.state_local_set(&step_id, &source))
            })
            .unwrap_or_else(|| Err(context::unknown(ctx)))
        }

        fn state_local_get(ctx: u64, step_id: String, source: Vec<u8>) -> Result<Vec<u8>, String> {
            context::run(ctx, || {
                with_manifest(|manifest| manifest.state_local_get(&step_id, &source))
            })
            .unwrap_or_else(|| Err(context::unknown(ctx)))
        }

        fn state_local_restore(
            ctx: u64,
            step_id: String,
            value: Vec<u8>,
            source: Vec<u8>,
        ) -> Result<Vec<u8>, String> {
            context::run(ctx, || {
                with_manifest(|manifest| manifest.state_local_restore(&step_id, &value, &source))
            })
            .unwrap_or_else(|| Err(context::unknown(ctx)))
        }

        fn state_output(
            ctx: u64,
            step_id: String,
            value: Vec<u8>,
            source: Vec<u8>,
        ) -> Result<Vec<u8>, String> {
            context::run(ctx, || {
                with_manifest(|manifest| manifest.state_output(&step_id, &value, &source))
            })
            .unwrap_or_else(|| Err(context::unknown(ctx)))
        }

        fn state_error(ctx: u64, step_id: String, error: Vec<u8>) -> Result<Vec<u8>, String> {
            context::run(ctx, || {
                with_manifest(|manifest| manifest.state_error(&step_id, &error))
            })
            .unwrap_or_else(|| Err(context::unknown(ctx)))
        }

        fn api_error_json(
            ctx: u64,
            code: String,
            message: String,
            retry_after_ms: Option<u64>,
        ) -> Vec<u8> {
            context::run(ctx, || {
                crate::direct_json::api_error_json(&code, &message, retry_after_ms)
            })
            .unwrap_or_default()
        }

        fn agent_operation_input(
            ctx: u64,
            input: Vec<u8>,
            operation_id: Option<String>,
            continuation: Option<Vec<u8>>,
        ) -> Result<Vec<u8>, String> {
            context::run(ctx, || {
                crate::direct_json::agent_operation_input(
                    &input,
                    operation_id.as_deref(),
                    continuation.as_deref(),
                )
            })
            .unwrap_or_else(|| Err(context::unknown(ctx)))
        }

        fn ai_wait_tool_signal_id(
            ctx: u64,
            step_id: String,
            instance_id: String,
            label: String,
            call_counter: u32,
            source: Vec<u8>,
        ) -> Result<String, String> {
            context::run(ctx, || {
                MANIFEST.with(|slot| {
                    let slot = slot.borrow();
                    let manifest = slot
                        .as_ref()
                        .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                    manifest.ai_wait_tool_signal_id(
                        &step_id,
                        &instance_id,
                        &label,
                        call_counter,
                        &source,
                    )
                })
            })
            .unwrap_or_else(|| Err(context::unknown(ctx)))
        }

        fn ai_wait_tool_result(ctx: u64, signal_payload: Vec<u8>) -> Result<Vec<u8>, String> {
            context::run(ctx, || {
                MANIFEST.with(|slot| {
                    let slot = slot.borrow();
                    let manifest = slot
                        .as_ref()
                        .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                    manifest.ai_wait_tool_result(&signal_payload)
                })
            })
            .unwrap_or_else(|| Err(context::unknown(ctx)))
        }

        fn embed_workflow_cache_key(
            ctx: u64,
            step_id: String,
            source: Vec<u8>,
        ) -> Result<Vec<u8>, String> {
            context::run(ctx, || {
                MANIFEST.with(|slot| {
                    let slot = slot.borrow();
                    let manifest = slot
                        .as_ref()
                        .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                    manifest.embed_workflow_cache_key(&step_id, &source)
                })
            })
            .unwrap_or_else(|| Err(context::unknown(ctx)))
        }

        fn embed_workflow_variables(
            ctx: u64,
            step_id: String,
            source: Vec<u8>,
            child_input: Vec<u8>,
        ) -> Result<Vec<u8>, String> {
            context::run(ctx, || {
                MANIFEST.with(|slot| {
                    let slot = slot.borrow();
                    let manifest = slot
                        .as_ref()
                        .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                    manifest.embed_workflow_variables(&step_id, &source, &child_input)
                })
            })
            .unwrap_or_else(|| Err(context::unknown(ctx)))
        }

        fn embed_workflow_result(
            ctx: u64,
            step_id: String,
            source: Vec<u8>,
            child_output: Vec<u8>,
        ) -> Result<Vec<u8>, String> {
            context::run(ctx, || {
                MANIFEST.with(|slot| {
                    let slot = slot.borrow();
                    let manifest = slot
                        .as_ref()
                        .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                    manifest.embed_workflow_result(&step_id, &source, &child_output)
                })
            })
            .unwrap_or_else(|| Err(context::unknown(ctx)))
        }

        fn embed_workflow_output_from_result(
            ctx: u64,
            step_id: String,
            source: Vec<u8>,
            step_result: Vec<u8>,
        ) -> Result<Vec<u8>, String> {
            context::run(ctx, || {
                MANIFEST.with(|slot| {
                    let slot = slot.borrow();
                    let manifest = slot
                        .as_ref()
                        .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                    manifest.embed_workflow_output_from_result(&step_id, &source, &step_result)
                })
            })
            .unwrap_or_else(|| Err(context::unknown(ctx)))
        }

        fn embed_workflow_error(
            ctx: u64,
            step_id: String,
            child_error: Vec<u8>,
        ) -> Result<Vec<u8>, String> {
            context::run(ctx, || {
                MANIFEST.with(|slot| {
                    let slot = slot.borrow();
                    let manifest = slot
                        .as_ref()
                        .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                    manifest.embed_workflow_error(&step_id, &child_error)
                })
            })
            .unwrap_or_else(|| Err(context::unknown(ctx)))
        }

        fn embed_workflow_error_scoped(
            ctx: u64,
            step_id: String,
            source: Vec<u8>,
            child_error: Vec<u8>,
        ) -> Result<Vec<u8>, String> {
            context::run(ctx, || {
                MANIFEST.with(|slot| {
                    let slot = slot.borrow();
                    let manifest = slot
                        .as_ref()
                        .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                    manifest.embed_workflow_error_scoped(&step_id, &source, &child_error)
                })
            })
            .unwrap_or_else(|| Err(context::unknown(ctx)))
        }

        fn retry_sleep_key(
            ctx: u64,
            checkpoint_id: String,
            attempt_number: u32,
        ) -> Result<Vec<u8>, String> {
            context::run(ctx, || {
                Ok(direct_json::DirectJsonManifest::retry_sleep_key(
                    &checkpoint_id,
                    attempt_number,
                ))
            })
            .unwrap_or_else(|| Err(context::unknown(ctx)))
        }

        fn agent_attempt_result_key(
            ctx: u64,
            checkpoint_id: String,
            attempt_number: u32,
        ) -> Result<Vec<u8>, String> {
            context::run(ctx, || {
                Ok(direct_json::DirectJsonManifest::agent_attempt_result_key(
                    &checkpoint_id,
                    attempt_number,
                ))
            })
            .unwrap_or_else(|| Err(context::unknown(ctx)))
        }

        fn agent_attempt_envelope(
            ctx: u64,
            tag: u8,
            retryable: bool,
            rate_limited: bool,
            retry_after_tag: bool,
            retry_after_ms: u64,
            payload: Vec<u8>,
        ) -> Result<Vec<u8>, String> {
            context::run(ctx, || {
                Ok(direct_json::DirectJsonManifest::agent_attempt_envelope(
                    tag,
                    retryable,
                    rate_limited,
                    retry_after_tag,
                    retry_after_ms,
                    &payload,
                ))
            })
            .unwrap_or_else(|| Err(context::unknown(ctx)))
        }

        fn retry_delay_ms(
            ctx: u64,
            attempt_number: u32,
            total_attempts: u32,
            base_delay_ms: u64,
            max_delay_ms: u64,
            retry_after_ms: Option<u64>,
        ) -> Result<u64, String> {
            context::run(ctx, || {
                Ok(direct_json::DirectJsonManifest::retry_delay_ms(
                    attempt_number,
                    total_attempts,
                    base_delay_ms,
                    max_delay_ms,
                    retry_after_ms,
                ))
            })
            .unwrap_or_else(|| Err(context::unknown(ctx)))
        }

        fn workflow_error_retryable(ctx: u64, error: Vec<u8>) -> Result<bool, String> {
            context::run(ctx, || {
                Ok(direct_json::DirectJsonManifest::workflow_error_retryable(
                    &error,
                ))
            })
            .unwrap_or_else(|| Err(context::unknown(ctx)))
        }

        fn workflow_error_rate_limited(ctx: u64, error: Vec<u8>) -> Result<bool, String> {
            context::run(ctx, || {
                Ok(direct_json::DirectJsonManifest::workflow_error_rate_limited(&error))
            })
            .unwrap_or_else(|| Err(context::unknown(ctx)))
        }

        fn workflow_error_retry_after_ms(ctx: u64, error: Vec<u8>) -> Result<Option<u64>, String> {
            context::run(ctx, || {
                Ok(direct_json::DirectJsonManifest::workflow_error_retry_after_ms(&error))
            })
            .unwrap_or_else(|| Err(context::unknown(ctx)))
        }

        fn agent_output(
            ctx: u64,
            agent_id: u32,
            source: Vec<u8>,
            output: Vec<u8>,
        ) -> Result<Vec<u8>, String> {
            context::run(ctx, || {
                MANIFEST.with(|slot| {
                    let slot = slot.borrow();
                    let manifest = slot
                        .as_ref()
                        .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                    manifest.agent_output(agent_id, &source, &output)
                })
            })
            .unwrap_or_else(|| Err(context::unknown(ctx)))
        }

        fn ai_agent_output(
            ctx: u64,
            agent_id: u32,
            source: Vec<u8>,
            output: Vec<u8>,
        ) -> Result<Vec<u8>, String> {
            context::run(ctx, || {
                MANIFEST.with(|slot| {
                    let slot = slot.borrow();
                    let manifest = slot
                        .as_ref()
                        .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                    manifest.ai_agent_output(agent_id, &source, &output)
                })
            })
            .unwrap_or_else(|| Err(context::unknown(ctx)))
        }

        fn ai_turn_next_input(
            ctx: u64,
            base: Vec<u8>,
            turn_out: Vec<u8>,
            pending: Vec<u8>,
        ) -> Result<Vec<u8>, String> {
            context::run(ctx, || {
                direct_json::DirectJsonManifest::ai_turn_next_input(&base, &turn_out, &pending)
            })
            .unwrap_or_else(|| Err(context::unknown(ctx)))
        }

        fn ai_turn_is_complete(ctx: u64, turn_out: Vec<u8>) -> Result<bool, String> {
            context::run(ctx, || {
                direct_json::DirectJsonManifest::ai_turn_is_complete(&turn_out)
            })
            .unwrap_or_else(|| Err(context::unknown(ctx)))
        }

        fn ai_turn_tool_count(ctx: u64, turn_out: Vec<u8>) -> Result<u32, String> {
            context::run(ctx, || {
                direct_json::DirectJsonManifest::ai_turn_tool_count(&turn_out)
            })
            .unwrap_or_else(|| Err(context::unknown(ctx)))
        }

        fn ai_turn_tool_args(ctx: u64, turn_out: Vec<u8>, index: u32) -> Result<Vec<u8>, String> {
            context::run(ctx, || {
                direct_json::DirectJsonManifest::ai_turn_tool_args(&turn_out, index)
            })
            .unwrap_or_else(|| Err(context::unknown(ctx)))
        }

        fn ai_turn_tool_index(ctx: u64, turn_out: Vec<u8>, index: u32) -> Result<u32, String> {
            context::run(ctx, || {
                direct_json::DirectJsonManifest::ai_turn_tool_index(&turn_out, index)
            })
            .unwrap_or_else(|| Err(context::unknown(ctx)))
        }

        fn ai_tool_args_with_timeout(
            ctx: u64,
            args: Vec<u8>,
            timeout_ms: u64,
        ) -> Result<Vec<u8>, String> {
            context::run(ctx, || {
                direct_json::DirectJsonManifest::ai_tool_args_with_timeout(&args, timeout_ms)
            })
            .unwrap_or_else(|| Err(context::unknown(ctx)))
        }

        fn ai_turn_add_result(
            ctx: u64,
            pending: Vec<u8>,
            turn_out: Vec<u8>,
            index: u32,
            tool_result: Vec<u8>,
        ) -> Result<Vec<u8>, String> {
            context::run(ctx, || {
                direct_json::DirectJsonManifest::ai_turn_add_result(
                    &pending,
                    &turn_out,
                    index,
                    &tool_result,
                )
            })
            .unwrap_or_else(|| Err(context::unknown(ctx)))
        }

        fn ai_turn_cache_key(
            ctx: u64,
            step_id: String,
            iteration: u32,
            source: Vec<u8>,
        ) -> Result<String, String> {
            context::run(ctx, || {
                direct_json::DirectJsonManifest::ai_turn_cache_key(&step_id, iteration, &source)
            })
            .unwrap_or_else(|| Err(context::unknown(ctx)))
        }

        fn ai_turn_response_key(
            ctx: u64,
            step_id: String,
            iteration: u32,
            source: Vec<u8>,
        ) -> Result<String, String> {
            context::run(ctx, || {
                direct_json::DirectJsonManifest::ai_turn_response_key(&step_id, iteration, &source)
            })
            .unwrap_or_else(|| Err(context::unknown(ctx)))
        }

        fn ai_turn_response_validate(ctx: u64, response: Vec<u8>) -> Result<(), String> {
            context::run(ctx, || {
                direct_json::DirectJsonManifest::ai_turn_response_validate(&response)
            })
            .unwrap_or_else(|| Err(context::unknown(ctx)))
        }

        fn ai_turn_snapshot(
            ctx: u64,
            state: Vec<u8>,
            pending: Vec<u8>,
            tool_calls: u32,
            complete: bool,
        ) -> Result<Vec<u8>, String> {
            context::run(ctx, || {
                direct_json::DirectJsonManifest::ai_turn_snapshot(
                    &state, &pending, tool_calls, complete,
                )
            })
            .unwrap_or_else(|| Err(context::unknown(ctx)))
        }

        fn ai_turn_snapshot_part(
            ctx: u64,
            snapshot: Vec<u8>,
            part: u32,
        ) -> Result<Vec<u8>, String> {
            context::run(ctx, || {
                direct_json::DirectJsonManifest::ai_turn_snapshot_part(&snapshot, part)
            })
            .unwrap_or_else(|| Err(context::unknown(ctx)))
        }

        fn ai_turn_snapshot_tool_calls(ctx: u64, snapshot: Vec<u8>) -> Result<u32, String> {
            context::run(ctx, || {
                direct_json::DirectJsonManifest::ai_turn_snapshot_tool_calls(&snapshot)
            })
            .unwrap_or_else(|| Err(context::unknown(ctx)))
        }

        fn ai_turn_snapshot_complete(ctx: u64, snapshot: Vec<u8>) -> Result<bool, String> {
            context::run(ctx, || {
                direct_json::DirectJsonManifest::ai_turn_snapshot_complete(&snapshot)
            })
            .unwrap_or_else(|| Err(context::unknown(ctx)))
        }

        fn ai_turn_slot_key(
            ctx: u64,
            step_id: String,
            iteration: u32,
            source: Vec<u8>,
        ) -> Result<String, String> {
            context::run(ctx, || {
                direct_json::DirectJsonManifest::ai_turn_slot_key(&step_id, iteration, &source)
            })
            .unwrap_or_else(|| Err(context::unknown(ctx)))
        }

        fn ai_turn_slot_snapshot(
            ctx: u64,
            state: Vec<u8>,
            pending: Vec<u8>,
            tool_calls: u32,
            turn: u32,
        ) -> Result<Vec<u8>, String> {
            context::run(ctx, || {
                direct_json::DirectJsonManifest::ai_turn_slot_snapshot(
                    &state, &pending, tool_calls, turn,
                )
            })
            .unwrap_or_else(|| Err(context::unknown(ctx)))
        }

        fn ai_turn_snapshot_turn(ctx: u64, snapshot: Vec<u8>) -> Result<u32, String> {
            context::run(ctx, || {
                direct_json::DirectJsonManifest::ai_turn_snapshot_turn(&snapshot)
            })
            .unwrap_or_else(|| Err(context::unknown(ctx)))
        }

        fn ai_turn_latest(ctx: u64, first: Vec<u8>, second: Vec<u8>) -> Result<Vec<u8>, String> {
            context::run(ctx, || {
                direct_json::DirectJsonManifest::ai_turn_latest(&first, &second)
            })
            .unwrap_or_else(|| Err(context::unknown(ctx)))
        }

        fn ai_agent_result_key(
            ctx: u64,
            step_id: String,
            source: Vec<u8>,
        ) -> Result<String, String> {
            context::run(ctx, || {
                direct_json::DirectJsonManifest::ai_agent_result_key(&step_id, &source)
            })
            .unwrap_or_else(|| Err(context::unknown(ctx)))
        }

        fn ai_agent_result(ctx: u64, state: Vec<u8>) -> Result<Vec<u8>, String> {
            context::run(ctx, || {
                direct_json::DirectJsonManifest::ai_agent_result(&state)
            })
            .unwrap_or_else(|| Err(context::unknown(ctx)))
        }

        fn ai_summary_key(ctx: u64, step_id: String, source: Vec<u8>) -> Result<String, String> {
            context::run(ctx, || {
                direct_json::DirectJsonManifest::ai_summary_key(&step_id, &source)
            })
            .unwrap_or_else(|| Err(context::unknown(ctx)))
        }

        fn ai_turn_output(
            ctx: u64,
            agent_id: u32,
            source: Vec<u8>,
            turn_out: Vec<u8>,
        ) -> Result<Vec<u8>, String> {
            context::run(ctx, || {
                MANIFEST.with(|slot| {
                    let slot = slot.borrow();
                    let manifest = slot
                        .as_ref()
                        .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                    manifest.ai_turn_output(agent_id, &source, &turn_out)
                })
            })
            .unwrap_or_else(|| Err(context::unknown(ctx)))
        }

        fn ai_tool_debug_start(
            ctx: u64,
            agent_id: u32,
            turn_out: Vec<u8>,
            index: u32,
            iteration: u32,
            call_counter: u32,
            source: Vec<u8>,
        ) -> Result<Vec<u8>, String> {
            context::run(ctx, || {
                MANIFEST.with(|slot| {
                    let slot = slot.borrow();
                    let manifest = slot
                        .as_ref()
                        .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                    manifest.ai_tool_debug_start(
                        agent_id,
                        &turn_out,
                        index,
                        iteration,
                        call_counter,
                        &source,
                    )
                })
            })
            .unwrap_or_else(|| Err(context::unknown(ctx)))
        }

        fn ai_tool_debug_end(
            ctx: u64,
            agent_id: u32,
            turn_out: Vec<u8>,
            index: u32,
            iteration: u32,
            call_counter: u32,
            tool_result: Vec<u8>,
            source: Vec<u8>,
        ) -> Result<Vec<u8>, String> {
            context::run(ctx, || {
                MANIFEST.with(|slot| {
                    let slot = slot.borrow();
                    let manifest = slot
                        .as_ref()
                        .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                    manifest.ai_tool_debug_end(
                        agent_id,
                        &turn_out,
                        index,
                        iteration,
                        call_counter,
                        &tool_result,
                        &source,
                    )
                })
            })
            .unwrap_or_else(|| Err(context::unknown(ctx)))
        }

        fn ai_memory_debug_start(
            ctx: u64,
            agent_id: u32,
            phase: u32,
            conversation: Vec<u8>,
            state: Vec<u8>,
            max_messages: u32,
            source: Vec<u8>,
        ) -> Result<Vec<u8>, String> {
            context::run(ctx, || {
                MANIFEST.with(|slot| {
                    let slot = slot.borrow();
                    let manifest = slot
                        .as_ref()
                        .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                    manifest.ai_memory_debug_start(
                        agent_id,
                        phase,
                        &conversation,
                        &state,
                        max_messages,
                        &source,
                    )
                })
            })
            .unwrap_or_else(|| Err(context::unknown(ctx)))
        }

        fn ai_memory_debug_end(
            ctx: u64,
            agent_id: u32,
            phase: u32,
            conversation: Vec<u8>,
            state: Vec<u8>,
            prior_state: Vec<u8>,
            max_messages: u32,
            source: Vec<u8>,
        ) -> Result<Vec<u8>, String> {
            context::run(ctx, || {
                MANIFEST.with(|slot| {
                    let slot = slot.borrow();
                    let manifest = slot
                        .as_ref()
                        .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                    manifest.ai_memory_debug_end(
                        agent_id,
                        phase,
                        &conversation,
                        &state,
                        &prior_state,
                        max_messages,
                        &source,
                    )
                })
            })
            .unwrap_or_else(|| Err(context::unknown(ctx)))
        }

        fn ai_memory_initial_state(ctx: u64, load_output: Vec<u8>) -> Result<Vec<u8>, String> {
            context::run(ctx, || {
                direct_json::DirectJsonManifest::ai_memory_initial_state(&load_output)
            })
            .unwrap_or_else(|| Err(context::unknown(ctx)))
        }

        fn ai_memory_save_input(
            ctx: u64,
            conversation: Vec<u8>,
            final_state: Vec<u8>,
        ) -> Result<Vec<u8>, String> {
            context::run(ctx, || {
                direct_json::DirectJsonManifest::ai_memory_save_input(&conversation, &final_state)
            })
            .unwrap_or_else(|| Err(context::unknown(ctx)))
        }

        fn ai_memory_compact_sliding(
            ctx: u64,
            state: Vec<u8>,
            max_messages: u32,
        ) -> Result<Vec<u8>, String> {
            context::run(ctx, || {
                direct_json::DirectJsonManifest::ai_memory_compact_sliding(&state, max_messages)
            })
            .unwrap_or_else(|| Err(context::unknown(ctx)))
        }

        fn ai_summarize_input(
            ctx: u64,
            base: Vec<u8>,
            state: Vec<u8>,
            max_messages: u32,
        ) -> Result<Vec<u8>, String> {
            context::run(ctx, || {
                direct_json::DirectJsonManifest::ai_summarize_input(&base, &state, max_messages)
            })
            .unwrap_or_else(|| Err(context::unknown(ctx)))
        }

        fn ai_summarize_output(ctx: u64, result: Vec<u8>) -> Result<Vec<u8>, String> {
            context::run(ctx, || {
                direct_json::DirectJsonManifest::ai_summarize_output(&result)
            })
            .unwrap_or_else(|| Err(context::unknown(ctx)))
        }

        fn agent_validate_input(
            ctx: u64,
            agent_id: u32,
            input: Vec<u8>,
        ) -> Result<Vec<u8>, String> {
            context::run(ctx, || {
                MANIFEST.with(|slot| {
                    let slot = slot.borrow();
                    let manifest = slot
                        .as_ref()
                        .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                    manifest.agent_validate_input(agent_id, &input)
                })
            })
            .unwrap_or_else(|| Err(context::unknown(ctx)))
        }

        fn agent_connection_input(
            ctx: u64,
            agent_id: u32,
            input: Vec<u8>,
            descriptor: Vec<u8>,
        ) -> Result<Vec<u8>, String> {
            context::run(ctx, || {
                MANIFEST.with(|slot| {
                    let slot = slot.borrow();
                    let manifest = slot
                        .as_ref()
                        .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                    manifest.agent_connection_input(agent_id, &input, &descriptor)
                })
            })
            .unwrap_or_else(|| Err(context::unknown(ctx)))
        }

        fn agent_connection_id(
            ctx: u64,
            agent_id: u32,
            source: Vec<u8>,
        ) -> Result<Vec<u8>, String> {
            context::run(ctx, || {
                MANIFEST.with(|slot| {
                    let slot = slot.borrow();
                    let manifest = slot
                        .as_ref()
                        .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                    manifest.resolve_connection_id(agent_id, &source)
                })
            })
            .unwrap_or_else(|| Err(context::unknown(ctx)))
        }

        fn agent_scope_input(
            ctx: u64,
            agent_id: u32,
            input: Vec<u8>,
            source: Vec<u8>,
        ) -> Result<Vec<u8>, String> {
            context::run(ctx, || {
                MANIFEST.with(|slot| {
                    let slot = slot.borrow();
                    let manifest = slot
                        .as_ref()
                        .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                    manifest.agent_scope_input(agent_id, &input, &source)
                })
            })
            .unwrap_or_else(|| Err(context::unknown(ctx)))
        }

        fn agent_tool_scope_input(
            ctx: u64,
            ai_step_id: String,
            label: String,
            call_counter: u32,
            input: Vec<u8>,
            source: Vec<u8>,
        ) -> Result<Vec<u8>, String> {
            context::run(ctx, || {
                direct_json::DirectJsonManifest::agent_tool_scope_input(
                    &ai_step_id,
                    &label,
                    call_counter,
                    &input,
                    &source,
                )
            })
            .unwrap_or_else(|| Err(context::unknown(ctx)))
        }

        fn tool_scope_source(
            ctx: u64,
            ai_step_id: String,
            label: String,
            call_counter: u32,
            source: Vec<u8>,
        ) -> Result<Vec<u8>, String> {
            context::run(ctx, || {
                direct_json::DirectJsonManifest::tool_scope_source(
                    &ai_step_id,
                    &label,
                    call_counter,
                    &source,
                )
            })
            .unwrap_or_else(|| Err(context::unknown(ctx)))
        }

        fn agent_aux_scope_source(
            ctx: u64,
            agent_id: u32,
            source: Vec<u8>,
        ) -> Result<Vec<u8>, String> {
            context::run(ctx, || {
                MANIFEST.with(|slot| {
                    let slot = slot.borrow();
                    let manifest = slot
                        .as_ref()
                        .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                    manifest.agent_aux_scope_source(agent_id, &source)
                })
            })
            .unwrap_or_else(|| Err(context::unknown(ctx)))
        }

        fn agent_cache_key(ctx: u64, agent_id: u32, source: Vec<u8>) -> Result<Vec<u8>, String> {
            context::run(ctx, || {
                MANIFEST.with(|slot| {
                    let slot = slot.borrow();
                    let manifest = slot
                        .as_ref()
                        .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                    manifest.agent_cache_key(agent_id, &source)
                })
            })
            .unwrap_or_else(|| Err(context::unknown(ctx)))
        }

        fn agent_retry_sleep_key(
            ctx: u64,
            checkpoint_id: String,
            attempt_number: u32,
        ) -> Result<Vec<u8>, String> {
            context::run(ctx, || {
                Ok(direct_json::DirectJsonManifest::agent_retry_sleep_key(
                    &checkpoint_id,
                    attempt_number,
                ))
            })
            .unwrap_or_else(|| Err(context::unknown(ctx)))
        }

        fn agent_retry_delay_ms(
            ctx: u64,
            attempt_number: u32,
            total_attempts: u32,
            base_delay_ms: u64,
            max_delay_ms: u64,
            retry_after_ms: Option<u64>,
        ) -> Result<u64, String> {
            context::run(ctx, || {
                Ok(direct_json::DirectJsonManifest::agent_retry_delay_ms(
                    attempt_number,
                    total_attempts,
                    base_delay_ms,
                    max_delay_ms,
                    retry_after_ms,
                ))
            })
            .unwrap_or_else(|| Err(context::unknown(ctx)))
        }

        fn agent_retry_error_info(error: ErrorInfo) -> Result<AgentRetryError, String> {
            {
                let retry = direct_json::DirectJsonManifest::agent_retry_error_info(
                    &error.code,
                    &error.message,
                    &error.category,
                    &error.severity,
                    error.retryable,
                    error.retry_after_ms,
                    error.attributes.as_deref(),
                    error.details.as_deref(),
                )?;
                Ok(AgentRetryError {
                    payload: retry.payload,
                    retryable: retry.retryable,
                    rate_limited: retry.rate_limited,
                })
            }
        }

        fn agent_error(agent_id: u32, error: ErrorInfo, ctx: u64) -> Result<Vec<u8>, String> {
            context::run(ctx, || {
                MANIFEST.with(|slot| {
                    let slot = slot.borrow();
                    let manifest = slot
                        .as_ref()
                        .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                    manifest.agent_error(
                        agent_id,
                        &error.code,
                        &error.message,
                        &error.category,
                        &error.severity,
                        error.retryable,
                        error.retry_after_ms,
                        error.attributes.as_deref(),
                        error.details.as_deref(),
                    )
                })
            })
            .unwrap_or_else(|| Err(context::unknown(ctx)))
        }

        fn agent_error_from_info(
            ctx: u64,
            agent_id: u32,
            error_info: Vec<u8>,
        ) -> Result<Vec<u8>, String> {
            context::run(ctx, || {
                MANIFEST.with(|slot| {
                    let slot = slot.borrow();
                    let manifest = slot
                        .as_ref()
                        .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                    manifest.agent_error_from_info(agent_id, &error_info)
                })
            })
            .unwrap_or_else(|| Err(context::unknown(ctx)))
        }

        fn agent_debug_error(
            ctx: u64,
            agent_id: u32,
            source: Vec<u8>,
            error: Vec<u8>,
        ) -> Result<Vec<u8>, String> {
            context::run(ctx, || {
                MANIFEST.with(|slot| {
                    let slot = slot.borrow();
                    let manifest = slot
                        .as_ref()
                        .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                    manifest.agent_debug_error(agent_id, &source, &error)
                })
            })
            .unwrap_or_else(|| Err(context::unknown(ctx)))
        }

        fn step_debug_start(ctx: u64, step_id: String, source: Vec<u8>) -> Result<Vec<u8>, String> {
            context::run(ctx, || {
                MANIFEST.with(|slot| {
                    let slot = slot.borrow();
                    let manifest = slot
                        .as_ref()
                        .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                    manifest.step_debug_start(&step_id, &source)
                })
            })
            .unwrap_or_else(|| Err(context::unknown(ctx)))
        }

        fn step_debug_end(
            ctx: u64,
            step_id: String,
            source: Vec<u8>,
            launched_at_ms: u64,
            settled_at_ms: u64,
        ) -> Result<Vec<u8>, String> {
            context::run(ctx, || {
                MANIFEST.with(|slot| {
                    let slot = slot.borrow();
                    let manifest = slot
                        .as_ref()
                        .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                    manifest.step_debug_end(&step_id, &source, launched_at_ms, settled_at_ms)
                })
            })
            .unwrap_or_else(|| Err(context::unknown(ctx)))
        }

        fn step_debug_error(
            ctx: u64,
            step_id: String,
            source: Vec<u8>,
            error: Vec<u8>,
        ) -> Result<Vec<u8>, String> {
            context::run(ctx, || {
                MANIFEST.with(|slot| {
                    let slot = slot.borrow();
                    let manifest = slot
                        .as_ref()
                        .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                    manifest.step_debug_error(&step_id, &source, &error)
                })
            })
            .unwrap_or_else(|| Err(context::unknown(ctx)))
        }
    }

    super::bindings::export!(Component with_types_in super::bindings);
}
