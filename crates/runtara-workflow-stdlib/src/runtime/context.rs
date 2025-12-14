// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Runtime context for workflow execution

use serde_json::Value;
use std::collections::HashMap;

/// Runtime context providing workflow execution state
#[derive(Debug, Default)]
pub struct RuntimeContext {
    /// Step execution results
    pub steps_context: HashMap<String, Value>,
    /// Workflow input
    pub input: Value,
    /// Connection data
    pub connections: HashMap<String, Value>,
}

impl RuntimeContext {
    /// Create a new runtime context
    pub fn new() -> Self {
        Self::default()
    }

    /// Create runtime context with input
    pub fn with_input(input: Value) -> Self {
        Self {
            input,
            ..Default::default()
        }
    }

    /// Get a step result by step ID
    pub fn get_step_result(&self, step_id: &str) -> Option<&Value> {
        self.steps_context.get(step_id)
    }

    /// Set a step result
    pub fn set_step_result(&mut self, step_id: String, value: Value) {
        self.steps_context.insert(step_id, value);
    }

    /// Get the workflow input
    pub fn get_input(&self) -> &Value {
        &self.input
    }
}
