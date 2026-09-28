//! `control:start`: durable, idempotent admission of a child run.
//!
//! [`normalize_start`] is pure: it rejects malformed arguments and builds the
//! `v1:` fingerprint of the normalized envelope. [`ExecutionEngine::start_child`]
//! then checks, in order: the operation's earlier admission (same fingerprint
//! replays the stored child, another is `replay-conflict`), the parent's run
//! label, the lineage depth, the workflow and version, the inputs, and the
//! compile state (a permanent failure is `not-runnable` and writes nothing; a
//! workflow not compiled yet is admitted and waits in the outbox until its
//! deadline). Capacity comes last, in the durable enqueue (decision D5).

use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::ExecutionEngine;
use crate::api::dto::trigger_event::TriggerEvent;
use crate::api::repositories::workflows::CompilationStatus;
use crate::workers::execution_outbox::{
    ChildAdmission, ControlChildRequest, ExecutionOutbox, ExecutionOutboxError,
    source_idempotency_key,
};
use runtara_control_contract as contract;
use runtara_workflows::input_validation::validate_workflow_start_inputs;

/// Longest workflow id `start` accepts.
const MAX_WORKFLOW_ID_BYTES: usize = 256;

/// A `control:start` request, normalized. Built by [`normalize_start`].
#[derive(Debug, Clone, PartialEq)]
pub struct ChildStart {
    /// Workflow id (never a slug).
    pub workflow_id: String,
    /// Requested version; `None` resolves the current one at admission.
    pub version: Option<i32>,
    /// The `{data, variables}` envelope, `_` variables dropped.
    pub inputs: Value,
    /// Normalized run label.
    pub run_label: Option<String>,
    /// `cancel` or `leave_running`.
    pub parent_close_policy: String,
    /// `v1:` + sha256 of the canonical JSON of the fields above.
    pub fingerprint: String,
}

/// The child an admission returned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartedChild {
    pub instance_id: String,
    pub workflow_id: String,
    pub version: i32,
    pub run_label: Option<String>,
    /// The operation had already admitted this child.
    pub replayed: bool,
}

/// Why `start` refused, in control's terms.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartChildError {
    /// Malformed arguments, inputs that fail the schema, or lineage too deep.
    Invalid(String),
    /// No such workflow or version in the tenant.
    NotFound(String),
    /// The workflow's compilation failed permanently.
    NotRunnable(String),
    /// The parent already gave the run label to another child.
    LabelConflict,
    /// The operation already admitted a child with other arguments.
    ReplayConflict,
    /// The concurrency limit or control's share of it is full. Not
    /// retryable when the limit can never admit a child (at most 1).
    Capacity { retryable: bool },
    /// Refused by an entitlement other than the concurrency limit.
    Denied(String),
    /// A transient platform failure.
    Unavailable(String),
}

/// The fingerprint version this build writes and compares.
pub const START_FINGERPRINT_PREFIX: &str = "v1:";

/// The idempotency key of a `control:start` operation:
/// `control:{caller}:{op_hash}`.
pub fn control_start_key(caller: &str, operation: &str) -> String {
    source_idempotency_key("control", &format!("{caller}:{operation}"))
}

/// Normalize `start` arguments without touching a database.
pub fn normalize_start(
    workflow_id: &str,
    version: Option<u32>,
    input: &[u8],
    run_label: Option<&str>,
    parent_close_policy: contract::ParentClosePolicy,
) -> Result<ChildStart, StartChildError> {
    let invalid = |message: &str| StartChildError::Invalid(message.to_owned());
    let workflow_id = workflow_id.trim();
    if workflow_id.is_empty()
        || workflow_id.len() > MAX_WORKFLOW_ID_BYTES
        || workflow_id.chars().any(char::is_control)
    {
        return Err(invalid(
            "workflowId must be 1-256 bytes of printable text (a workflow id, not a slug)",
        ));
    }
    let version = match version {
        None => None,
        Some(0) => return Err(invalid("version must be at least 1")),
        Some(version) => {
            Some(i32::try_from(version).map_err(|_| invalid("version is out of range"))?)
        }
    };
    let run_label = match run_label {
        None => None,
        Some(label) => {
            let normalized = runtara_dsl::run_label::normalize_run_label(Some(label))
                .map_err(|error| StartChildError::Invalid(format!("runLabel: {error}")))?;
            if normalized.is_none() {
                return Err(invalid("runLabel must not be empty"));
            }
            normalized
        }
    };
    let inputs = normalize_inputs(input)?;
    let parent_close_policy = parent_close_policy.input_name().to_owned();
    let canonical = runtara_core::persistence::inputs::canonical_payload(&json!({
        "workflowId": workflow_id,
        "version": version,
        "inputs": inputs,
        "runLabel": run_label,
        "parentClosePolicy": parent_close_policy,
    }));
    Ok(ChildStart {
        workflow_id: workflow_id.to_owned(),
        version,
        inputs,
        run_label,
        parent_close_policy,
        fingerprint: format!("{START_FINGERPRINT_PREFIX}{:x}", Sha256::digest(canonical)),
    })
}

/// The `{data, variables}` envelope: empty input is `{}`/`{}`, system
/// variables (`_` prefix) are dropped, and NUL characters, which the
/// database cannot store, are refused.
fn normalize_inputs(input: &[u8]) -> Result<Value, StartChildError> {
    let invalid = |message: &str| StartChildError::Invalid(message.to_owned());
    let mut envelope = if input.is_empty() {
        Map::new()
    } else {
        match serde_json::from_slice::<Value>(input) {
            Ok(Value::Object(map)) => map,
            Ok(_) => return Err(invalid("inputs must be a {data, variables} object")),
            Err(_) => return Err(invalid("inputs must be JSON")),
        }
    };
    envelope
        .entry("data")
        .or_insert_with(|| Value::Object(Map::new()));
    match envelope
        .entry("variables")
        .or_insert_with(|| Value::Object(Map::new()))
    {
        Value::Object(variables) => variables.retain(|key, _| !key.starts_with('_')),
        _ => return Err(invalid("inputs.variables must be an object")),
    }
    let envelope = Value::Object(envelope);
    if contains_nul(&envelope) {
        return Err(invalid("inputs must not contain NUL characters"));
    }
    Ok(envelope)
}

fn contains_nul(value: &Value) -> bool {
    match value {
        Value::String(text) => text.contains('\0'),
        Value::Array(items) => items.iter().any(contains_nul),
        Value::Object(map) => map
            .iter()
            .any(|(key, value)| key.contains('\0') || contains_nul(value)),
        _ => false,
    }
}

/// The lineage depth of a run from its lineage (itself first): top-level
/// runs are depth 1. A last entry that still names a parent had an ancestor
/// whose row is gone, which counts once.
pub fn lineage_depth(lineage: &[(String, Option<String>)]) -> u32 {
    let known = lineage.len().max(1) as u32;
    let missing = lineage.last().is_some_and(|(_, parent)| parent.is_some()) as u32;
    known.saturating_add(missing)
}

fn replayed(row: ControlChildRequest) -> StartedChild {
    StartedChild {
        instance_id: row.instance_id,
        workflow_id: row.workflow_id,
        version: row.workflow_version.unwrap_or_default(),
        run_label: row.run_label,
        replayed: true,
    }
}

/// The stored admission of this operation, compared with the replay.
fn replay(row: ControlChildRequest, fingerprint: &str) -> Result<StartedChild, StartChildError> {
    match row.start_fingerprint.as_deref() {
        Some(stored) if stored.starts_with(START_FINGERPRINT_PREFIX) => {
            if stored == fingerprint {
                Ok(replayed(row))
            } else {
                Err(StartChildError::ReplayConflict)
            }
        }
        _ => Err(StartChildError::Unavailable(
            "the stored admission has a fingerprint this build cannot compare".into(),
        )),
    }
}

fn unavailable(error: impl std::fmt::Display) -> StartChildError {
    StartChildError::Unavailable(error.to_string())
}

impl ExecutionEngine {
    /// Durably admit `start` as a child of `parent` (decisions D5-D7).
    pub async fn start_child(
        &self,
        tenant_id: &str,
        parent: &str,
        operation: &str,
        start: &ChildStart,
    ) -> Result<StartedChild, StartChildError> {
        let key = control_start_key(parent, operation);
        if let Some(row) = self
            .outbox
            .control_start(tenant_id, &key)
            .await
            .map_err(unavailable)?
        {
            return replay(row, &start.fingerprint);
        }
        if let Some(label) = start.run_label.as_deref()
            && self
                .outbox
                .parent_label_holder(tenant_id, parent, label)
                .await
                .map_err(unavailable)?
                .is_some()
        {
            return Err(StartChildError::LabelConflict);
        }

        let runtime = self.require_runtime_client().map_err(unavailable)?;
        let lineage = runtime
            .control_lineage(tenant_id, parent)
            .await
            .map_err(unavailable)?;
        if lineage_depth(&lineage) >= contract::MAX_LINEAGE_DEPTH {
            return Err(StartChildError::Invalid(format!(
                "the calling run is at the lineage depth limit ({}); it cannot start a child",
                contract::MAX_LINEAGE_DEPTH
            )));
        }

        let version = self
            .resolve_version(tenant_id, &start.workflow_id, start.version)
            .await
            .map_err(|error| match error {
                super::ExecutionError::NotFound(message) => StartChildError::NotFound(message),
                other => unavailable(other),
            })?;
        let workflow = self
            .workflow_repo
            .get_by_id(tenant_id, &start.workflow_id, Some(version))
            .await
            .map_err(unavailable)?
            .ok_or_else(|| {
                StartChildError::NotFound(format!(
                    "workflow '{}' version {version} not found",
                    start.workflow_id
                ))
            })?;
        let inputs = validate_workflow_start_inputs(start.inputs.clone(), &workflow.input_schema)
            .map_err(|error| StartChildError::Invalid(error.message))?;

        // Not compiled yet is admitted (decision D7); the launch path
        // compiles it and the outbox requeues it until its deadline.
        let (_, compilation) = self
            .workflow_repo
            .ensure_compilation_ready(tenant_id, &start.workflow_id, Some(version))
            .await
            .map_err(unavailable)?;
        if let CompilationStatus::Failed {
            error,
            terminal: true,
            ..
        } = compilation
        {
            return Err(StartChildError::NotRunnable(format!(
                "workflow '{}' version {version} failed to compile: {error}",
                start.workflow_id
            )));
        }

        let cap = self.effective_concurrency_cap();
        let limit = u32::try_from(cap).unwrap_or(u32::MAX);
        if !contract::capacity_satisfiable(limit) {
            return Err(StartChildError::Capacity { retryable: false });
        }
        if let Err(denial) = self.try_admit_locally(tenant_id).await {
            return Err(match denial {
                crate::entitlement_error::EntitlementDenial::LimitExceeded { .. } => {
                    StartChildError::Capacity { retryable: true }
                }
                other => StartChildError::Denied(other.message()),
            });
        }

        let instance_id = Uuid::new_v4().to_string();
        let event = TriggerEvent::control(
            instance_id.clone(),
            tenant_id.to_owned(),
            start.workflow_id.clone(),
            version,
            inputs,
            workflow.track_events,
            parent.to_owned(),
            start.parent_close_policy.clone(),
            operation.to_owned(),
            start.run_label.clone(),
            chrono::Utc::now().timestamp_millis(),
        );
        let admission = ChildAdmission {
            parent_instance_id: parent,
            parent_close_policy: &start.parent_close_policy,
            operation,
            fingerprint: &start.fingerprint,
            control_share: u64::from(contract::control_share(limit)),
        };
        let enqueued = self
            .outbox
            .enqueue_child(tenant_id, &event, &key, cap, admission)
            .await;
        match enqueued {
            Ok(enqueued) if enqueued.duplicate => {
                self.release_local_reservation(tenant_id);
                // A concurrent attempt of this operation won the admission.
                let row = self
                    .outbox
                    .control_start(tenant_id, &key)
                    .await
                    .map_err(unavailable)?
                    .ok_or_else(|| unavailable("the admitted child vanished"))?;
                replay(row, &start.fingerprint)
            }
            Ok(_) => Ok(StartedChild {
                instance_id,
                workflow_id: start.workflow_id.clone(),
                version,
                run_label: start.run_label.clone(),
                replayed: false,
            }),
            Err(error) => {
                self.release_local_reservation(tenant_id);
                Err(match error {
                    ExecutionOutboxError::AdmissionFull { .. }
                    | ExecutionOutboxError::ControlShareFull { .. } => {
                        StartChildError::Capacity { retryable: true }
                    }
                    ExecutionOutboxError::ParentRunLabelConflict => StartChildError::LabelConflict,
                    ExecutionOutboxError::StartReplayConflict => StartChildError::ReplayConflict,
                    ExecutionOutboxError::InvalidRunLabel(message) => {
                        StartChildError::Invalid(message)
                    }
                    other => unavailable(other),
                })
            }
        }
    }

    /// The source request of a `control:start` child of `tenant`, if the
    /// server admitted one with that id.
    pub async fn control_child(
        &self,
        tenant_id: &str,
        instance_id: &str,
    ) -> Result<Option<ControlChildRequest>, ExecutionOutboxError> {
        self.outbox.control_child(tenant_id, instance_id).await
    }

    /// The server's admission records, for the control service's cancel and
    /// never-launched outcomes.
    pub fn outbox(&self) -> &ExecutionOutbox {
        &self.outbox
    }

    /// The children of `parent` still in admission, in admission order.
    pub async fn admitted_children(
        &self,
        tenant_id: &str,
        parent: &str,
    ) -> Result<Vec<ControlChildRequest>, ExecutionOutboxError> {
        self.outbox.admitted_children(tenant_id, parent).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use contract::ParentClosePolicy::{Cancel, LeaveRunning};

    fn start(input: &str) -> Result<ChildStart, StartChildError> {
        normalize_start("wf-1", None, input.as_bytes(), Some("order-7"), Cancel)
    }

    #[test]
    fn the_fingerprint_follows_the_normalized_envelope() {
        let a =
            start(r#"{"data": {"a": 1, "b": 2}, "variables": {"x": 1, "_system": 9}}"#).unwrap();
        let b = start(r#"{"variables": {"x": 1}, "data": {"b": 2, "a": 1}}"#).unwrap();
        assert_eq!(
            a.fingerprint, b.fingerprint,
            "key order and `_` vars do not count"
        );
        assert!(a.fingerprint.starts_with("v1:"));
        assert_eq!(a.fingerprint.len(), "v1:".len() + 64);
        assert_eq!(
            a.inputs,
            json!({"data": {"a": 1, "b": 2}, "variables": {"x": 1}})
        );
        // An empty input is the empty envelope.
        let empty = start("").unwrap();
        assert_eq!(empty.inputs, json!({"data": {}, "variables": {}}));
        assert_eq!(empty.fingerprint, start("{}").unwrap().fingerprint);
        // Every argument counts.
        let base = start("{}").unwrap().fingerprint;
        for other in [
            normalize_start("wf-2", None, b"{}", Some("order-7"), Cancel),
            normalize_start("wf-1", Some(1), b"{}", Some("order-7"), Cancel),
            normalize_start("wf-1", None, b"{}", Some("order-8"), Cancel),
            normalize_start("wf-1", None, b"{}", None, Cancel),
            normalize_start("wf-1", None, b"{}", Some("order-7"), LeaveRunning),
            normalize_start(
                "wf-1",
                None,
                br#"{"data":{"a":1}}"#,
                Some("order-7"),
                Cancel,
            ),
        ] {
            assert_ne!(other.unwrap().fingerprint, base);
        }
    }

    #[test]
    fn malformed_arguments_are_invalid_before_any_read() {
        let invalid = |result: Result<ChildStart, StartChildError>| {
            matches!(result, Err(StartChildError::Invalid(_)))
        };
        assert!(invalid(normalize_start("", None, b"{}", None, Cancel)));
        assert!(invalid(normalize_start(" ", None, b"{}", None, Cancel)));
        assert!(invalid(normalize_start(
            &"w".repeat(257),
            None,
            b"{}",
            None,
            Cancel
        )));
        assert!(invalid(normalize_start("wf", Some(0), b"{}", None, Cancel)));
        assert!(invalid(normalize_start(
            "wf",
            Some(u32::MAX),
            b"{}",
            None,
            Cancel
        )));
        assert!(invalid(normalize_start("wf", None, b"[]", None, Cancel)));
        assert!(invalid(normalize_start(
            "wf",
            None,
            b"not json",
            None,
            Cancel
        )));
        assert!(invalid(normalize_start(
            "wf",
            None,
            br#"{"variables": 1}"#,
            None,
            Cancel
        )));
        assert!(invalid(normalize_start(
            "wf",
            None,
            br#"{"data": {"a": "x\u0000y"}}"#,
            None,
            Cancel
        )));
        for label in ["", " ", "a\u{0}b", "line\nbreak"] {
            assert!(
                invalid(normalize_start("wf", None, b"{}", Some(label), Cancel)),
                "{label:?}"
            );
        }
        assert!(invalid(normalize_start(
            "wf",
            None,
            b"{}",
            Some(&"x".repeat(1025)),
            Cancel
        )));
        assert!(normalize_start("wf", None, b"{}", Some(&"x".repeat(1024)), Cancel).is_ok());
    }

    #[test]
    fn the_idempotency_key_is_namespaced_by_caller_and_operation() {
        assert_eq!(control_start_key("parent", "op"), "control:parent:op");
        assert_ne!(control_start_key("p", "op"), control_start_key("q", "op"));
    }

    #[test]
    fn depth_counts_the_run_and_its_ancestors() {
        let chain = |ids: &[(&str, Option<&str>)]| {
            ids.iter()
                .map(|(id, parent)| (id.to_string(), parent.map(str::to_owned)))
                .collect::<Vec<_>>()
        };
        assert_eq!(lineage_depth(&[]), 1);
        assert_eq!(lineage_depth(&chain(&[("root", None)])), 1);
        assert_eq!(lineage_depth(&chain(&[("c", Some("p")), ("p", None)])), 2);
        // A cleaned-up ancestor still counts.
        assert_eq!(lineage_depth(&chain(&[("c", Some("gone"))])), 2);
        let deep: Vec<_> = (0..16)
            .map(|i| (format!("r{i}"), (i < 15).then(|| format!("r{}", i + 1))))
            .collect();
        assert_eq!(lineage_depth(&deep), contract::MAX_LINEAGE_DEPTH);
    }

    #[test]
    fn a_replay_compares_the_stored_fingerprint() {
        let row = |fingerprint: Option<&str>| ControlChildRequest {
            request_id: Uuid::nil(),
            instance_id: "child".into(),
            workflow_id: "wf".into(),
            workflow_version: Some(3),
            run_label: Some("l".into()),
            parent_instance_id: Some("parent".into()),
            parent_close_policy: Some("cancel".into()),
            start_fingerprint: fingerprint.map(str::to_owned),
            state: "queued".into(),
            terminal_reason: None,
            outcome: None,
            outcome_reason: None,
            created_at: chrono::Utc::now(),
        };
        let child = replay(row(Some("v1:a")), "v1:a").unwrap();
        assert!(child.replayed);
        assert_eq!((child.instance_id.as_str(), child.version), ("child", 3));
        assert_eq!(
            replay(row(Some("v1:b")), "v1:a"),
            Err(StartChildError::ReplayConflict)
        );
        for unknown in [None, Some("v9:a")] {
            assert!(matches!(
                replay(row(unknown), "v1:a"),
                Err(StartChildError::Unavailable(_))
            ));
        }
    }
}
