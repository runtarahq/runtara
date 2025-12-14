// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Auto-generated unified operator registry for generated workflows
//!
//! This file is generated at build time by build.rs
//! DO NOT EDIT MANUALLY
//!
//! Note: This module references sibling operator modules (utils, transform, csv)
//! which must be declared in the parent module (main.rs)

use serde_json::Value;

/// Execute an operator operation
///
/// # Arguments
/// * `operator_id` - The operator name (e.g., "Utils", "Transform", "CSV")
/// * `operation_id` - The operation name (e.g., "random-double", "extract")
/// * `step_inputs` - The input data as JSON Value
///
/// # Returns
/// Result containing the operation result as JSON Value or an error
pub fn execute_operation(
    operator_id: &str,
    operation_id: &str,
    step_inputs: Value,
) -> Result<Value, String> {
    // Normalize operator_id to lowercase for case-insensitive matching
    let operator_normalized = operator_id.to_lowercase();
    let operator = operator_normalized.as_str();

    match operator {
        _ => Err(format!("Unknown operator: {}", operator_id)),
    }
}


