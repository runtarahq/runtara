// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Typed agent suspension: the Rust side of `runtara:agent/suspension`.
//!
//! The WIT lives in `runtara-wit`; this crate is the shared vocabulary guest
//! agents, the compiler and the host agree on: the size caps, error codes,
//! canonical layout, and the guest-side [`Suspendable`] / [`SuspendContext`]
//! types. Natively this crate has no dependencies.

/// Name of the per-agent interface a suspending agent exports beside
/// `capabilities`, inside its own `runtara:agent-<id>` package.
pub const SUSPENDABLE_INTERFACE: &str = "suspendable";

/// Largest continuation (`suspension.state`) the host keeps per operation.
pub const MAX_CONTINUATION_BYTES: usize = 64 * 1024;

/// Most wakes one suspension may carry.
pub const MAX_WAKES: usize = 16;

/// Longest `instances` wait id, in bytes.
pub const MAX_WAKE_ID_BYTES: usize = 64;

/// A suspending capability was invoked through a path that cannot park, such
/// as the plain `capabilities.invoke` or a test invocation.
pub const SUSPENSION_UNSUPPORTED: &str = "SUSPENSION_UNSUPPORTED";

/// The suspension violated the contract (no wakes, too many, oversized, or
/// an instance wake its operation did not register). The host refuses it and
/// the step fails with this code instead of parking.
pub const AGENT_INVALID_SUSPENSION: &str = "AGENT_INVALID_SUSPENSION";

/// A suspending capability refused the continuation it was handed (for
/// example a version it does not read). The step fails; its failed exit
/// discards the continuation, so a retry starts the operation afresh.
pub const AGENT_CONTINUATION_REJECTED: &str = "AGENT_CONTINUATION_REJECTED";

/// A capability returned a suspension through a path that cannot park: a
/// non-suspending capability, or the plain `capabilities.invoke` export.
pub const AGENT_UNEXPECTED_SUSPEND: &str = "AGENT_UNEXPECTED_SUSPEND";

/// Canonical-ABI layout (wasm32) of the `types` interface: what the direct
/// emitter reads from a `suspendable.invoke` result and what the host's
/// component-type mirrors must match. Pinned against `wit_parser::SizeAlign`
/// by this crate's tests. Byte sizes and offsets.
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

    fn resolve() -> Resolve {
        runtara_wit::resolve().expect("runtara WIT resolves")
    }

    fn suspension_types(resolve: &Resolve) -> &wit_parser::Interface {
        let id = resolve
            .interfaces
            .iter()
            .find(|(id, _)| resolve.id_of(*id).as_deref() == Some(runtara_wit::agent::SUSPENSION))
            .map(|(id, _)| id)
            .expect("runtara:agent/suspension");
        &resolve.interfaces[id]
    }

    #[test]
    fn the_suspension_types_have_the_documented_shapes() {
        let resolve = resolve();
        let types = suspension_types(&resolve);
        let cases = |name: &str| -> Vec<String> {
            match &resolve.types[types.types[name]].kind {
                TypeDefKind::Variant(variant) => {
                    variant.cases.iter().map(|c| c.name.clone()).collect()
                }
                TypeDefKind::Record(record) => {
                    record.fields.iter().map(|f| f.name.clone()).collect()
                }
                other => panic!("{name}: unexpected {other:?}"),
            }
        };
        assert_eq!(cases("wake"), ["at", "instances"]);
        assert_eq!(cases("suspension"), ["wakes", "state"]);
        assert_eq!(cases("outcome"), ["completed", "suspended"]);
    }

    #[test]
    fn the_generated_suspending_agent_declares_the_documented_interface() {
        let mut resolve = resolve();
        let shape = runtara_wit::AgentShape {
            suspendable: true,
            ..Default::default()
        };
        let id = resolve
            .push_str("probe.wit", &runtara_wit::agent_package("probe", shape))
            .unwrap();
        let package = &resolve.packages[id];
        let interface = &resolve.interfaces[package.interfaces[SUSPENDABLE_INTERFACE]];
        assert!(matches!(
            interface.functions["invoke"].kind,
            wit_parser::FunctionKind::AsyncFreestanding
        ));
        let world = &resolve.worlds[package.worlds["agent"]];
        assert!(
            world
                .imports
                .keys()
                .any(|key| resolve.name_world_key(key) == runtara_wit::agent::CONTINUATION)
        );
        assert!(world.exports.keys().any(|key| {
            resolve.name_world_key(key)
                == format!(
                    "runtara:agent-probe/{SUSPENDABLE_INTERFACE}@{}",
                    runtara_wit::VERSION
                )
        }));
    }

    /// The canonical layout of the suspension types equals the constants the
    /// emitter reads and the host mirrors ([`layout`]).
    #[test]
    fn layout_constants_match_the_wit_size_align() {
        use wit_parser::{Int, SizeAlign, Type};

        let mut resolve = resolve();
        let shape = runtara_wit::AgentShape {
            suspendable: true,
            ..Default::default()
        };
        let id = resolve
            .push_str(
                "probe.wit",
                &runtara_wit::agent_package("layout-probe", shape),
            )
            .unwrap();
        let mut sizes = SizeAlign::default();
        sizes.fill(&resolve);
        let bytes = |size: wit_parser::ArchitectureSize| size.size_wasm32() as u32;
        let alignment = |ty: &Type| match sizes.align(ty) {
            wit_parser::Alignment::Bytes(bytes) => bytes.get() as u32,
            wit_parser::Alignment::Pointer => 4,
        };
        let types = suspension_types(&resolve);
        let ty = |name: &str| Type::Id(types.types[name]);
        let payload_offset = |name: &str| {
            let TypeDefKind::Variant(variant) = &resolve.types[types.types[name]].kind else {
                panic!("{name} must be a variant");
            };
            bytes(sizes.payload_offset(
                variant.tag(),
                variant.cases.iter().map(|case| case.ty.as_ref()),
            ))
        };

        assert_eq!(bytes(sizes.size(&ty("wake"))), layout::WAKE_SIZE);
        assert_eq!(alignment(&ty("wake")), layout::WAKE_ALIGN);
        assert_eq!(payload_offset("wake"), layout::WAKE_PAYLOAD_OFFSET);

        assert_eq!(
            bytes(sizes.size(&ty("suspension"))),
            layout::SUSPENSION_SIZE
        );
        assert_eq!(alignment(&ty("suspension")), layout::SUSPENSION_ALIGN);
        let TypeDefKind::Record(suspension) = &resolve.types[types.types["suspension"]].kind else {
            panic!("suspension must be a record");
        };
        let offsets: Vec<u32> = sizes
            .field_offsets(suspension.fields.iter().map(|field| &field.ty))
            .into_iter()
            .map(|(offset, _)| bytes(offset))
            .collect();
        assert_eq!(
            offsets,
            [
                layout::SUSPENSION_WAKES_OFFSET,
                layout::SUSPENSION_STATE_OFFSET
            ]
        );

        assert_eq!(bytes(sizes.size(&ty("outcome"))), layout::OUTCOME_SIZE);
        assert_eq!(alignment(&ty("outcome")), layout::OUTCOME_ALIGN);
        assert_eq!(payload_offset("outcome"), layout::OUTCOME_PAYLOAD_OFFSET);

        let package = &resolve.packages[id];
        let suspendable = &resolve.interfaces[package.interfaces[SUSPENDABLE_INTERFACE]];
        let Some(Type::Id(result)) = suspendable.functions["invoke"].result else {
            panic!("suspendable.invoke returns a result");
        };
        let TypeDefKind::Result(result) = &resolve.types[result].kind else {
            panic!("suspendable.invoke returns a result");
        };
        assert_eq!(
            bytes(sizes.payload_offset(Int::U8, [result.ok.as_ref(), result.err.as_ref()])),
            layout::INVOKE_RESULT_PAYLOAD_OFFSET
        );
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
