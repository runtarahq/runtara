// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Host side of typed agent suspension.
//!
//! - `runtara:workflow/operation`, imported only by compiled workflow
//!   logic, names the operation of an operation-scoped call site (a suspending
//!   capability or a control call, durable or not). `enter` parses the site's
//!   canonical v2 Agent checkpoint key into an [`OperationIdentity`] and the
//!   host keys everything by `op_hash = sha256(checkpoint-key)` inside the
//!   run's own instance; nothing about the identity comes from agent input.
//!   The entered `op_hash` is the `operation` of every control call the site
//!   makes (`ControlAuthority`), and a second `enter` in one run fails closed.
//! - `runtara:agent/continuation.continuation()` hands a suspending
//!   agent the continuation of the entered operation.
//!
//! An agent suspension may wake at a time (`at`). Nothing lets an agent
//! register an instance wait, so an `instances` wake is refused as
//! `AGENT_INVALID_SUSPENSION`; a run waits on other runs with the
//! `WaitForInstances` step (`instance_wait_host`).
//!
//! Persistence goes through [`crate::runtime_host::RuntimeHost`]; hosts
//! without typed suspension refuse, which fails the step instead of losing
//! state.

use sha2::{Digest, Sha256};
use wasmtime::StoreContextMut;
use wasmtime::component::Linker;

use crate::workflow::WorkflowState;

/// WIT mirror of `runtara:agent/suspension.wake`.
#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    wasmtime::component::ComponentType,
    wasmtime::component::Lift,
    wasmtime::component::Lower,
)]
#[component(variant)]
pub enum SuspensionWake {
    /// Re-invoke at (or after) this wall-clock ms since the Unix epoch.
    #[component(name = "at")]
    At(u64),
    /// Re-invoke when this host-owned instance wait settles.
    #[component(name = "instances")]
    Instances(String),
}

/// WIT mirror of `runtara:agent/suspension.suspension`.
#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    wasmtime::component::ComponentType,
    wasmtime::component::Lift,
    wasmtime::component::Lower,
)]
#[component(record)]
pub struct Suspension {
    pub wakes: Vec<SuspensionWake>,
    pub state: Vec<u8>,
}

/// WIT mirror of `runtara:agent/suspension.outcome`.
#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    wasmtime::component::ComponentType,
    wasmtime::component::Lift,
    wasmtime::component::Lower,
)]
#[component(variant)]
pub enum SuspendableOutcome {
    #[component(name = "completed")]
    Completed(Vec<u8>),
    #[component(name = "suspended")]
    Suspended(Suspension),
}

impl SuspensionWake {
    fn to_contract(&self) -> runtara_agent_suspension::Wake {
        match self {
            Self::At(at) => runtara_agent_suspension::Wake::At(*at),
            Self::Instances(id) => runtara_agent_suspension::Wake::Instances(id.clone()),
        }
    }
}

/// Largest checkpoint key `enter` accepts.
pub const MAX_OPERATION_KEY_BYTES: usize = 16 * 1024;

/// Prefix of a canonical v2 durable key.
const DURABLE_KEY_V2_PREFIX: &str = "runtara:v2:";

/// Suffixes the runtime appends to a step key for per-attempt results, retry
/// sleeps and retry audit rows. None of them names an operation.
const DERIVED_KEY_SUFFIXES: [&str; 3] = ["::attempt::", "::retry_sleep::", "::retry::"];

/// The identity of an operation-scoped call site, parsed from the canonical
/// v2 Agent checkpoint key the compiler hands `scope.enter`. Frozen with
/// `runtara:workflow@1.0.0`: the key is
/// `runtara:v2:["agent", workflow, namespace, loop-path, [agent, capability, step]]`,
/// so it differs per loop iteration and per embedding and is the same on every
/// replay and retry of the same site.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationIdentity {
    /// The checkpoint key as entered.
    pub key: String,
    /// Hex sha256 of `key`; the host keys everything about the operation by it.
    pub op_hash: String,
    /// One-based attempt of the entering site.
    pub attempt: u32,
    /// Agent id of the call site.
    pub agent_id: String,
    /// Step id of the call site.
    pub step_id: String,
}

/// Parse the checkpoint key of an operation-scoped Agent call site. Fails
/// closed on anything but a canonical v2 `agent` key: another kind, a legacy
/// key, a derived per-attempt, retry-sleep or retry-audit key, a key over
/// [`MAX_OPERATION_KEY_BYTES`], or attempt 0.
pub fn parse_agent_operation_key(key: &str, attempt: u32) -> Result<OperationIdentity, String> {
    if key.is_empty() || key.len() > MAX_OPERATION_KEY_BYTES {
        return Err(format!(
            "an operation key must be 1-{MAX_OPERATION_KEY_BYTES} bytes"
        ));
    }
    if attempt == 0 {
        return Err("an operation attempt is one-based".into());
    }
    for suffix in DERIVED_KEY_SUFFIXES {
        if let Some((_, tail)) = key.rsplit_once(suffix)
            && !tail.is_empty()
            && tail.bytes().all(|byte| byte.is_ascii_digit())
        {
            return Err(format!(
                "`{}` keys are derived from a step key and name no operation",
                suffix.trim_matches(':')
            ));
        }
    }
    let encoded = key
        .strip_prefix(DURABLE_KEY_V2_PREFIX)
        .ok_or("an operation key must be a canonical v2 durable key")?;
    let tuple: serde_json::Value = serde_json::from_str(encoded)
        .map_err(|_| "an operation key must be a canonical v2 durable key".to_string())?;
    // Canonical means the compiler's exact serialization: a key that parses
    // but re-serializes differently would hash to a different operation.
    if serde_json::to_string(&tuple).ok().as_deref() != Some(encoded) {
        return Err("an operation key must be a canonical v2 durable key".into());
    }
    let parts = tuple
        .as_array()
        .filter(|parts| parts.len() == 5)
        .ok_or("an operation key must have five parts")?;
    if parts[0].as_str() != Some("agent") {
        return Err("only Agent call sites are operations".into());
    }
    let site: Vec<&str> = parts[4]
        .as_array()
        .map(|site| site.iter().filter_map(serde_json::Value::as_str).collect())
        .unwrap_or_default();
    let [agent_id, _capability_id, step_id] = site[..] else {
        return Err("an Agent operation key names its agent, capability and step".into());
    };
    if agent_id.is_empty() || step_id.is_empty() {
        return Err("an Agent operation key names its agent, capability and step".into());
    }
    Ok(OperationIdentity {
        key: key.to_owned(),
        op_hash: operation_hash(key),
        attempt,
        agent_id: agent_id.to_owned(),
        step_id: step_id.to_owned(),
    })
}

/// The operation a call site entered and has not left.
#[derive(Debug, Clone)]
pub(crate) struct EnteredOperation {
    pub(crate) identity: OperationIdentity,
    pub(crate) continuation: Option<Vec<u8>>,
}

const SECOND_ENTER: &str =
    "an operation is already entered; operation-scoped sites run one at a time";

/// Per-run operation-scope state, owned by the workflow store.
#[derive(Debug, Default)]
pub(crate) struct OperationScopeState {
    current: Option<EnteredOperation>,
}

impl OperationScopeState {
    /// The entered operation, if any.
    pub(crate) fn current(&self) -> Option<&EnteredOperation> {
        self.current.as_ref()
    }

    /// Check that no operation is entered and parse the site's identity.
    pub(crate) fn admit(&self, key: &str, attempt: u32) -> Result<OperationIdentity, String> {
        if self.current.is_some() {
            return Err(SECOND_ENTER.into());
        }
        parse_agent_operation_key(key, attempt)
    }

    /// Enter `identity`. Fails closed when another operation got in first
    /// (`admit` and this straddle the continuation load).
    pub(crate) fn enter(
        &mut self,
        identity: OperationIdentity,
        continuation: Option<Vec<u8>>,
    ) -> Result<(), String> {
        if self.current.is_some() {
            return Err(SECOND_ENTER.into());
        }
        self.current = Some(EnteredOperation {
            identity,
            continuation,
        });
        Ok(())
    }

    /// Leave the entered operation, if any.
    pub(crate) fn leave(&mut self) -> Option<EnteredOperation> {
        self.current.take()
    }

    /// Check a suspension of the entered operation against the caps, and
    /// return its identity. No operation registers instance waits, so an
    /// `instances` wake is refused.
    pub(crate) fn check_suspension(
        &self,
        state: &[u8],
        wakes: &[SuspensionWake],
    ) -> Result<OperationIdentity, String> {
        let Some(current) = self.current.as_ref() else {
            return Err("no operation is entered".into());
        };
        let invalid = |error: String| {
            format!(
                "{}: {error}",
                runtara_agent_suspension::AGENT_INVALID_SUSPENSION
            )
        };
        let contract: Vec<_> = wakes.iter().map(SuspensionWake::to_contract).collect();
        runtara_agent_suspension::validate_suspension(&contract, state).map_err(invalid)?;
        if let Some(foreign) = wakes.iter().find_map(|wake| match wake {
            SuspensionWake::Instances(id) => Some(id),
            SuspensionWake::At(_) => None,
        }) {
            return Err(invalid(format!(
                "`{foreign}` is not an instance wait this operation registered; agents cannot register one"
            )));
        }
        Ok(current.identity.clone())
    }

    /// Leave the entered operation as suspended.
    pub(crate) fn suspended(&mut self) {
        self.current = None;
    }
}

/// `op_hash` of a call site: hex sha256 of its canonical checkpoint key.
pub fn operation_hash(checkpoint_key: &str) -> String {
    format!("{:x}", Sha256::digest(checkpoint_key.as_bytes()))
}

/// Store data that can answer `runtara:agent/continuation`.
pub(crate) trait SuspensionContextView {
    /// The continuation of the entered operation, if any.
    fn continuation(&self) -> Option<Vec<u8>>;
}

impl SuspensionContextView for WorkflowState {
    fn continuation(&self) -> Option<Vec<u8>> {
        self.operation
            .current()
            .and_then(|operation| operation.continuation.clone())
    }
}

/// Stores outside a workflow run (dispatcher, trusted and control executor
/// stores) have no operation, so no continuation.
impl SuspensionContextView for crate::host_state::HostState {
    fn continuation(&self) -> Option<Vec<u8>> {
        None
    }
}

/// Bind `runtara:agent/continuation`.
pub(crate) fn add_suspension_context_to_linker<T: SuspensionContextView + Send + 'static>(
    linker: &mut Linker<T>,
) -> anyhow::Result<()> {
    linker
        .instance(runtara_wit::agent::CONTINUATION)?
        .func_wrap("continuation", |store: StoreContextMut<'_, T>, (): ()| {
            Ok((store.data().continuation(),))
        })?;
    Ok(())
}

fn runtime(
    store: &StoreContextMut<'_, WorkflowState>,
) -> Result<std::sync::Arc<dyn crate::runtime_host::RuntimeHost>, String> {
    store.data().runtime_host().cloned().ok_or_else(|| {
        "typed agent suspension needs a runtime host (WorkflowRunSpec.runtime is None)".to_string()
    })
}

/// Bind `runtara:workflow/operation` for workflow stores.
pub(crate) fn add_operation_scope_to_linker(
    linker: &mut Linker<WorkflowState>,
) -> anyhow::Result<()> {
    let mut scope = linker.instance(runtara_wit::workflow::OPERATION)?;
    scope.func_wrap_async(
        "enter",
        |mut store: StoreContextMut<'_, WorkflowState>,
         (key, attempt, load): (String, u32, bool)| {
            Box::new(async move {
                let identity = match store.data().operation.admit(&key, attempt) {
                    Ok(identity) => identity,
                    Err(error) => return Ok((Err(error),)),
                };
                let op_hash = identity.op_hash.clone();
                let continuation = if load {
                    let host = match runtime(&store) {
                        Ok(host) => host,
                        Err(error) => return Ok((Err(error),)),
                    };
                    match host
                        .operation_continuation_load(op_hash.clone(), attempt)
                        .await
                    {
                        Ok(continuation) => continuation,
                        Err(error) => return Ok((Err(error),)),
                    }
                } else {
                    None
                };
                Ok((store.data_mut().operation.enter(identity, continuation),))
            })
        },
    )?;
    scope.func_wrap_async(
        "suspend",
        |mut store: StoreContextMut<'_, WorkflowState>,
         (state, wakes): (Vec<u8>, Vec<SuspensionWake>)| {
            Box::new(async move {
                // A suspension that breaks the caps is the step error
                // `AGENT_INVALID_SUSPENSION`; the operation stays entered so
                // the failed exit discards whatever it kept.
                let identity = match store.data().operation.check_suspension(&state, &wakes) {
                    Ok(identity) => identity,
                    Err(error) => return Ok((Err(error),)),
                };
                // Losing the continuation would re-run the operation from
                // scratch, so a store failure fails the run instead of the step.
                runtime(&store)
                    .map_err(wasmtime::Error::msg)?
                    .operation_continuation_store(identity.op_hash, identity.attempt, state)
                    .await
                    .map_err(|error| {
                        wasmtime::format_err!("storing the operation continuation failed: {error}")
                    })?;
                store.data_mut().operation.suspended();
                Ok((Ok(()),))
            })
        },
    )?;
    scope.func_wrap_async(
        "exit",
        |mut store: StoreContextMut<'_, WorkflowState>, (failed,): (bool,)| {
            Box::new(async move {
                let operation = store.data_mut().operation.leave();
                // A failure is not checkpointed, so a replay or retry starts
                // the operation afresh: its wait is closed (a retry registers
                // a new one) and its continuation goes. Both must be confirmed
                // before the step's error path runs.
                if failed && let Some(operation) = operation {
                    let host = runtime(&store).map_err(wasmtime::Error::msg)?;
                    let op_hash = operation.identity.op_hash;
                    host.operation_wait_close(op_hash.clone())
                        .await
                        .map_err(wasmtime::Error::msg)?;
                    host.operation_release(op_hash)
                        .await
                        .map_err(wasmtime::Error::msg)?;
                }
                Ok(())
            })
        },
    )?;
    scope.func_wrap_async(
        "release",
        |store: StoreContextMut<'_, WorkflowState>, (key,): (String,)| {
            Box::new(async move {
                // The result is already checkpointed, so a failed release only
                // leaves a stale continuation that instance cleanup removes.
                let released = match runtime(&store) {
                    Ok(host) => host.operation_release(operation_hash(&key)).await,
                    Err(error) => Err(error),
                };
                if let Err(error) = released {
                    tracing::warn!(%error, "operation release failed");
                }
                Ok(())
            })
        },
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(workflow: &str, loop_path: serde_json::Value, step: &str) -> String {
        format!(
            "runtara:v2:{}",
            serde_json::json!([
                "agent",
                workflow,
                [],
                loop_path,
                ["control", "cancel", step]
            ])
        )
    }

    #[test]
    fn agent_keys_parse_into_the_frozen_identity() {
        let root = key("wf", serde_json::json!([]), "stop-child");
        let identity = parse_agent_operation_key(&root, 2).unwrap();
        assert_eq!(identity.key, root);
        assert_eq!(identity.op_hash, operation_hash(&root));
        assert_eq!(identity.attempt, 2);
        assert_eq!(identity.agent_id, "control");
        assert_eq!(identity.step_id, "stop-child");
        // Each iteration is its own operation.
        let iteration = key(
            "wf",
            serde_json::json!([["split", "each", 3]]),
            "stop-child",
        );
        assert_ne!(
            parse_agent_operation_key(&iteration, 1).unwrap().op_hash,
            identity.op_hash
        );
    }

    #[test]
    fn anything_but_a_canonical_agent_key_is_refused() {
        let root = key("wf", serde_json::json!([]), "stop-child");
        let delay = format!(
            "runtara:v2:{}",
            serde_json::json!(["delay", "wf", [], [], ["wait"]])
        );
        let spaced = root.replace(",", ", ");
        let refused = [
            String::new(),
            "agent::control::cancel::stop-child".into(),
            delay,
            spaced,
            format!("{root}::attempt::2"),
            format!("{root}::retry_sleep::1"),
            format!("{root}::retry::3"),
            "runtara:v2:[\"agent\",\"wf\",[],[],[\"control\",\"cancel\"]]".into(),
            format!("runtara:v2:{}", "x".repeat(MAX_OPERATION_KEY_BYTES)),
        ];
        for key in refused {
            assert!(parse_agent_operation_key(&key, 1).is_err(), "{key}");
        }
        assert!(parse_agent_operation_key(&root, 0).is_err());
    }

    #[test]
    fn a_second_enter_fails_closed_until_the_first_leaves() {
        let mut state = OperationScopeState::default();
        let first = key("wf", serde_json::json!([]), "a");
        let identity = state.admit(&first, 1).unwrap();
        state.enter(identity.clone(), None).unwrap();
        let second = key("wf", serde_json::json!([]), "b");
        assert_eq!(state.admit(&second, 1).unwrap_err(), SECOND_ENTER);
        // A racing enter that was admitted before the first one landed.
        assert_eq!(
            state.enter(identity.clone(), None).unwrap_err(),
            SECOND_ENTER
        );
        assert_eq!(state.current().unwrap().identity, identity);
        state.leave();
        assert!(state.admit(&second, 1).is_ok());
    }

    #[test]
    fn a_suspension_is_checked_against_the_caps_and_names_no_instance_wait() {
        let mut state = OperationScopeState::default();
        let wakes = [SuspensionWake::At(9)];
        assert!(
            state.check_suspension(b"s", &wakes).is_err(),
            "outside an operation"
        );
        let site = key("wf", serde_json::json!([]), "pause");
        let identity = state.admit(&site, 2).unwrap();
        state.enter(identity.clone(), None).unwrap();

        let invalid = runtara_agent_suspension::AGENT_INVALID_SUSPENSION;
        for (wakes, bytes) in [
            (vec![], 0),
            (
                vec![SuspensionWake::At(1); runtara_agent_suspension::MAX_WAKES + 1],
                0,
            ),
            (
                vec![SuspensionWake::At(1)],
                runtara_agent_suspension::MAX_CONTINUATION_BYTES + 1,
            ),
            // No operation registers an instance wait.
            (vec![SuspensionWake::Instances("w1".into())], 0),
            (
                vec![
                    SuspensionWake::At(1),
                    SuspensionWake::Instances("w1".into()),
                ],
                0,
            ),
        ] {
            let error = state.check_suspension(&vec![0; bytes], &wakes).unwrap_err();
            assert!(error.starts_with(invalid), "{error}");
        }
        assert_eq!(state.check_suspension(b"s", &wakes).unwrap(), identity);
        state.suspended();
        assert!(state.current().is_none());
    }

    /// The hand-written suspension mirrors have the canonical layout the
    /// emitter reads (`runtara_agent_suspension::layout`, pinned against the
    /// WIT by `runtara-agent-suspension`'s tests).
    #[test]
    fn suspension_mirrors_match_the_canonical_layout() {
        use runtara_agent_suspension::layout;
        use wasmtime::component::ComponentType;
        assert_eq!(SuspensionWake::SIZE32, layout::WAKE_SIZE as usize);
        assert_eq!(SuspensionWake::ALIGN32, layout::WAKE_ALIGN);
        assert_eq!(Suspension::SIZE32, layout::SUSPENSION_SIZE as usize);
        assert_eq!(Suspension::ALIGN32, layout::SUSPENSION_ALIGN);
        assert_eq!(SuspendableOutcome::SIZE32, layout::OUTCOME_SIZE as usize);
        assert_eq!(SuspendableOutcome::ALIGN32, layout::OUTCOME_ALIGN);
        assert_eq!(
            <Result<SuspendableOutcome, crate::ErrorInfo> as ComponentType>::ALIGN32,
            layout::INVOKE_RESULT_PAYLOAD_OFFSET
        );
    }

    #[test]
    fn the_operation_hash_is_the_sha256_of_the_checkpoint_key() {
        assert_eq!(
            operation_hash(""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_ne!(operation_hash("a::1"), operation_hash("a::2"));
    }
}
