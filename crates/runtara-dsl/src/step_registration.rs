// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Step type metadata registration
//!
//! This module registers all step types with inventory for automatic
//! DSL schema generation. Each step type's metadata is derived from
//! the struct definition using schemars.
//!
//! This approach keeps schema_types.rs clean while still achieving
//! the goal of single-source-of-truth: the step struct IS the schema.

use crate::agent_meta::StepTypeMeta;
use crate::{
    AgentStep, ConditionalStep, ConnectionStep, ErrorStep, FinishStep, LogStep, SplitStep,
    StartScenarioStep, SwitchStep, WhileStep,
};

// ============================================================================
// Schema Generator Functions
// ============================================================================

fn schema_finish_step() -> schemars::schema::RootSchema {
    schemars::schema_for!(FinishStep)
}

fn schema_agent_step() -> schemars::schema::RootSchema {
    schemars::schema_for!(AgentStep)
}

fn schema_conditional_step() -> schemars::schema::RootSchema {
    schemars::schema_for!(ConditionalStep)
}

fn schema_split_step() -> schemars::schema::RootSchema {
    schemars::schema_for!(SplitStep)
}

fn schema_switch_step() -> schemars::schema::RootSchema {
    schemars::schema_for!(SwitchStep)
}

fn schema_start_scenario_step() -> schemars::schema::RootSchema {
    schemars::schema_for!(StartScenarioStep)
}

fn schema_while_step() -> schemars::schema::RootSchema {
    schemars::schema_for!(WhileStep)
}

fn schema_log_step() -> schemars::schema::RootSchema {
    schemars::schema_for!(LogStep)
}

fn schema_connection_step() -> schemars::schema::RootSchema {
    schemars::schema_for!(ConnectionStep)
}

fn schema_error_step() -> schemars::schema::RootSchema {
    schemars::schema_for!(ErrorStep)
}

// ============================================================================
// Step Type Metadata Registrations
// ============================================================================

static FINISH_STEP_META: StepTypeMeta = StepTypeMeta {
    id: "Finish",
    display_name: "Finish",
    description: "Exit point - defines scenario outputs",
    category: "control",
    schema_fn: schema_finish_step,
};

static AGENT_STEP_META: StepTypeMeta = StepTypeMeta {
    id: "Agent",
    display_name: "Agent",
    description: "Executes an operator operation",
    category: "execution",
    schema_fn: schema_agent_step,
};

static CONDITIONAL_STEP_META: StepTypeMeta = StepTypeMeta {
    id: "Conditional",
    display_name: "Conditional",
    description: "Evaluates conditions and branches execution",
    category: "control",
    schema_fn: schema_conditional_step,
};

static SPLIT_STEP_META: StepTypeMeta = StepTypeMeta {
    id: "Split",
    display_name: "Split",
    description: "Iterates over an array, executing subgraph for each item",
    category: "control",
    schema_fn: schema_split_step,
};

static SWITCH_STEP_META: StepTypeMeta = StepTypeMeta {
    id: "Switch",
    display_name: "Switch",
    description: "Multi-way branch based on value matching",
    category: "control",
    schema_fn: schema_switch_step,
};

static START_SCENARIO_STEP_META: StepTypeMeta = StepTypeMeta {
    id: "StartScenario",
    display_name: "Start Scenario",
    description: "Executes a nested child scenario",
    category: "execution",
    schema_fn: schema_start_scenario_step,
};

static WHILE_STEP_META: StepTypeMeta = StepTypeMeta {
    id: "While",
    display_name: "While Loop",
    description: "Repeats execution while condition is true",
    category: "control",
    schema_fn: schema_while_step,
};

static LOG_STEP_META: StepTypeMeta = StepTypeMeta {
    id: "Log",
    display_name: "Log",
    description: "Emit custom log/debug events",
    category: "utility",
    schema_fn: schema_log_step,
};

static CONNECTION_STEP_META: StepTypeMeta = StepTypeMeta {
    id: "Connection",
    display_name: "Connection",
    description: "Acquire a connection for secure agents",
    category: "utility",
    schema_fn: schema_connection_step,
};

static ERROR_STEP_META: StepTypeMeta = StepTypeMeta {
    id: "Error",
    display_name: "Error",
    description: "Emit a structured error and terminate workflow",
    category: "control",
    schema_fn: schema_error_step,
};

// Register all step types with inventory
inventory::submit! { &FINISH_STEP_META }
inventory::submit! { &AGENT_STEP_META }
inventory::submit! { &CONDITIONAL_STEP_META }
inventory::submit! { &SPLIT_STEP_META }
inventory::submit! { &SWITCH_STEP_META }
inventory::submit! { &START_SCENARIO_STEP_META }
inventory::submit! { &WHILE_STEP_META }
inventory::submit! { &LOG_STEP_META }
inventory::submit! { &CONNECTION_STEP_META }
inventory::submit! { &ERROR_STEP_META }
