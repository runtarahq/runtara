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

    use super::bindings::exports::runtara::workflow_stdlib::json::Guest;
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
