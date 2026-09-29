// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Host side of a run's queryable state (`runtara:workflow/state`).
//!
//! The embedding implements [`RunStateHost`] and installs it once
//! ([`crate::WorkflowExecutor::set_run_state_host`]). Compiled workflow code
//! reaches it through the host, never through agent bytes, so the run always
//! comes from the calling store ([`RunStateAuthority`]), and a write's
//! identity is derived by the host from the SetState step's checkpoint key
//! the way an operation's `op_hash` is
//! ([`crate::operation_scope_host::operation_hash`]).
//!
//! Errors reach the guest as JSON `{code, message}`.

use std::sync::Arc;

use serde::Serialize;
use serde_json::{Map, Value};

/// Whose state. Only the host fills it in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunStateAuthority {
    /// The tenant the run belongs to.
    pub tenant: String,
    /// The run.
    pub instance: String,
}

/// Why a state call failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum RunStateErrorCode {
    /// A malformed key or patch.
    Invalid,
    /// The merged state would exceed the size cap.
    TooLarge,
    /// The run is not running (or not found).
    NotRunning,
    /// The state service is unavailable; retryable.
    Unavailable,
}

/// A state call's error, as the guest receives it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RunStateError {
    pub code: RunStateErrorCode,
    pub message: String,
}

impl RunStateError {
    pub fn new(code: RunStateErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    fn json(&self) -> String {
        serde_json::to_string(self).expect("a state error always serializes")
    }
}

impl std::fmt::Display for RunStateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}: {}", self.code, self.message)
    }
}

impl std::error::Error for RunStateError {}

/// The run state service.
#[async_trait::async_trait]
pub trait RunStateHost: Send + Sync {
    /// Merge `patch` into the run's state once per `operation_id`; a repeat
    /// changes nothing and succeeds.
    async fn set(
        &self,
        authority: &RunStateAuthority,
        operation_id: &str,
        patch: Map<String, Value>,
    ) -> Result<(), RunStateError>;

    /// The run's current state, empty before any write.
    async fn get(&self, authority: &RunStateAuthority)
    -> Result<Map<String, Value>, RunStateError>;
}

/// Prefix of a canonical v2 durable key.
const DURABLE_KEY_V2_PREFIX: &str = "runtara:v2:";

/// Key kind of a SetState step.
pub const STATE_KEY_KIND: &str = "state";

/// The write identity of a SetState step: the operation hash of its canonical
/// v2 `state` key (`runtara:v2:["state", workflow, namespace, loop-path,
/// [step]]`), so it differs per loop iteration and is the same on every
/// replay. Fails closed on any other key.
pub fn state_operation_id(key: &str) -> Result<String, String> {
    let malformed = || "a state key must be a canonical v2 state key".to_string();
    if key.is_empty() || key.len() > crate::operation_scope_host::MAX_OPERATION_KEY_BYTES {
        return Err(malformed());
    }
    let encoded = key
        .strip_prefix(DURABLE_KEY_V2_PREFIX)
        .ok_or_else(malformed)?;
    let tuple: Value = serde_json::from_str(encoded).map_err(|_| malformed())?;
    // Canonical means the compiler's exact serialization: a key that parses
    // but re-serializes differently would hash to a different write.
    if serde_json::to_string(&tuple).ok().as_deref() != Some(encoded) {
        return Err(malformed());
    }
    let parts = tuple
        .as_array()
        .filter(|parts| parts.len() == 5)
        .ok_or_else(malformed)?;
    let step = parts[4].as_array().and_then(|site| match site.as_slice() {
        [Value::String(step)] if !step.is_empty() => Some(step),
        _ => None,
    });
    if parts[0].as_str() != Some(STATE_KEY_KIND) || step.is_none() {
        return Err(malformed());
    }
    Ok(crate::operation_scope_host::operation_hash(key))
}

/// Per-run access to the state service: the host-installed service and the
/// run's own authority.
#[derive(Default)]
pub(crate) struct RunStateAccess {
    host: Option<Arc<dyn RunStateHost>>,
    authority: Option<RunStateAuthority>,
}

impl RunStateAccess {
    /// State of the run `instance` of `tenant`, both host-supplied.
    pub(crate) fn for_run(
        host: Option<Arc<dyn RunStateHost>>,
        tenant: Option<&str>,
        instance: Option<&str>,
    ) -> Self {
        let authority = match (tenant, instance) {
            (Some(tenant), Some(instance)) if !tenant.is_empty() && !instance.is_empty() => {
                Some(RunStateAuthority {
                    tenant: tenant.to_owned(),
                    instance: instance.to_owned(),
                })
            }
            _ => None,
        };
        Self { host, authority }
    }

    fn service(&self) -> Result<(Arc<dyn RunStateHost>, RunStateAuthority), RunStateError> {
        let host = self.host.clone().ok_or_else(|| {
            RunStateError::new(
                RunStateErrorCode::Unavailable,
                "no run state service is configured",
            )
        })?;
        let authority = self.authority.clone().ok_or_else(|| {
            RunStateError::new(
                RunStateErrorCode::Invalid,
                "only a workflow run of a tenant has state",
            )
        })?;
        Ok((host, authority))
    }
}

/// Bind `runtara:workflow/state` for workflow stores. The tenant and the run
/// come from the store, never from the guest; without an installed service
/// every call fails closed with `unavailable`.
pub(crate) fn add_run_state_to_linker(
    linker: &mut wasmtime::component::Linker<crate::workflow::WorkflowState>,
) -> anyhow::Result<()> {
    use crate::workflow::WorkflowState;
    use wasmtime::StoreContextMut;

    let invalid = |message: String| RunStateError::new(RunStateErrorCode::Invalid, message).json();

    let mut state = linker.instance(runtara_wit::workflow::STATE)?;
    state.func_wrap_async(
        "set",
        move |store: StoreContextMut<'_, WorkflowState>, (key, patch): (String, Vec<u8>)| {
            Box::new(async move {
                let operation_id = match state_operation_id(&key) {
                    Ok(operation_id) => operation_id,
                    Err(error) => return Ok((Err(invalid(error)),)),
                };
                let patch = match serde_json::from_slice::<Value>(&patch) {
                    Ok(Value::Object(patch)) => patch,
                    Ok(_) => {
                        return Ok((Err(invalid("a state patch must be a JSON object".into())),));
                    }
                    Err(error) => {
                        return Ok((Err(invalid(format!("malformed state patch: {error}"))),));
                    }
                };
                let (host, authority) = match store.data().run_state.service() {
                    Ok(service) => service,
                    Err(error) => return Ok((Err(error.json()),)),
                };
                Ok((host
                    .set(&authority, &operation_id, patch)
                    .await
                    .map_err(|error| error.json()),))
            })
        },
    )?;
    state.func_wrap_async(
        "get",
        |store: StoreContextMut<'_, WorkflowState>, (): ()| {
            Box::new(async move {
                let (host, authority) = match store.data().run_state.service() {
                    Ok(service) => service,
                    Err(error) => return Ok((Err(error.json()),)),
                };
                Ok((match host.get(&authority).await {
                    Ok(state) => Ok(serde_json::to_vec(&Value::Object(state))
                        .expect("a JSON object always serializes")),
                    Err(error) => Err(error.json()),
                },))
            })
        },
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(parts: Value) -> String {
        format!("runtara:v2:{}", serde_json::to_string(&parts).unwrap())
    }

    #[test]
    fn state_keys_hash_to_operation_ids() {
        let a = key(serde_json::json!(["state", "root", [], [], ["set"]]));
        let b = key(serde_json::json!([
            "state",
            "root",
            [],
            [["while", "loop", 1]],
            ["set"]
        ]));
        let id = state_operation_id(&a).unwrap();
        assert_eq!(id.len(), 64);
        assert_eq!(id, crate::operation_scope_host::operation_hash(&a));
        assert_ne!(id, state_operation_id(&b).unwrap());
    }

    #[test]
    fn other_keys_are_refused() {
        for bad in [
            String::new(),
            "state".to_string(),
            key(serde_json::json!([
                "wait-instances",
                "root",
                [],
                [],
                ["set"]
            ])),
            key(serde_json::json!(["state", "root", [], [], []])),
            key(serde_json::json!(["state", "root", [], [], ["a", "b"]])),
            key(serde_json::json!(["state", "root", [], []])),
            "runtara:v2:[\"state\", \"root\", [], [], [\"set\"]]".to_string(),
        ] {
            assert!(state_operation_id(&bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn errors_serialize_as_code_and_message() {
        let error = RunStateError::new(RunStateErrorCode::TooLarge, "too big");
        assert_eq!(error.json(), r#"{"code":"too-large","message":"too big"}"#);
    }
}
