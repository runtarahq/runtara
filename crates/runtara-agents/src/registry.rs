// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Agent registry using inventory-based dynamic dispatch
//!
//! This module provides capability execution by looking up executors
//! registered via the `#[capability]` macro at compile time.

use serde_json::Value;

/// Execute an agent capability
///
/// # Arguments
/// * `agent_id` - The agent name (e.g., "utils", "transform", "csv")
/// * `capability_id` - The capability name (e.g., "random-double", "extract")
/// * `step_inputs` - The input data as JSON Value
///
/// # Returns
/// Result containing the capability result as JSON Value or an error
pub fn execute_capability(
    agent_id: &str,
    capability_id: &str,
    step_inputs: Value,
) -> Result<Value, String> {
    runtara_dsl::agent_meta::execute_capability(agent_id, capability_id, step_inputs)
}
