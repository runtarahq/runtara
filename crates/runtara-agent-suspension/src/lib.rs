// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Typed agent suspension contract, `runtara:agent-suspension@0.1.0`.
//!
//! The WIT is the artifact of this crate; the Rust surface is the shared
//! vocabulary guest agents, the compiler and the host agree on: interface
//! names, the per-agent `suspendable` interface text, the size caps, and the
//! guest-side [`Suspendable`] / [`SuspendContext`] types. Natively this crate
//! has no dependencies.

/// WIT package name.
pub const PACKAGE: &str = "runtara:agent-suspension@0.1.0";

/// Component import name of the `types` interface.
pub const TYPES_INTERFACE: &str = "runtara:agent-suspension/types@0.1.0";

/// Component import name of the `context` interface. Only agents that declare
/// a suspending capability may import it.
pub const CONTEXT_INTERFACE: &str = "runtara:agent-suspension/context@0.1.0";

/// WIT source of `runtara:agent-suspension@0.1.0`.
pub const WIT: &str = include_str!("../wit/runtara-agent-suspension.wit");

/// Name of the per-agent interface a suspending agent exports beside
/// `capabilities`, inside its own `runtara:agent-<id>@0.4.0` package.
pub const SUSPENDABLE_INTERFACE: &str = "suspendable";

/// The per-agent `suspendable` interface, exactly as a suspending agent's
/// package declares it. It is type-identical in its flat signature to
/// `capabilities.invoke`; only the success payload differs.
pub const SUSPENDABLE_INTERFACE_WIT: &str = "interface suspendable {
    use runtara:agent/types@0.4.0.{error-info};
    use runtara:agent-suspension/types@0.1.0.{outcome};
    invoke: async func(capability-id: string, input: list<u8>) -> result<outcome, error-info>;
}
";

/// Largest continuation (`suspension.state`) the host keeps per operation.
pub const MAX_CONTINUATION_BYTES: usize = 64 * 1024;

/// Most wakes one suspension may carry.
pub const MAX_WAKES: usize = 16;

/// Longest `instances` wait id, in bytes.
pub const MAX_WAKE_ID_BYTES: usize = 64;

/// A suspending capability was invoked through a path that cannot park, such
/// as the plain `capabilities.invoke` or a test invocation.
pub const SUSPENSION_UNSUPPORTED: &str = "SUSPENSION_UNSUPPORTED";

/// The suspension violated the contract (no wakes, too many, oversized).
pub const AGENT_INVALID_SUSPENSION: &str = "AGENT_INVALID_SUSPENSION";

/// Canonical-ABI layout (wasm32) of the `types` interface: what the direct
/// emitter reads from a `suspendable.invoke` result and what the host's
/// component-type mirrors must match. Pinned against `wit_parser::SizeAlign`
/// by `runtara-workflow-wit`'s tests. Byte sizes and offsets.
pub mod layout {
    /// `wake`: a u8 discriminant, the payload 8-aligned for `at(u64)`.
    pub const WAKE_SIZE: u32 = 16;
    pub const WAKE_ALIGN: u32 = 8;
    pub const WAKE_PAYLOAD_OFFSET: u32 = 8;
    /// `suspension`: `wakes` then `state`, each a pointer and a length.
    pub const SUSPENSION_SIZE: u32 = 16;
    pub const SUSPENSION_ALIGN: u32 = 4;
    pub const SUSPENSION_WAKES_OFFSET: u32 = 0;
    pub const SUSPENSION_STATE_OFFSET: u32 = 8;
    /// `outcome`: a u8 discriminant, then `completed(list<u8>)` or
    /// `suspended(suspension)` at the same payload offset.
    pub const OUTCOME_SIZE: u32 = 20;
    pub const OUTCOME_ALIGN: u32 = 4;
    pub const OUTCOME_PAYLOAD_OFFSET: u32 = 4;
    /// `result<outcome, error-info>` of `suspendable.invoke`: `error-info`
    /// holds an `option<u64>`, so both arms sit 8-aligned after the tag.
    pub const INVOKE_RESULT_PAYLOAD_OFFSET: u32 = 8;
}

/// When the host should re-invoke a suspended capability.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Wake {
    /// Re-invoke at (or after) this wall-clock ms since the Unix epoch.
    At(u64),
    /// Re-invoke when the host-owned instance wait with this id settles.
    Instances(String),
}

/// What a suspending capability returns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Suspendable<T> {
    /// The capability finished with this output.
    Completed(T),
    /// Park, and re-invoke with `state` as the continuation when any wake
    /// fires.
    Suspended { wakes: Vec<Wake>, state: Vec<u8> },
}

impl<T> Suspendable<T> {
    /// Map the completed output, keeping a suspension as is.
    pub fn map<U>(self, f: impl FnOnce(T) -> U) -> Suspendable<U> {
        match self {
            Self::Completed(value) => Suspendable::Completed(f(value)),
            Self::Suspended { wakes, state } => Suspendable::Suspended { wakes, state },
        }
    }
}

/// The host-owned continuation of the operation a suspending capability runs
/// in. Built by generated agent glue, never from workflow input.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SuspendContext {
    continuation: Option<Vec<u8>>,
}

impl SuspendContext {
    /// Context for an invocation with the given saved continuation.
    pub fn new(continuation: Option<Vec<u8>>) -> Self {
        Self { continuation }
    }

    /// The state saved by the operation's last suspension, or `None` on its
    /// first invocation.
    pub fn continuation(&self) -> Option<&[u8]> {
        self.continuation.as_deref()
    }
}

/// Check a suspension against the contract caps. The host enforces the same
/// rules; guests call this to fail with a clear error before returning.
pub fn validate_suspension(wakes: &[Wake], state: &[u8]) -> Result<(), String> {
    if wakes.is_empty() {
        return Err("a suspension needs at least one wake".into());
    }
    if wakes.len() > MAX_WAKES {
        return Err(format!(
            "a suspension carries {} wakes; at most {MAX_WAKES} are allowed",
            wakes.len()
        ));
    }
    for wake in wakes {
        if let Wake::Instances(id) = wake
            && (id.is_empty() || id.len() > MAX_WAKE_ID_BYTES)
        {
            return Err(format!(
                "an instance wait id must be 1..={MAX_WAKE_ID_BYTES} bytes"
            ));
        }
    }
    if state.len() > MAX_CONTINUATION_BYTES {
        return Err(format!(
            "a continuation of {} bytes exceeds {MAX_CONTINUATION_BYTES}",
            state.len()
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use wit_parser::{Resolve, TypeDefKind};

    #[test]
    fn package_parses_with_the_documented_shapes() {
        let mut resolve = Resolve::default();
        let id = resolve.push_str("agent-suspension.wit", WIT).unwrap();
        let package = &resolve.packages[id];
        assert_eq!(package.name.to_string(), PACKAGE);
        let types = &resolve.interfaces[package.interfaces["types"]];
        let TypeDefKind::Variant(wake) = &resolve.types[types.types["wake"]].kind else {
            panic!("wake must be a variant");
        };
        assert_eq!(
            wake.cases
                .iter()
                .map(|c| c.name.as_str())
                .collect::<Vec<_>>(),
            ["at", "instances"]
        );
        let TypeDefKind::Record(suspension) = &resolve.types[types.types["suspension"]].kind else {
            panic!("suspension must be a record");
        };
        assert_eq!(
            suspension
                .fields
                .iter()
                .map(|f| f.name.as_str())
                .collect::<Vec<_>>(),
            ["wakes", "state"]
        );
        let TypeDefKind::Variant(outcome) = &resolve.types[types.types["outcome"]].kind else {
            panic!("outcome must be a variant");
        };
        assert_eq!(
            outcome
                .cases
                .iter()
                .map(|c| c.name.as_str())
                .collect::<Vec<_>>(),
            ["completed", "suspended"]
        );
        let context = &resolve.interfaces[package.interfaces["context"]];
        assert!(matches!(
            context.functions["continuation"].kind,
            wit_parser::FunctionKind::Freestanding
        ));
    }

    #[test]
    fn suspendable_interface_text_is_a_valid_agent_interface() {
        let mut resolve = Resolve::default();
        resolve
            .push_str(
                "agent.wit",
                "package runtara:agent@0.4.0;\ninterface types {\n record error-info { code: string, message: string, category: string, severity: string, retryable: bool, retry-after-ms: option<u64>, attributes: option<string> }\n}\n",
            )
            .unwrap();
        resolve.push_str("agent-suspension.wit", WIT).unwrap();
        let id = resolve
            .push_str(
                "probe.wit",
                &format!("package runtara:agent-probe@0.4.0;\n{SUSPENDABLE_INTERFACE_WIT}"),
            )
            .unwrap();
        let interface = &resolve.interfaces[resolve.packages[id].interfaces[SUSPENDABLE_INTERFACE]];
        assert!(matches!(
            interface.functions["invoke"].kind,
            wit_parser::FunctionKind::AsyncFreestanding
        ));
    }

    #[test]
    fn suspensions_are_validated_against_the_caps() {
        assert!(validate_suspension(&[], b"").is_err());
        assert!(validate_suspension(&[Wake::At(1)], b"state").is_ok());
        assert!(validate_suspension(&vec![Wake::At(1); MAX_WAKES + 1], b"").is_err());
        assert!(validate_suspension(&[Wake::Instances(String::new())], b"").is_err());
        assert!(
            validate_suspension(&[Wake::Instances("w".repeat(MAX_WAKE_ID_BYTES + 1))], b"")
                .is_err()
        );
        assert!(validate_suspension(&[Wake::At(1)], &vec![0; MAX_CONTINUATION_BYTES + 1]).is_err());
        assert!(validate_suspension(&[Wake::At(1)], &vec![0; MAX_CONTINUATION_BYTES]).is_ok());
    }
}
