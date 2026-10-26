// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Operation identities of typed agent suspension.
//!
//! An operation-scoped call site (a suspending capability or a control call,
//! durable or not) enters its operation over the workflow API
//! (`POST /v1/operations`). The host parses the site's canonical v2 Agent
//! checkpoint key into an [`OperationIdentity`] and keys everything by
//! `op_hash = sha256(checkpoint-key)` inside the run's own instance; nothing
//! about the identity comes from agent input.
//!
//! An agent suspension may wake only at a time (`at`). Nothing lets an agent
//! register an instance wait; a run waits on other runs with the
//! `WaitForInstances` step (`instance_wait_host`).
//!
//! Persistence goes through [`crate::runtime_host::RuntimeHost`]; hosts
//! without typed suspension refuse, which fails the step instead of losing
//! state.

use sha2::{Digest, Sha256};

/// The suspension types are the shared `runtara:agent/types` mirrors.
pub use crate::lifecycle::{
    WorkflowOutcome as SuspendableOutcome, WorkflowSuspension as Suspension,
    WorkflowWake as SuspensionWake,
};

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

/// `op_hash` of a call site: hex sha256 of its canonical checkpoint key.
pub fn operation_hash(checkpoint_key: &str) -> String {
    format!("{:x}", Sha256::digest(checkpoint_key.as_bytes()))
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
