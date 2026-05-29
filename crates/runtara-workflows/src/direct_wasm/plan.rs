// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Direct core run-plan model used by the direct WebAssembly emitter.

#[derive(Debug, Clone)]
pub(super) enum DirectRunPlan {
    Finish {
        step_id: String,
        mapping_id: u32,
    },
    Filter {
        step_id: String,
        filter_id: u32,
        next_plan: Box<DirectRunPlan>,
    },
    SwitchValue {
        step_id: String,
        switch_id: u32,
        next_plan: Box<DirectRunPlan>,
    },
    SwitchRoute {
        step_id: String,
        switch_id: u32,
        branches: Vec<DirectSwitchRoutePlan>,
        default_plan: Box<DirectRunPlan>,
    },
    EdgeRoute {
        branches: Vec<DirectEdgeConditionPlan>,
        default_plan: Box<DirectRunPlan>,
    },
    GroupBy {
        step_id: String,
        group_id: u32,
        next_plan: Box<DirectRunPlan>,
    },
    Split {
        step_id: String,
        split_id: u32,
        durable: bool,
        dont_stop_on_failed: bool,
        nested_plan: Box<DirectRunPlan>,
        next_plan: Box<DirectRunPlan>,
    },
    While {
        step_id: String,
        while_id: u32,
        nested_plan: Box<DirectRunPlan>,
        next_plan: Box<DirectRunPlan>,
    },
    Delay {
        step_id: String,
        delay_id: u32,
        durable: bool,
        next_plan: Box<DirectRunPlan>,
    },
    WaitForSignal {
        step_id: String,
        breakpoint: bool,
        on_wait_plan: Option<Box<DirectRunPlan>>,
        next_plan: Box<DirectRunPlan>,
    },
    Log {
        log_id: u32,
        next_plan: Box<DirectRunPlan>,
    },
    Agent {
        step_id: String,
        agent_id: u32,
        agent_component_id: String,
        input_mapping_id: u32,
        durable_checkpoint: bool,
        max_retries: u32,
        retry_delay_ms: u64,
        rate_limit_budget_ms: u64,
        next_plan: Box<DirectRunPlan>,
        error_plan: Option<DirectErrorRoutePlan>,
    },
    Error {
        step_id: String,
        error_id: u32,
    },
    Conditional {
        step_id: String,
        condition_id: u32,
        true_plan: Box<DirectRunPlan>,
        false_plan: Box<DirectRunPlan>,
    },
}

#[derive(Debug, Clone)]
pub(super) struct DirectSwitchRoutePlan {
    pub(super) label: String,
    pub(super) plan: Box<DirectRunPlan>,
}

#[derive(Debug, Clone)]
pub(super) struct DirectEdgeConditionPlan {
    pub(super) condition_id: u32,
    pub(super) plan: Box<DirectRunPlan>,
}

#[derive(Debug, Clone)]
pub(super) struct DirectErrorRoutePlan {
    pub(super) branches: Vec<DirectEdgeConditionPlan>,
    pub(super) default_plan: Option<Box<DirectRunPlan>>,
}

#[derive(Debug, Clone, Copy)]
pub(super) enum DirectFailureTarget {
    Split {
        split_id: u32,
        branch_depth: u32,
    },
    WaitOnWait {
        step_id_offset: i32,
        step_id_len: i32,
    },
}

impl DirectFailureTarget {
    pub(super) fn nested(self, extra_depth: u32) -> Self {
        match self {
            Self::Split {
                split_id,
                branch_depth,
            } => Self::Split {
                split_id,
                branch_depth: branch_depth + extra_depth,
            },
            Self::WaitOnWait {
                step_id_offset,
                step_id_len,
            } => Self::WaitOnWait {
                step_id_offset,
                step_id_len,
            },
        }
    }
}
