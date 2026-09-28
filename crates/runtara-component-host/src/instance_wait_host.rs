// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Host side of durable instance waits: a run waiting on its direct
//! children.
//!
//! The embedding implements [`InstanceWaitHost`] and installs it once at
//! boot ([`crate::WorkflowExecutor::set_instance_wait_host`]). Compiled
//! workflow code reaches it through the host, never through agent bytes, so
//! the caller's identity always comes from the calling run's store
//! ([`InstanceWaitAuthority`]), and the wait id is derived by the host from
//! the step's checkpoint key the way an operation's `op_hash` is
//! ([`crate::operation_scope_host::operation_hash`]).
//!
//! Every type here serializes to JSON (camelCase fields, snake_case
//! values), so a host interface can carry requests, reads and errors as JSON
//! bytes. An outcome's `output` and `error` hold JSON bytes and serialize as
//! the JSON values they encode.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

/// Who waits. Only the host fills it in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstanceWaitAuthority {
    /// The tenant the waiting run belongs to.
    pub tenant: String,
    /// The waiting run.
    pub caller: String,
}

/// When a wait is satisfied.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstanceWaitMode {
    /// Every target has finished.
    All,
    /// The first target has finished.
    Any,
}

/// What to wait for. The host dedupes the ids; a replay must name the same
/// set and mode, and the first registration's deadline stands.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InstanceWaitRequest {
    /// Direct children of the caller, at most
    /// [`runtara_control_contract::MAX_WAIT_TARGETS`] distinct ones.
    pub instance_ids: Vec<String>,
    pub mode: InstanceWaitMode,
    /// Business deadline in milliseconds since the Unix epoch. When it
    /// passes the wait settles with what it observed; it never cancels a
    /// target.
    #[serde(default)]
    pub deadline_ms: Option<u64>,
}

/// How a settled wait ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstanceWaitResolution {
    /// The mode was met.
    Satisfied,
    /// The deadline passed first.
    Deadline,
    /// The wait named no targets.
    Empty,
}

/// A finished target's status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstanceWaitStatus {
    Pending,
    Running,
    Suspended,
    Completed,
    Failed,
    Cancelled,
    /// Its admission ended without a launch.
    NotStarted,
}

/// One finished target. Values over their cap are omitted and flagged,
/// never truncated.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstanceWaitOutcome {
    pub instance_id: String,
    pub status: InstanceWaitStatus,
    pub finished_at_ms: Option<u64>,
    /// The run's output as JSON bytes, when inlined.
    #[serde(with = "json_bytes")]
    pub output: Option<Vec<u8>>,
    /// Size of the full output, also when omitted.
    pub output_bytes: Option<u64>,
    pub output_omitted: bool,
    /// The run's error as JSON bytes, when inlined.
    #[serde(with = "json_bytes")]
    pub error: Option<Vec<u8>>,
    pub error_omitted: bool,
}

/// One read of a wait: finished targets in finish order, the remaining
/// ids, the persisted deadline, and the resolution once it settled.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstanceWaitPoll {
    pub mode: InstanceWaitMode,
    /// `None` while the wait is pending.
    pub resolution: Option<InstanceWaitResolution>,
    pub finished: Vec<InstanceWaitOutcome>,
    pub remaining: Vec<String>,
    pub deadline_ms: Option<u64>,
}

impl InstanceWaitPoll {
    /// Whether the wait has settled.
    pub fn is_settled(&self) -> bool {
        self.resolution.is_some()
    }
}

/// Why a wait call failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum InstanceWaitErrorCode {
    /// Malformed arguments, a target that is the caller itself, or a caller
    /// that has finished.
    Invalid,
    /// A target is an ancestor, or the tenant is not served here.
    Denied,
    /// A target is neither the caller nor an ancestor, and not its child.
    NotChild,
    /// A target or the wait does not exist (any more).
    NotFound,
    /// More targets than the cap.
    TooLarge,
    /// The wait id already waits on other targets or in another mode.
    ReplayConflict,
    /// The wait was closed or released.
    Closed,
    /// The service cannot answer now; retry.
    Unavailable,
}

impl InstanceWaitErrorCode {
    /// The code's spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Invalid => "invalid",
            Self::Denied => "denied",
            Self::NotChild => "not-child",
            Self::NotFound => "not-found",
            Self::TooLarge => "too-large",
            Self::ReplayConflict => "replay-conflict",
            Self::Closed => "closed",
            Self::Unavailable => "unavailable",
        }
    }
}

/// A failed wait call. The message carries no platform detail.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstanceWaitError {
    pub code: InstanceWaitErrorCode,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_ms: Option<u64>,
}

impl InstanceWaitError {
    pub fn new(code: InstanceWaitErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            retry_after_ms: None,
        }
    }

    /// `unavailable`, retryable after a second.
    pub fn unavailable(message: impl Into<String>) -> Self {
        Self {
            code: InstanceWaitErrorCode::Unavailable,
            message: message.into(),
            retry_after_ms: Some(1_000),
        }
    }
}

impl std::fmt::Display for InstanceWaitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code.as_str(), self.message)
    }
}

impl std::error::Error for InstanceWaitError {}

/// The durable instance wait service. `wait_id` is host-derived and scoped
/// to the caller: the same id of another caller is another wait.
#[async_trait::async_trait]
pub trait InstanceWaitHost: Send + Sync {
    /// Register the caller's wait `wait_id` on `request`, or find it on
    /// replay, and evaluate it. Targets are authorized before anything
    /// registers; a replay with other targets or another mode is
    /// `replay-conflict`, and one with another deadline keeps the first.
    async fn register(
        &self,
        authority: &InstanceWaitAuthority,
        wait_id: &str,
        request: InstanceWaitRequest,
    ) -> Result<InstanceWaitPoll, InstanceWaitError>;

    /// Evaluate the caller's registered wait `wait_id` without blocking.
    async fn poll(
        &self,
        authority: &InstanceWaitAuthority,
        wait_id: &str,
    ) -> Result<InstanceWaitPoll, InstanceWaitError>;
}

/// Prefix of a canonical v2 durable key.
const DURABLE_KEY_V2_PREFIX: &str = "runtara:v2:";

/// Key kind of a WaitForInstances step.
pub const WAIT_INSTANCES_KEY_KIND: &str = "wait-instances";

/// The wait id of a WaitForInstances step: the operation hash of its
/// canonical v2 `wait-instances` key
/// (`runtara:v2:["wait-instances", workflow, namespace, loop-path, [step]]`),
/// so it differs per loop iteration and per embedding and is the same on
/// every replay. Fails closed on any other key.
pub fn wait_instances_id(key: &str) -> Result<String, String> {
    let malformed = || "a wait key must be a canonical v2 wait-instances key".to_string();
    if key.is_empty() || key.len() > crate::operation_scope_host::MAX_OPERATION_KEY_BYTES {
        return Err(malformed());
    }
    let encoded = key
        .strip_prefix(DURABLE_KEY_V2_PREFIX)
        .ok_or_else(malformed)?;
    let tuple: serde_json::Value = serde_json::from_str(encoded).map_err(|_| malformed())?;
    // Canonical means the compiler's exact serialization: a key that parses
    // but re-serializes differently would hash to a different wait.
    if serde_json::to_string(&tuple).ok().as_deref() != Some(encoded) {
        return Err(malformed());
    }
    let parts = tuple
        .as_array()
        .filter(|parts| parts.len() == 5)
        .ok_or_else(malformed)?;
    let step = parts[4].as_array().and_then(|site| match site.as_slice() {
        [serde_json::Value::String(step)] if !step.is_empty() => Some(step),
        _ => None,
    });
    if parts[0].as_str() != Some(WAIT_INSTANCES_KEY_KIND) || step.is_none() {
        return Err(malformed());
    }
    Ok(crate::operation_scope_host::operation_hash(key))
}

/// Per-run access to the instance wait service: the host-installed service,
/// the run's own authority, and the waits this invoke registered that were
/// still pending at their last read.
#[derive(Default)]
pub(crate) struct RunInstanceWaits {
    host: Option<Arc<dyn InstanceWaitHost>>,
    authority: Option<InstanceWaitAuthority>,
    pending: Vec<String>,
}

impl RunInstanceWaits {
    /// Waits of the run `instance` of `tenant`, both host-supplied.
    pub(crate) fn for_run(
        host: Option<Arc<dyn InstanceWaitHost>>,
        tenant: Option<&str>,
        instance: Option<&str>,
    ) -> Self {
        let authority = match (tenant, instance) {
            (Some(tenant), Some(caller)) if !tenant.is_empty() && !caller.is_empty() => {
                Some(InstanceWaitAuthority {
                    tenant: tenant.to_owned(),
                    caller: caller.to_owned(),
                })
            }
            _ => None,
        };
        Self {
            host,
            authority,
            pending: Vec::new(),
        }
    }

    /// Waits registered by this invoke and pending at their last read; a
    /// suspended run parks on them.
    pub(crate) fn pending(&self) -> &[String] {
        &self.pending
    }

    fn note(&mut self, wait_id: &str, poll: &InstanceWaitPoll) {
        self.pending.retain(|pending| pending != wait_id);
        if !poll.is_settled() {
            self.pending.push(wait_id.to_owned());
        }
    }

    fn service(
        &self,
    ) -> Result<(Arc<dyn InstanceWaitHost>, InstanceWaitAuthority), InstanceWaitError> {
        let host = self.host.clone().ok_or_else(|| {
            InstanceWaitError::unavailable("no instance wait service is configured")
        })?;
        let authority = self.authority.clone().ok_or_else(|| {
            InstanceWaitError::new(
                InstanceWaitErrorCode::Invalid,
                "only a workflow run of a tenant can wait on instances",
            )
        })?;
        Ok((host, authority))
    }
}

/// A wait call's error as the JSON string the guest receives.
fn error_json(error: &InstanceWaitError) -> String {
    serde_json::to_string(error).expect("a wait error always serializes")
}

fn invalid_json(message: impl Into<String>) -> String {
    error_json(&InstanceWaitError::new(
        InstanceWaitErrorCode::Invalid,
        message,
    ))
}

/// A read as JSON bytes; a finished run's result that is not JSON fails the
/// read instead of reaching the guest.
fn poll_json(poll: &InstanceWaitPoll) -> Result<Vec<u8>, String> {
    serde_json::to_vec(poll).map_err(|_| {
        error_json(&InstanceWaitError::unavailable(
            "a finished run's result could not be read",
        ))
    })
}

/// Bind `runtara:workflow-wait/instances` for workflow stores. The tenant and
/// the waiting run come from the store, never from the guest; without an
/// installed service every call fails closed with `unavailable`.
pub(crate) fn add_instance_waits_to_linker(
    linker: &mut wasmtime::component::Linker<crate::workflow::WorkflowState>,
) -> anyhow::Result<()> {
    use crate::workflow::WorkflowState;
    use wasmtime::StoreContextMut;

    let mut instances = linker.instance(runtara_workflow_wit::WAIT_INSTANCES_INTERFACE_NAME)?;
    instances.func_wrap_async(
        "register",
        |mut store: StoreContextMut<'_, WorkflowState>, (key, request): (String, Vec<u8>)| {
            Box::new(async move {
                let wait_id = match wait_instances_id(&key) {
                    Ok(wait_id) => wait_id,
                    Err(error) => return Ok((Err(invalid_json(error)),)),
                };
                let request: InstanceWaitRequest = match serde_json::from_slice(&request) {
                    Ok(request) => request,
                    Err(error) => {
                        return Ok((Err(invalid_json(format!("malformed wait request: {error}"))),));
                    }
                };
                // No targets settle at once, without a registration.
                if request.instance_ids.is_empty() {
                    let poll = InstanceWaitPoll {
                        mode: request.mode,
                        resolution: Some(InstanceWaitResolution::Empty),
                        finished: Vec::new(),
                        remaining: Vec::new(),
                        deadline_ms: None,
                    };
                    store.data_mut().instance_waits.note(&wait_id, &poll);
                    return Ok((poll_json(&poll),));
                }
                let (host, authority) = match store.data().instance_waits.service() {
                    Ok(service) => service,
                    Err(error) => return Ok((Err(error_json(&error)),)),
                };
                match host.register(&authority, &wait_id, request).await {
                    Ok(poll) => {
                        store.data_mut().instance_waits.note(&wait_id, &poll);
                        Ok((poll_json(&poll),))
                    }
                    Err(error) => {
                        // A failed step is not checkpointed, so a retry or a
                        // replay registers again: close whatever this call
                        // left, and confirm it before the step's error path
                        // runs.
                        store
                            .data_mut()
                            .instance_waits
                            .pending
                            .retain(|pending| pending != &wait_id);
                        let runtime = store.data().runtime_host().cloned().ok_or_else(|| {
                            wasmtime::format_err!(
                                "instance waits need a runtime host (WorkflowRunSpec.runtime is None)"
                            )
                        })?;
                        runtime
                            .operation_wait_close(wait_id)
                            .await
                            .map_err(|close| {
                                wasmtime::format_err!("closing the failed instance wait failed: {close}")
                            })?;
                        Ok((Err(error_json(&error)),))
                    }
                }
            })
        },
    )?;
    instances.func_wrap_async(
        "poll",
        |mut store: StoreContextMut<'_, WorkflowState>, (key,): (String,)| {
            Box::new(async move {
                let wait_id = match wait_instances_id(&key) {
                    Ok(wait_id) => wait_id,
                    Err(error) => return Ok((Err(invalid_json(error)),)),
                };
                let (host, authority) = match store.data().instance_waits.service() {
                    Ok(service) => service,
                    Err(error) => return Ok((Err(error_json(&error)),)),
                };
                Ok((match host.poll(&authority, &wait_id).await {
                    Ok(poll) => {
                        store.data_mut().instance_waits.note(&wait_id, &poll);
                        poll_json(&poll)
                    }
                    Err(error) => Err(error_json(&error)),
                },))
            })
        },
    )?;
    instances.func_wrap_async(
        "release",
        |mut store: StoreContextMut<'_, WorkflowState>, (key,): (String,)| {
            Box::new(async move {
                let Ok(wait_id) = wait_instances_id(&key) else {
                    return Ok(());
                };
                store
                    .data_mut()
                    .instance_waits
                    .pending
                    .retain(|pending| pending != &wait_id);
                // The result is already checkpointed, so a failed release
                // only leaves a settled wait that instance cleanup removes.
                let released = match store.data().runtime_host().cloned() {
                    Some(runtime) => runtime.operation_release(wait_id).await,
                    None => Err("no runtime host".to_string()),
                };
                if let Err(error) = released {
                    tracing::warn!(%error, "instance wait release failed");
                }
                Ok(())
            })
        },
    )?;
    Ok(())
}

/// `Option<Vec<u8>>` of JSON bytes as the JSON value they encode.
mod json_bytes {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use serde_json::Value;

    pub fn serialize<S: Serializer>(bytes: &Option<Vec<u8>>, s: S) -> Result<S::Ok, S::Error> {
        match bytes {
            None => s.serialize_none(),
            Some(bytes) => serde_json::from_slice::<Value>(bytes)
                .map_err(serde::ser::Error::custom)?
                .serialize(s),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Vec<u8>>, D::Error> {
        Option::<Value>::deserialize(d)?
            .map(|value| serde_json::to_vec(&value).map_err(serde::de::Error::custom))
            .transpose()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn settled() -> InstanceWaitPoll {
        InstanceWaitPoll {
            mode: InstanceWaitMode::Any,
            resolution: Some(InstanceWaitResolution::Satisfied),
            finished: vec![InstanceWaitOutcome {
                instance_id: "kid".into(),
                status: InstanceWaitStatus::NotStarted,
                finished_at_ms: Some(7),
                output: Some(br#"{"ok":true}"#.to_vec()),
                output_bytes: Some(11),
                output_omitted: false,
                error: None,
                error_omitted: true,
            }],
            remaining: vec!["other".into()],
            deadline_ms: None,
        }
    }

    /// The JSON shape a host interface carries: the settled read is the
    /// WaitForInstances output (mode, resolution, finished, remaining, and
    /// the persisted deadline), with results as inline JSON values.
    #[test]
    fn a_poll_serializes_to_the_wait_output_shape() {
        let poll = settled();
        let value = serde_json::to_value(&poll).unwrap();
        assert_eq!(
            value,
            json!({
                "mode": "any",
                "resolution": "satisfied",
                "finished": [{
                    "instanceId": "kid",
                    "status": "not_started",
                    "finishedAtMs": 7,
                    "output": {"ok": true},
                    "outputBytes": 11,
                    "outputOmitted": false,
                    "error": null,
                    "errorOmitted": true,
                }],
                "remaining": ["other"],
                "deadlineMs": null,
            })
        );
        assert_eq!(
            serde_json::from_value::<InstanceWaitPoll>(value).unwrap(),
            poll
        );
        let pending = InstanceWaitPoll {
            resolution: None,
            ..poll
        };
        assert!(!pending.is_settled());
        assert_eq!(
            serde_json::to_value(&pending).unwrap()["resolution"],
            json!(null)
        );
    }

    fn wait_key(kind: &str, loop_path: serde_json::Value, site: serde_json::Value) -> String {
        format!("runtara:v2:{}", json!([kind, "wf", [], loop_path, site]))
    }

    #[test]
    fn a_wait_id_is_the_hash_of_a_canonical_wait_instances_key() {
        let root = wait_key("wait-instances", json!([]), json!(["approvals"]));
        assert_eq!(
            wait_instances_id(&root).unwrap(),
            crate::operation_scope_host::operation_hash(&root)
        );
        // Each iteration is its own wait.
        let iteration = wait_key(
            "wait-instances",
            json!([["split", "each", 1]]),
            json!(["approvals"]),
        );
        assert_ne!(
            wait_instances_id(&iteration).unwrap(),
            wait_instances_id(&root).unwrap()
        );
        for refused in [
            String::new(),
            "wait-instances::approvals".to_string(),
            wait_key("agent", json!([]), json!(["waiter", "pause", "approvals"])),
            wait_key("wait-instances", json!([]), json!([])),
            wait_key("wait-instances", json!([]), json!([""])),
            wait_key("wait-instances", json!([]), json!(["a", "b"])),
            root.replace(',', ", "),
            format!("{root}::attempt::2"),
        ] {
            assert!(wait_instances_id(&refused).is_err(), "{refused}");
        }
    }

    #[test]
    fn a_run_notes_only_waits_still_pending_at_their_last_read() {
        let mut waits = RunInstanceWaits::for_run(None, Some("t"), Some("run"));
        let pending = InstanceWaitPoll {
            resolution: None,
            ..settled()
        };
        waits.note("a", &pending);
        waits.note("b", &pending);
        waits.note("a", &pending);
        assert_eq!(waits.pending(), ["b", "a"]);
        waits.note("b", &settled());
        assert_eq!(waits.pending(), ["a"]);
        let error = waits.service().err().unwrap();
        assert_eq!(error.code, InstanceWaitErrorCode::Unavailable);
        // Without a host-supplied tenant and run there is no authority.
        let unowned = RunInstanceWaits::for_run(None, None, Some("run"));
        assert!(unowned.authority.is_none());
    }

    #[test]
    fn output_that_is_not_json_does_not_serialize() {
        let mut poll = settled();
        poll.finished[0].output = Some(b"not json".to_vec());
        assert!(serde_json::to_vec(&poll).is_err());
    }

    #[test]
    fn requests_and_errors_round_trip() {
        let request: InstanceWaitRequest = serde_json::from_value(json!({
            "instanceIds": ["a", "b"],
            "mode": "all",
        }))
        .unwrap();
        assert_eq!(
            request,
            InstanceWaitRequest {
                instance_ids: vec!["a".into(), "b".into()],
                mode: InstanceWaitMode::All,
                deadline_ms: None,
            }
        );
        assert!(
            serde_json::from_value::<InstanceWaitRequest>(json!({
                "instanceIds": [], "mode": "all", "timeoutMs": 5
            }))
            .is_err(),
            "unknown request fields are refused"
        );
        for code in [
            InstanceWaitErrorCode::Invalid,
            InstanceWaitErrorCode::Denied,
            InstanceWaitErrorCode::NotChild,
            InstanceWaitErrorCode::NotFound,
            InstanceWaitErrorCode::TooLarge,
            InstanceWaitErrorCode::ReplayConflict,
            InstanceWaitErrorCode::Closed,
            InstanceWaitErrorCode::Unavailable,
        ] {
            assert_eq!(serde_json::to_value(code).unwrap(), json!(code.as_str()));
        }
        let error = InstanceWaitError::unavailable("later");
        let value = serde_json::to_value(&error).unwrap();
        assert_eq!(
            value,
            json!({"code": "unavailable", "message": "later", "retryAfterMs": 1000})
        );
        assert_eq!(
            serde_json::from_value::<InstanceWaitError>(value).unwrap(),
            error
        );
        assert_eq!(error.to_string(), "unavailable: later");
    }
}
