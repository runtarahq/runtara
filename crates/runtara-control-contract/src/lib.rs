// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Contract of the control service (`runtara:control@1.0.0`).
//!
//! The WIT in `runtara-wit/wit/control` fixes the ABI; this crate
//! fixes the numbers and names every side agrees on: size and paging caps,
//! lineage depth, the control share of the concurrency limit, retry hints,
//! cancel bounds, the agent-facing `CONTROL_<CODE>` error codes and the
//! `start` input schema. It is serde-only, so the control agent, the host
//! and the server can all depend on it.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

const KIB: usize = 1024;
const MIB: usize = 1024 * KIB;

/// Largest agent input the control executor accepts.
pub const MAX_INPUT_BYTES: usize = MIB;
/// Largest output one control execution returns.
pub const MAX_OUTCOME_BYTES: usize = 4 * MIB;
/// Largest response of one control service call.
pub const MAX_RESPONSE_BYTES: usize = 4 * MIB;
/// Hard cap on one control execution, below the step's own deadline.
pub const EXECUTION_TIME_LIMIT_MS: u64 = 90_000;

/// `get` inlines a terminal output up to this size, else omits it.
pub const GET_OUTPUT_INLINE_BYTES: usize = MIB;
/// `get` inlines a terminal error up to this size, else omits it.
pub const GET_ERROR_INLINE_BYTES: usize = 64 * KIB;

/// Smallest `page-size` of `query` and `list-pending-signals`.
pub const PAGE_SIZE_MIN: u32 = 1;
/// Largest `page-size`; anything outside the range is `invalid`.
pub const PAGE_SIZE_MAX: u32 = 100;

/// Deepest lineage `start` admits: a run at this depth cannot start a child
/// (`invalid`). Top-level runs are depth 1.
pub const MAX_LINEAGE_DEPTH: u32 = 16;

/// Longest run label, in bytes. Must equal
/// `runtara_dsl::run_label::MAX_RUN_LABEL_LENGTH`.
pub const MAX_RUN_LABEL_BYTES: usize = 1024;

/// Slots control-started children may hold under a tenant concurrency limit:
/// `max(1, floor(0.8 × limit))`, so outside triggers keep headroom.
pub fn control_share(limit: u32) -> u32 {
    ((u64::from(limit) * 4 / 5) as u32).max(1)
}

/// Whether `start` can ever admit a child under `limit`. At a limit of at
/// most 1 the parent itself holds the only slot, so a full share there fails
/// permanently ([`CONTROL_CAPACITY_UNSATISFIABLE`]) instead of retrying.
pub fn capacity_satisfiable(limit: u32) -> bool {
    limit > 1
}

/// Shortest retry hint of a retryable `capacity` error.
pub const CAPACITY_RETRY_MIN_MS: u64 = 3_000;
/// Longest retry hint of a retryable `capacity` error.
pub const CAPACITY_RETRY_MAX_MS: u64 = 8_000;

/// A jittered `retry-after-ms` for a full control share, in
/// `CAPACITY_RETRY_MIN_MS..=CAPACITY_RETRY_MAX_MS`. The caller supplies the
/// entropy, so the contract stays deterministic.
pub fn capacity_retry_after_ms(entropy: u64) -> u64 {
    CAPACITY_RETRY_MIN_MS + entropy % (CAPACITY_RETRY_MAX_MS - CAPACITY_RETRY_MIN_MS + 1)
}

/// Grace before `cancel` forces a stop when the caller names none.
pub const DEFAULT_CANCEL_GRACE_MS: u64 = 5_000;
/// Longest grace `cancel` accepts; more is `invalid`.
pub const MAX_CANCEL_GRACE_MS: u64 = 3_600_000;

/// The grace `cancel` applies, or `None` when the request is out of range.
pub fn cancel_grace_ms(requested: Option<u64>) -> Option<u64> {
    match requested {
        None => Some(DEFAULT_CANCEL_GRACE_MS),
        Some(grace) if grace <= MAX_CANCEL_GRACE_MS => Some(grace),
        Some(_) => None,
    }
}

/// Grace the parent-close cascade gives a `cancel` child when its parent
/// ends in any way.
pub const PARENT_CLOSE_GRACE_MS: u64 = 5_000;

/// The principal the parent-close cascade stops children as.
pub const PARENT_CLOSE_PRINCIPAL: &str = "platform:parent-close";

/// The cancellation reason the cascade records on a child. `parent_status` is
/// the parent's terminal status, or `None` when its row is gone.
pub fn parent_close_reason(parent_instance_id: &str, parent_status: Option<&str>) -> String {
    format!(
        "parent {parent_instance_id} terminated ({})",
        parent_status.unwrap_or("missing")
    )
}

/// Capability tag: the capability only works inside a run (it needs a
/// calling instance or an operation scope), so a test invocation fails with
/// [`CONTROL_REQUIRES_INSTANCE`].
pub const REQUIRES_RUN_TAG: &str = "runtime:requires-run";

/// `timeout`: a control call ran past its host bound.
pub const CONTROL_TIMEOUT: &str = "CONTROL_TIMEOUT";
/// The control executor failed outside the capability (trap, limits).
pub const CONTROL_EXECUTION_FAILED: &str = "CONTROL_EXECUTION_FAILED";

/// `capacity` with a retry hint: the control share is full for now.
pub const CONTROL_CAPACITY_RATE_LIMITED: &str = "CONTROL_CAPACITY_RATE_LIMITED";
/// `capacity` without a retry hint: the limit can never admit a child.
pub const CONTROL_CAPACITY_UNSATISFIABLE: &str = "CONTROL_CAPACITY_UNSATISFIABLE";
/// `requires-instance`, also what a test invocation of a
/// [`REQUIRES_RUN_TAG`] capability gets.
pub const CONTROL_REQUIRES_INSTANCE: &str = "CONTROL_REQUIRES_INSTANCE";

/// `runtara:control/types.error-code`, case for case and in WIT order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ErrorCode {
    Denied,
    Invalid,
    NotFound,
    NotRunnable,
    NotChild,
    RequiresInstance,
    RequiresOperation,
    Capacity,
    ReplayConflict,
    LabelConflict,
    TooLarge,
    Unavailable,
    Unsupported,
    NotWaiting,
    Ambiguous,
    AlreadyAnswered,
    NotPausable,
    NotPaused,
    Timeout,
}

impl ErrorCode {
    /// Every code, in WIT declaration order.
    pub const ALL: [ErrorCode; 19] = [
        Self::Denied,
        Self::Invalid,
        Self::NotFound,
        Self::NotRunnable,
        Self::NotChild,
        Self::RequiresInstance,
        Self::RequiresOperation,
        Self::Capacity,
        Self::ReplayConflict,
        Self::LabelConflict,
        Self::TooLarge,
        Self::Unavailable,
        Self::Unsupported,
        Self::NotWaiting,
        Self::Ambiguous,
        Self::AlreadyAnswered,
        Self::NotPausable,
        Self::NotPaused,
        Self::Timeout,
    ];

    /// The WIT case name.
    pub fn wit_name(self) -> &'static str {
        match self {
            Self::Denied => "denied",
            Self::Invalid => "invalid",
            Self::NotFound => "not-found",
            Self::NotRunnable => "not-runnable",
            Self::NotChild => "not-child",
            Self::RequiresInstance => "requires-instance",
            Self::RequiresOperation => "requires-operation",
            Self::Capacity => "capacity",
            Self::ReplayConflict => "replay-conflict",
            Self::LabelConflict => "label-conflict",
            Self::TooLarge => "too-large",
            Self::Unavailable => "unavailable",
            Self::Unsupported => "unsupported",
            Self::NotWaiting => "not-waiting",
            Self::Ambiguous => "ambiguous",
            Self::AlreadyAnswered => "already-answered",
            Self::NotPausable => "not-pausable",
            Self::NotPaused => "not-paused",
            Self::Timeout => "timeout",
        }
    }

    /// The agent error code (`CONTROL_<CODE>`). `capacity` splits on the
    /// retry hint: with one it is [`CONTROL_CAPACITY_RATE_LIMITED`], without
    /// [`CONTROL_CAPACITY_UNSATISFIABLE`].
    pub fn agent_code(self, retry_after_ms: Option<u64>) -> &'static str {
        match self {
            Self::Denied => "CONTROL_DENIED",
            Self::Invalid => "CONTROL_INVALID",
            Self::NotFound => "CONTROL_NOT_FOUND",
            Self::NotRunnable => "CONTROL_NOT_RUNNABLE",
            Self::NotChild => "CONTROL_NOT_CHILD",
            Self::RequiresInstance => CONTROL_REQUIRES_INSTANCE,
            Self::RequiresOperation => "CONTROL_REQUIRES_OPERATION",
            Self::Capacity if retry_after_ms.is_some() => CONTROL_CAPACITY_RATE_LIMITED,
            Self::Capacity => CONTROL_CAPACITY_UNSATISFIABLE,
            Self::ReplayConflict => "CONTROL_REPLAY_CONFLICT",
            Self::LabelConflict => "CONTROL_LABEL_CONFLICT",
            Self::TooLarge => "CONTROL_TOO_LARGE",
            Self::Unavailable => "CONTROL_UNAVAILABLE",
            Self::Unsupported => "CONTROL_UNSUPPORTED",
            Self::NotWaiting => "CONTROL_NOT_WAITING",
            Self::Ambiguous => "CONTROL_AMBIGUOUS",
            Self::AlreadyAnswered => "CONTROL_ALREADY_ANSWERED",
            Self::NotPausable => "CONTROL_NOT_PAUSABLE",
            Self::NotPaused => "CONTROL_NOT_PAUSED",
            Self::Timeout => CONTROL_TIMEOUT,
        }
    }

    /// Whether the step may retry: `unavailable`, and `capacity` while the
    /// control share is full. A child's failure is outcome data, never a
    /// retryable control error.
    pub fn retryable(self, retry_after_ms: Option<u64>) -> bool {
        match self {
            Self::Unavailable => true,
            Self::Capacity => retry_after_ms.is_some(),
            _ => false,
        }
    }

    /// Every agent error code a control capability can surface, including
    /// both `capacity` spellings and the executor's own codes.
    pub fn all_agent_codes() -> Vec<&'static str> {
        let mut codes: Vec<_> = Self::ALL.iter().map(|code| code.agent_code(None)).collect();
        codes.push(CONTROL_CAPACITY_RATE_LIMITED);
        codes.push(CONTROL_EXECUTION_FAILED);
        codes
    }
}

/// The agent-facing error envelope for a control failure, in the
/// `#[capability]` JSON shape (`code`, `message`, `category`, `severity`,
/// `retryable`, `retry_after_ms`).
pub fn agent_error(code: ErrorCode, message: &str, retry_after_ms: Option<u64>) -> Value {
    let retryable = code.retryable(retry_after_ms);
    let mut error = json!({
        "code": code.agent_code(retry_after_ms),
        "message": message,
        "category": if retryable { "transient" } else { "permanent" },
        "severity": "error",
        "retryable": retryable,
    });
    if let Some(after) = retry_after_ms.filter(|_| retryable) {
        error["retry_after_ms"] = json!(after);
    }
    error
}

/// `runtara:control/types.parent-close-policy`. The agent input spells the
/// cases `cancel` and `leave_running`; the WIT spells them `cancel` and
/// `leave-running`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParentClosePolicy {
    /// Cancel the child when the parent ends in any way, after
    /// [`PARENT_CLOSE_GRACE_MS`]. The editor preselects it.
    Cancel,
    /// Leave the child running.
    LeaveRunning,
}

impl ParentClosePolicy {
    /// Every policy, in WIT declaration order.
    pub const ALL: [ParentClosePolicy; 2] = [Self::Cancel, Self::LeaveRunning];

    /// The agent input spelling.
    pub fn input_name(self) -> &'static str {
        match self {
            Self::Cancel => "cancel",
            Self::LeaveRunning => "leave_running",
        }
    }

    /// The WIT case name.
    pub fn wit_name(self) -> &'static str {
        match self {
            Self::Cancel => "cancel",
            Self::LeaveRunning => "leave-running",
        }
    }
}

/// Name of the required parent-close policy field of the `start` input.
pub const PARENT_CLOSE_POLICY_FIELD: &str = "parentClosePolicy";

/// JSON Schema of the `start` input. `parentClosePolicy` is required and has
/// no default, so an author must choose (validation keeps it unmapped, E022);
/// the editor preselects its first value, `cancel`.
pub fn start_input_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["workflowId", PARENT_CLOSE_POLICY_FIELD],
        "properties": {
            "workflowId": {
                "type": "string",
                "minLength": 1,
                "description": "Id of the workflow to start (not its slug)"
            },
            "version": {
                "type": "integer",
                "minimum": 1,
                "description": "Workflow version; defaults to the current version, fixed at admission"
            },
            "inputs": {
                "type": "object",
                "description": "The child's {data, variables} input envelope"
            },
            "runLabel": {
                "type": "string",
                "minLength": 1,
                "maxLength": MAX_RUN_LABEL_BYTES,
                "description": "Business identity of the child, unique per parent for the parent's lifetime"
            },
            PARENT_CLOSE_POLICY_FIELD: {
                "type": "string",
                "enum": ParentClosePolicy::ALL.map(ParentClosePolicy::input_name),
                "description": "What happens to the child if this run ends first: cancel it (after a 5 s grace) or leave it running"
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn the_control_share_keeps_a_fifth_of_the_limit_for_other_triggers() {
        for (limit, share) in [
            (0, 1),
            (1, 1),
            (2, 1),
            (3, 2),
            (4, 3),
            (5, 4),
            (10, 8),
            (11, 8),
            (100, 80),
            (u32::MAX, 3_435_973_836),
        ] {
            assert_eq!(control_share(limit), share, "limit {limit}");
        }
        assert!(!capacity_satisfiable(0));
        assert!(!capacity_satisfiable(1));
        assert!(capacity_satisfiable(2));
    }

    #[test]
    fn capacity_retry_hints_stay_in_range() {
        assert_eq!(capacity_retry_after_ms(0), CAPACITY_RETRY_MIN_MS);
        assert_eq!(capacity_retry_after_ms(5_000), CAPACITY_RETRY_MAX_MS);
        for entropy in [1, 7, 4_999, 5_001, u64::MAX] {
            let hint = capacity_retry_after_ms(entropy);
            assert!((CAPACITY_RETRY_MIN_MS..=CAPACITY_RETRY_MAX_MS).contains(&hint));
        }
    }

    #[test]
    fn error_codes_are_unique_and_capacity_splits_on_the_hint() {
        let wit: HashSet<_> = ErrorCode::ALL.iter().map(|c| c.wit_name()).collect();
        assert_eq!(wit.len(), ErrorCode::ALL.len());
        let agent = ErrorCode::all_agent_codes();
        assert_eq!(agent.iter().collect::<HashSet<_>>().len(), agent.len());
        assert!(agent.iter().all(|code| code.starts_with("CONTROL_")));
        for code in ErrorCode::ALL {
            let expected = format!(
                "CONTROL_{}",
                code.wit_name().to_ascii_uppercase().replace('-', "_")
            );
            if code != ErrorCode::Capacity {
                assert_eq!(code.agent_code(None), expected);
            }
        }
        assert_eq!(
            ErrorCode::Capacity.agent_code(Some(3_000)),
            CONTROL_CAPACITY_RATE_LIMITED
        );
        assert_eq!(
            ErrorCode::Capacity.agent_code(None),
            CONTROL_CAPACITY_UNSATISFIABLE
        );
        assert!(ErrorCode::Capacity.retryable(Some(3_000)));
        assert!(!ErrorCode::Capacity.retryable(None));
        assert!(ErrorCode::Unavailable.retryable(None));
        assert!(!ErrorCode::NotFound.retryable(Some(1)));
        assert!(!ErrorCode::Timeout.retryable(None));
    }

    #[test]
    fn agent_errors_carry_category_and_hint() {
        let error = agent_error(ErrorCode::Capacity, "share full", Some(4_000));
        assert_eq!(error["code"], CONTROL_CAPACITY_RATE_LIMITED);
        assert_eq!(error["category"], "transient");
        assert_eq!(error["retryable"], true);
        assert_eq!(error["retry_after_ms"], 4_000);
        let error = agent_error(ErrorCode::Capacity, "limit 1", None);
        assert_eq!(error["code"], CONTROL_CAPACITY_UNSATISFIABLE);
        assert_eq!(error["category"], "permanent");
        assert!(error.get("retry_after_ms").is_none());
        let error = agent_error(ErrorCode::NotChild, "not mine", Some(1));
        assert!(error.get("retry_after_ms").is_none());
    }

    #[test]
    fn cancel_bounds_and_the_parent_close_reason() {
        assert_eq!(cancel_grace_ms(None), Some(5_000));
        assert_eq!(cancel_grace_ms(Some(0)), Some(0));
        assert_eq!(cancel_grace_ms(Some(3_600_000)), Some(3_600_000));
        assert_eq!(cancel_grace_ms(Some(3_600_001)), None);
        assert_eq!(PARENT_CLOSE_GRACE_MS, DEFAULT_CANCEL_GRACE_MS);
        assert_eq!(
            parent_close_reason("parent-1", Some("failed")),
            "parent parent-1 terminated (failed)"
        );
        assert_eq!(
            parent_close_reason("parent-1", None),
            "parent parent-1 terminated (missing)"
        );
    }

    #[test]
    fn the_start_input_requires_a_parent_close_policy_without_a_default() {
        let schema = start_input_schema();
        let required: Vec<_> = schema["required"].as_array().unwrap().iter().collect();
        assert!(required.contains(&&json!(PARENT_CLOSE_POLICY_FIELD)));
        let policy = &schema["properties"][PARENT_CLOSE_POLICY_FIELD];
        assert_eq!(policy["enum"], json!(["cancel", "leave_running"]));
        assert!(policy.get("default").is_none(), "the author must choose");
        assert_eq!(
            serde_json::to_value(ParentClosePolicy::LeaveRunning).unwrap(),
            "leave_running"
        );
        assert_eq!(
            serde_json::from_value::<ParentClosePolicy>(json!("cancel")).unwrap(),
            ParentClosePolicy::Cancel
        );
        assert_eq!(
            ParentClosePolicy::ALL.map(ParentClosePolicy::wit_name),
            ["cancel", "leave-running"]
        );
        assert_eq!(
            schema["properties"]["runLabel"]["maxLength"],
            json!(runtara_dsl::run_label::MAX_RUN_LABEL_LENGTH)
        );
        // The validator reads these fields for W074 and W077.
        use runtara_dsl::step_context_rules as rules;
        for field in [
            rules::CONTROL_START_WORKFLOW_ID_FIELD,
            rules::CONTROL_START_RUN_LABEL_FIELD,
        ] {
            assert!(schema["properties"].get(field).is_some(), "{field}");
        }
    }
}
