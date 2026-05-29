// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Runtara Workflow Standard Library
//!
//! Unified library for workflow binaries. Combines agents and runtime
//! into a single crate that workflows link against.
//!
//! This library integrates with runtara-core via runtara-sdk for:
//! - Instance registration and lifecycle management
//! - Checkpointing for crash recovery
//! - Signal handling (pause, cancel, resume)
//! - Heartbeat/tick for liveness monitoring
//!
//! Usage in generated workflow code:
//! ```rust
//! extern crate runtara_workflow_stdlib;
//! use runtara_workflow_stdlib::prelude::*;
//! ```
//!
//! Agent implementations are no longer linked into workflow binaries.
//! Workflows dispatch to each agent through its own per-agent WIT
//! interface (`runtara:agent-<id>/capabilities@0.3.0`), bound at
//! `wac compose` time. The stdlib is now ~thin runtime: condition
//! evaluators, SDK protocol wrapper, template rendering, validators.

#[cfg(all(target_arch = "wasm32", feature = "direct-component"))]
#[allow(warnings)]
mod bindings;

// Runtime module (wraps runtara-sdk)
#[cfg(feature = "sdk-runtime")]
pub mod runtime;

// Condition helpers for generated conditional steps
pub mod conditions;

// Switch step helpers for generated switch steps
pub mod switch_helpers;

// Connection envelope types for generated workflow code.
pub mod connections;

// Instance output handling (for Environment communication)
pub mod instance_output;

// Deep-resolve nested {valueType:"reference"} envelopes inside capability
// inputs (e.g. references buried inside a `condition: ConditionExpression`).
pub mod value_resolver;

// Re-export serde at top level
pub use serde;
pub use serde_json;

// Note: tokio and futures are no longer re-exported — generated workflows are synchronous.

// Re-export runtara-sdk for direct use
#[cfg(feature = "sdk-runtime")]
pub use runtara_sdk;

// Re-export runtara-ai as `ai` for AI Agent step codegen.
// Generated workflow code references `runtara_workflow_stdlib::ai::completion`,
// `::message`, `::types`, `::provider`, and `OneOrMany`. Keep this until the
// AI Agent codegen is migrated to dispatch through the `ai-tools` WIT agent.
#[cfg(feature = "sdk-runtime")]
pub use runtara_ai as ai;

// Template rendering for MappingValue::Template
pub mod template;

// JSON helpers for direct-emitted workflow components
pub mod direct_json;

// Child workflow input validation (runtime)
pub mod child_input_validation;

// Agent capability input validation (runtime)
pub mod agent_input_validation;

// Prelude for convenient imports
pub mod prelude {
    // Runtime types
    #[cfg(feature = "sdk-runtime")]
    pub use crate::runtime::{Error, Result};

    // SDK types for durability
    #[cfg(feature = "native")]
    pub use crate::runtime::HttpSdkConfig;
    #[cfg(feature = "sdk-runtime")]
    pub use crate::runtime::{RuntaraSdk, register_sdk, resilient, sdk};

    // Condition helpers for generated conditional steps
    pub use crate::conditions::{is_truthy, to_number, values_equal};

    // Switch step output processing for generated switch steps
    pub use crate::switch_helpers::process_switch_output;

    // Connection envelope types (codegen builds these as stubs; credentials
    // are injected server-side via the runtara-http proxy, not in-workflow).
    pub use crate::connections::{ConnectionResponse, RateLimitState};

    // Note: instance_output removed from prelude - SDK events are now the single source
    // of truth for instance state. Test harness can import directly if needed.

    // Serde types
    pub use serde::{Deserialize, Serialize};
    pub use serde_json;

    // Child input validation for EmbedWorkflow steps
    pub use crate::child_input_validation::{
        ChildInputSchema, ChildInputValidationError, RequiredField, validate_child_inputs,
    };

    // Agent input validation for Agent steps
    pub use crate::agent_input_validation::{
        AgentInputValidationError, RequiredAgentInput, validate_agent_inputs,
    };
}

// Direct access to commonly used modules
#[cfg(feature = "sdk-runtime")]
pub use runtime::{Error, Result};

// Re-export child input validation for generated code
pub use child_input_validation::{
    ChildInputSchema, ChildInputValidationError, RequiredField, validate_child_inputs,
};

// Re-export agent input validation for generated code
pub use agent_input_validation::{
    AgentInputValidationError, RequiredAgentInput, validate_agent_inputs,
};

#[cfg(all(target_arch = "wasm32", feature = "direct-component"))]
mod component {
    use std::cell::RefCell;

    use super::bindings::exports::runtara::workflow_stdlib::json::{AgentRetryError, Guest};
    use super::direct_json::{self, DirectJsonManifest};

    struct Component;

    thread_local! {
        static MANIFEST: RefCell<Option<DirectJsonManifest>> = const { RefCell::new(None) };
    }

    impl Guest for Component {
        fn init_manifest(manifest: Vec<u8>) -> Result<(), String> {
            let manifest = DirectJsonManifest::parse(&manifest)?;
            MANIFEST.with(|slot| {
                *slot.borrow_mut() = Some(manifest);
            });
            Ok(())
        }

        fn build_source(
            data: Vec<u8>,
            variables: Vec<u8>,
            steps: Vec<u8>,
        ) -> Result<Vec<u8>, String> {
            direct_json::build_source(&data, &variables, &steps)
        }

        fn apply_mapping(mapping_id: u32, source: Vec<u8>) -> Result<Vec<u8>, String> {
            MANIFEST.with(|slot| {
                let slot = slot.borrow();
                let manifest = slot
                    .as_ref()
                    .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                manifest.apply_mapping(mapping_id, &source)
            })
        }

        fn eval_condition(condition_id: u32, source: Vec<u8>) -> Result<bool, String> {
            MANIFEST.with(|slot| {
                let slot = slot.borrow();
                let manifest = slot
                    .as_ref()
                    .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                manifest.eval_condition(condition_id, &source)
            })
        }

        fn process_switch(switch_id: u32, source: Vec<u8>) -> Result<String, String> {
            MANIFEST.with(|slot| {
                let slot = slot.borrow();
                let manifest = slot
                    .as_ref()
                    .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                manifest.process_switch(switch_id, &source)
            })
        }

        fn value_switch(switch_id: u32, source: Vec<u8>) -> Result<Vec<u8>, String> {
            MANIFEST.with(|slot| {
                let slot = slot.borrow();
                let manifest = slot
                    .as_ref()
                    .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                manifest.value_switch(switch_id, &source)
            })
        }

        fn split_items(split_id: u32, source: Vec<u8>) -> Result<Vec<u8>, String> {
            MANIFEST.with(|slot| {
                let slot = slot.borrow();
                let manifest = slot
                    .as_ref()
                    .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                manifest.split_items(split_id, &source)
            })
        }

        fn split_item_count(split_id: u32, source: Vec<u8>) -> Result<u32, String> {
            MANIFEST.with(|slot| {
                let slot = slot.borrow();
                let manifest = slot
                    .as_ref()
                    .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                manifest.split_item_count(split_id, &source)
            })
        }

        fn split_item(split_id: u32, source: Vec<u8>, index: u32) -> Result<Vec<u8>, String> {
            MANIFEST.with(|slot| {
                let slot = slot.borrow();
                let manifest = slot
                    .as_ref()
                    .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                manifest.split_item(split_id, &source, index)
            })
        }

        fn split_iteration_variables(
            split_id: u32,
            source: Vec<u8>,
            item: Vec<u8>,
            index: u32,
        ) -> Result<Vec<u8>, String> {
            MANIFEST.with(|slot| {
                let slot = slot.borrow();
                let manifest = slot
                    .as_ref()
                    .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                manifest.split_iteration_variables(split_id, &source, &item, index)
            })
        }

        fn split_validate_input(split_id: u32, item: Vec<u8>, index: u32) -> Result<(), String> {
            MANIFEST.with(|slot| {
                let slot = slot.borrow();
                let manifest = slot
                    .as_ref()
                    .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                manifest.split_validate_input(split_id, &item, index)
            })
        }

        fn split_validate_output(split_id: u32, output: Vec<u8>, index: u32) -> Result<(), String> {
            MANIFEST.with(|slot| {
                let slot = slot.borrow();
                let manifest = slot
                    .as_ref()
                    .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                manifest.split_validate_output(split_id, &output, index)
            })
        }

        fn split_initial_results(split_id: u32) -> Result<Vec<u8>, String> {
            MANIFEST.with(|slot| {
                let slot = slot.borrow();
                let manifest = slot
                    .as_ref()
                    .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                manifest.split_initial_results(split_id)
            })
        }

        fn split_append_output(
            split_id: u32,
            results: Vec<u8>,
            output: Vec<u8>,
        ) -> Result<Vec<u8>, String> {
            MANIFEST.with(|slot| {
                let slot = slot.borrow();
                let manifest = slot
                    .as_ref()
                    .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                manifest.split_append_output(split_id, &results, &output)
            })
        }

        fn split_append_error(
            split_id: u32,
            results: Vec<u8>,
            error: String,
            index: u32,
        ) -> Result<Vec<u8>, String> {
            MANIFEST.with(|slot| {
                let slot = slot.borrow();
                let manifest = slot
                    .as_ref()
                    .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                manifest.split_append_error(split_id, &results, error, index)
            })
        }

        fn split_output(
            split_id: u32,
            source: Vec<u8>,
            results: Vec<u8>,
        ) -> Result<Vec<u8>, String> {
            MANIFEST.with(|slot| {
                let slot = slot.borrow();
                let manifest = slot
                    .as_ref()
                    .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                manifest.split_output(split_id, &source, &results)
            })
        }

        fn split_cache_key(split_id: u32, source: Vec<u8>) -> Result<Vec<u8>, String> {
            MANIFEST.with(|slot| {
                let slot = slot.borrow();
                let manifest = slot
                    .as_ref()
                    .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                manifest.split_cache_key(split_id, &source)
            })
        }

        fn split_result(
            split_id: u32,
            source: Vec<u8>,
            results: Vec<u8>,
        ) -> Result<Vec<u8>, String> {
            MANIFEST.with(|slot| {
                let slot = slot.borrow();
                let manifest = slot
                    .as_ref()
                    .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                manifest.split_result(split_id, &source, &results)
            })
        }

        fn split_output_from_result(
            split_id: u32,
            source: Vec<u8>,
            step_result: Vec<u8>,
        ) -> Result<Vec<u8>, String> {
            MANIFEST.with(|slot| {
                let slot = slot.borrow();
                let manifest = slot
                    .as_ref()
                    .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                manifest.split_output_from_result(split_id, &source, &step_result)
            })
        }

        fn while_max_iterations(while_id: u32) -> Result<u32, String> {
            MANIFEST.with(|slot| {
                let slot = slot.borrow();
                let manifest = slot
                    .as_ref()
                    .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                manifest.while_max_iterations(while_id)
            })
        }

        fn while_initial_state(while_id: u32) -> Result<Vec<u8>, String> {
            MANIFEST.with(|slot| {
                let slot = slot.borrow();
                let manifest = slot
                    .as_ref()
                    .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                manifest.while_initial_state(while_id)
            })
        }

        fn while_condition_source(
            while_id: u32,
            source: Vec<u8>,
            state: Vec<u8>,
        ) -> Result<Vec<u8>, String> {
            MANIFEST.with(|slot| {
                let slot = slot.borrow();
                let manifest = slot
                    .as_ref()
                    .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                manifest.while_condition_source(while_id, &source, &state)
            })
        }

        fn while_condition(while_id: u32, source: Vec<u8>) -> Result<bool, String> {
            MANIFEST.with(|slot| {
                let slot = slot.borrow();
                let manifest = slot
                    .as_ref()
                    .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                manifest.while_condition(while_id, &source)
            })
        }

        fn while_iteration_variables(
            while_id: u32,
            variables: Vec<u8>,
            state: Vec<u8>,
        ) -> Result<Vec<u8>, String> {
            MANIFEST.with(|slot| {
                let slot = slot.borrow();
                let manifest = slot
                    .as_ref()
                    .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                manifest.while_iteration_variables(while_id, &variables, &state)
            })
        }

        fn while_advance_state(
            while_id: u32,
            state: Vec<u8>,
            output: Vec<u8>,
        ) -> Result<Vec<u8>, String> {
            MANIFEST.with(|slot| {
                let slot = slot.borrow();
                let manifest = slot
                    .as_ref()
                    .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                manifest.while_advance_state(while_id, &state, &output)
            })
        }

        fn while_output(while_id: u32, source: Vec<u8>, state: Vec<u8>) -> Result<Vec<u8>, String> {
            MANIFEST.with(|slot| {
                let slot = slot.borrow();
                let manifest = slot
                    .as_ref()
                    .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                manifest.while_output(while_id, &source, &state)
            })
        }

        fn filter(filter_id: u32, source: Vec<u8>) -> Result<Vec<u8>, String> {
            MANIFEST.with(|slot| {
                let slot = slot.borrow();
                let manifest = slot
                    .as_ref()
                    .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                manifest.filter(filter_id, &source)
            })
        }

        fn log_event(log_id: u32, source: Vec<u8>) -> Result<Vec<u8>, String> {
            MANIFEST.with(|slot| {
                let slot = slot.borrow();
                let manifest = slot
                    .as_ref()
                    .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                manifest.log_event(log_id, &source)
            })
        }

        fn log(log_id: u32, source: Vec<u8>) -> Result<Vec<u8>, String> {
            MANIFEST.with(|slot| {
                let slot = slot.borrow();
                let manifest = slot
                    .as_ref()
                    .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                manifest.log(log_id, &source)
            })
        }

        fn error_event(error_id: u32, source: Vec<u8>) -> Result<Vec<u8>, String> {
            MANIFEST.with(|slot| {
                let slot = slot.borrow();
                let manifest = slot
                    .as_ref()
                    .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                manifest.error_event(error_id, &source)
            })
        }

        fn error(error_id: u32, source: Vec<u8>) -> Result<Vec<u8>, String> {
            MANIFEST.with(|slot| {
                let slot = slot.borrow();
                let manifest = slot
                    .as_ref()
                    .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                manifest.error(error_id, &source)
            })
        }

        fn error_steps(step_id: String, error: Vec<u8>, steps: Vec<u8>) -> Result<Vec<u8>, String> {
            direct_json::error_steps(&step_id, &error, &steps)
        }

        fn group_by(group_id: u32, source: Vec<u8>) -> Result<Vec<u8>, String> {
            MANIFEST.with(|slot| {
                let slot = slot.borrow();
                let manifest = slot
                    .as_ref()
                    .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                manifest.group_by(group_id, &source)
            })
        }

        fn delay_duration_ms(delay_id: u32, source: Vec<u8>) -> Result<u64, String> {
            MANIFEST.with(|slot| {
                let slot = slot.borrow();
                let manifest = slot
                    .as_ref()
                    .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                manifest.delay_duration_ms(delay_id, &source)
            })
        }

        fn delay(delay_id: u32, source: Vec<u8>, duration_ms: u64) -> Result<Vec<u8>, String> {
            MANIFEST.with(|slot| {
                let slot = slot.borrow();
                let manifest = slot
                    .as_ref()
                    .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                manifest.delay(delay_id, &source, duration_ms)
            })
        }

        fn breakpoint_key(step_id: String, source: Vec<u8>) -> Result<String, String> {
            MANIFEST.with(|slot| {
                let slot = slot.borrow();
                let manifest = slot
                    .as_ref()
                    .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                manifest.breakpoint_key(&step_id, &source)
            })
        }

        fn breakpoint_event(step_id: String, source: Vec<u8>) -> Result<Vec<u8>, String> {
            MANIFEST.with(|slot| {
                let slot = slot.borrow();
                let manifest = slot
                    .as_ref()
                    .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                manifest.breakpoint_event(&step_id, &source)
            })
        }

        fn wait_signal_id(
            step_id: String,
            instance_id: String,
            source: Vec<u8>,
        ) -> Result<String, String> {
            MANIFEST.with(|slot| {
                let slot = slot.borrow();
                let manifest = slot
                    .as_ref()
                    .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                manifest.wait_signal_id(&step_id, &instance_id, &source)
            })
        }

        fn wait_timeout_ms(step_id: String, source: Vec<u8>) -> Result<Option<u64>, String> {
            MANIFEST.with(|slot| {
                let slot = slot.borrow();
                let manifest = slot
                    .as_ref()
                    .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                manifest.wait_timeout_ms(&step_id, &source)
            })
        }

        fn wait_timeout_error(
            step_id: String,
            signal_id: String,
            timeout_ms: u64,
        ) -> Result<Vec<u8>, String> {
            MANIFEST.with(|slot| {
                let slot = slot.borrow();
                let manifest = slot
                    .as_ref()
                    .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                manifest.wait_timeout_error(&step_id, &signal_id, timeout_ms)
            })
        }

        fn wait_on_wait_variables(
            step_id: String,
            instance_id: String,
            signal_id: String,
            source: Vec<u8>,
        ) -> Result<Vec<u8>, String> {
            MANIFEST.with(|slot| {
                let slot = slot.borrow();
                let manifest = slot
                    .as_ref()
                    .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                manifest.wait_on_wait_variables(&step_id, &instance_id, &signal_id, &source)
            })
        }

        fn wait_on_wait_error(step_id: String, error: Vec<u8>) -> Result<Vec<u8>, String> {
            MANIFEST.with(|slot| {
                let slot = slot.borrow();
                let manifest = slot
                    .as_ref()
                    .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                manifest.wait_on_wait_error(&step_id, &error)
            })
        }

        fn wait_poll_interval_ms(step_id: String) -> Result<u64, String> {
            MANIFEST.with(|slot| {
                let slot = slot.borrow();
                let manifest = slot
                    .as_ref()
                    .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                manifest.wait_poll_interval_ms(&step_id)
            })
        }

        fn wait_event(
            step_id: String,
            signal_id: String,
            source: Vec<u8>,
        ) -> Result<Vec<u8>, String> {
            MANIFEST.with(|slot| {
                let slot = slot.borrow();
                let manifest = slot
                    .as_ref()
                    .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                manifest.wait_event(&step_id, &signal_id, &source)
            })
        }

        fn wait_debug_start(
            step_id: String,
            signal_id: String,
            timeout_ms: Option<u64>,
            source: Vec<u8>,
        ) -> Result<Vec<u8>, String> {
            MANIFEST.with(|slot| {
                let slot = slot.borrow();
                let manifest = slot
                    .as_ref()
                    .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                manifest.wait_debug_start(&step_id, &signal_id, timeout_ms, &source)
            })
        }

        fn wait_output(
            step_id: String,
            signal_id: String,
            signal_payload: Vec<u8>,
            source: Vec<u8>,
        ) -> Result<Vec<u8>, String> {
            MANIFEST.with(|slot| {
                let slot = slot.borrow();
                let manifest = slot
                    .as_ref()
                    .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                manifest.wait_output(&step_id, &signal_id, &signal_payload, &source)
            })
        }

        fn embed_workflow_cache_key(step_id: String, source: Vec<u8>) -> Result<Vec<u8>, String> {
            MANIFEST.with(|slot| {
                let slot = slot.borrow();
                let manifest = slot
                    .as_ref()
                    .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                manifest.embed_workflow_cache_key(&step_id, &source)
            })
        }

        fn embed_workflow_variables(
            step_id: String,
            source: Vec<u8>,
            child_input: Vec<u8>,
        ) -> Result<Vec<u8>, String> {
            MANIFEST.with(|slot| {
                let slot = slot.borrow();
                let manifest = slot
                    .as_ref()
                    .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                manifest.embed_workflow_variables(&step_id, &source, &child_input)
            })
        }

        fn embed_workflow_result(
            step_id: String,
            source: Vec<u8>,
            child_output: Vec<u8>,
        ) -> Result<Vec<u8>, String> {
            MANIFEST.with(|slot| {
                let slot = slot.borrow();
                let manifest = slot
                    .as_ref()
                    .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                manifest.embed_workflow_result(&step_id, &source, &child_output)
            })
        }

        fn embed_workflow_output_from_result(
            step_id: String,
            source: Vec<u8>,
            step_result: Vec<u8>,
        ) -> Result<Vec<u8>, String> {
            MANIFEST.with(|slot| {
                let slot = slot.borrow();
                let manifest = slot
                    .as_ref()
                    .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                manifest.embed_workflow_output_from_result(&step_id, &source, &step_result)
            })
        }

        fn embed_workflow_error(step_id: String, child_error: Vec<u8>) -> Result<Vec<u8>, String> {
            MANIFEST.with(|slot| {
                let slot = slot.borrow();
                let manifest = slot
                    .as_ref()
                    .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                manifest.embed_workflow_error(&step_id, &child_error)
            })
        }

        fn retry_sleep_key(checkpoint_id: String, attempt_number: u32) -> Result<Vec<u8>, String> {
            Ok(direct_json::DirectJsonManifest::retry_sleep_key(
                &checkpoint_id,
                attempt_number,
            ))
        }

        fn retry_delay_ms(
            attempt_number: u32,
            total_attempts: u32,
            base_delay_ms: u64,
            max_delay_ms: u64,
            retry_after_ms: Option<u64>,
        ) -> Result<u64, String> {
            Ok(direct_json::DirectJsonManifest::retry_delay_ms(
                attempt_number,
                total_attempts,
                base_delay_ms,
                max_delay_ms,
                retry_after_ms,
            ))
        }

        fn workflow_error_retryable(error: Vec<u8>) -> Result<bool, String> {
            Ok(direct_json::DirectJsonManifest::workflow_error_retryable(
                &error,
            ))
        }

        fn workflow_error_rate_limited(error: Vec<u8>) -> Result<bool, String> {
            Ok(direct_json::DirectJsonManifest::workflow_error_rate_limited(&error))
        }

        fn workflow_error_retry_after_ms(error: Vec<u8>) -> Result<Option<u64>, String> {
            Ok(direct_json::DirectJsonManifest::workflow_error_retry_after_ms(&error))
        }

        fn agent_output(
            agent_id: u32,
            source: Vec<u8>,
            output: Vec<u8>,
        ) -> Result<Vec<u8>, String> {
            MANIFEST.with(|slot| {
                let slot = slot.borrow();
                let manifest = slot
                    .as_ref()
                    .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                manifest.agent_output(agent_id, &source, &output)
            })
        }

        fn agent_validate_input(agent_id: u32, input: Vec<u8>) -> Result<Vec<u8>, String> {
            MANIFEST.with(|slot| {
                let slot = slot.borrow();
                let manifest = slot
                    .as_ref()
                    .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                manifest.agent_validate_input(agent_id, &input)
            })
        }

        fn agent_connection_input(agent_id: u32, input: Vec<u8>) -> Result<Vec<u8>, String> {
            MANIFEST.with(|slot| {
                let slot = slot.borrow();
                let manifest = slot
                    .as_ref()
                    .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                manifest.agent_connection_input(agent_id, &input)
            })
        }

        fn agent_cache_key(agent_id: u32, source: Vec<u8>) -> Result<Vec<u8>, String> {
            MANIFEST.with(|slot| {
                let slot = slot.borrow();
                let manifest = slot
                    .as_ref()
                    .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                manifest.agent_cache_key(agent_id, &source)
            })
        }

        fn agent_retry_sleep_key(
            checkpoint_id: String,
            attempt_number: u32,
        ) -> Result<Vec<u8>, String> {
            Ok(direct_json::DirectJsonManifest::agent_retry_sleep_key(
                &checkpoint_id,
                attempt_number,
            ))
        }

        fn agent_retry_delay_ms(
            attempt_number: u32,
            total_attempts: u32,
            base_delay_ms: u64,
            max_delay_ms: u64,
            retry_after_ms: Option<u64>,
        ) -> Result<u64, String> {
            Ok(direct_json::DirectJsonManifest::agent_retry_delay_ms(
                attempt_number,
                total_attempts,
                base_delay_ms,
                max_delay_ms,
                retry_after_ms,
            ))
        }

        fn agent_error_info(
            code: String,
            message: String,
            category: String,
            severity: String,
            retryable: bool,
            retry_after_ms: Option<u64>,
            attributes: Option<String>,
        ) -> Result<Vec<u8>, String> {
            direct_json::DirectJsonManifest::agent_error_info(
                &code,
                &message,
                &category,
                &severity,
                retryable,
                retry_after_ms,
                attributes.as_deref(),
            )
        }

        fn agent_retry_error_info(
            code: String,
            message: String,
            category: String,
            severity: String,
            retryable: bool,
            retry_after_ms: Option<u64>,
            attributes: Option<String>,
        ) -> Result<AgentRetryError, String> {
            let retry = direct_json::DirectJsonManifest::agent_retry_error_info(
                &code,
                &message,
                &category,
                &severity,
                retryable,
                retry_after_ms,
                attributes.as_deref(),
            )?;
            Ok(AgentRetryError {
                payload: retry.payload,
                retryable: retry.retryable,
                rate_limited: retry.rate_limited,
            })
        }

        fn agent_error(
            agent_id: u32,
            code: String,
            message: String,
            category: String,
            severity: String,
            retryable: bool,
            retry_after_ms: Option<u64>,
            attributes: Option<String>,
        ) -> Result<Vec<u8>, String> {
            MANIFEST.with(|slot| {
                let slot = slot.borrow();
                let manifest = slot
                    .as_ref()
                    .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                manifest.agent_error(
                    agent_id,
                    &code,
                    &message,
                    &category,
                    &severity,
                    retryable,
                    retry_after_ms,
                    attributes.as_deref(),
                )
            })
        }

        fn agent_error_from_info(agent_id: u32, error_info: Vec<u8>) -> Result<Vec<u8>, String> {
            MANIFEST.with(|slot| {
                let slot = slot.borrow();
                let manifest = slot
                    .as_ref()
                    .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                manifest.agent_error_from_info(agent_id, &error_info)
            })
        }

        fn agent_debug_error(agent_id: u32, error: Vec<u8>) -> Result<Vec<u8>, String> {
            MANIFEST.with(|slot| {
                let slot = slot.borrow();
                let manifest = slot
                    .as_ref()
                    .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                manifest.agent_debug_error(agent_id, &error)
            })
        }

        fn step_debug_start(step_id: String, source: Vec<u8>) -> Result<Vec<u8>, String> {
            MANIFEST.with(|slot| {
                let slot = slot.borrow();
                let manifest = slot
                    .as_ref()
                    .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                manifest.step_debug_start(&step_id, &source)
            })
        }

        fn step_debug_end(step_id: String, source: Vec<u8>) -> Result<Vec<u8>, String> {
            MANIFEST.with(|slot| {
                let slot = slot.borrow();
                let manifest = slot
                    .as_ref()
                    .ok_or_else(|| "direct stdlib manifest was not initialized".to_string())?;
                manifest.step_debug_end(&step_id, &source)
            })
        }
    }

    super::bindings::export!(Component with_types_in super::bindings);
}
