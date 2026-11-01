//! Per-logic-instance stdlib state.
//!
//! One stdlib instance serves every logic instance in a run (the root and each
//! published workflow-agent), and each names its context on every call. The
//! current context's manifest, interning arena and handle nonce live in
//! [`MANIFEST`] and the `direct_json` thread-locals for the length of a call;
//! the others wait in [`CONTEXTS`].

// Only the wasm component calls into contexts; the tests below exercise them
// natively.
#![cfg_attr(
    not(all(target_arch = "wasm32", feature = "direct-component")),
    allow(dead_code)
)]

use std::cell::RefCell;

use crate::direct_json::{self, DirectJsonManifest, ValueStoreSlot};

thread_local! {
    /// The current context's manifest.
    pub(crate) static MANIFEST: RefCell<Option<DirectJsonManifest>> = const { RefCell::new(None) };
    static CONTEXTS: RefCell<Vec<Context>> = const { RefCell::new(Vec::new()) };
}

#[derive(Default)]
struct Context {
    manifest: Option<DirectJsonManifest>,
    values: ValueStoreSlot,
}

impl Context {
    /// Exchange this context with the current one. Swapping in and swapping
    /// out are the same exchange.
    fn swap(&mut self) {
        MANIFEST.with(|slot| std::mem::swap(&mut *slot.borrow_mut(), &mut self.manifest));
        direct_json::swap_value_store(&mut self.values);
    }
}

/// The context a logic instance starts with: `prior`, reused, when it names
/// one, else a new one.
pub(crate) fn open(prior: u32) -> u32 {
    CONTEXTS.with(|contexts| {
        let mut contexts = contexts.borrow_mut();
        if (prior as usize) < contexts.len() {
            return prior;
        }
        contexts.push(Context::default());
        (contexts.len() - 1) as u32
    })
}

/// Run `call` with `ctx` current. `None` when no such context exists. Stdlib
/// calls never call out, so they never nest.
pub(crate) fn run<R>(ctx: u32, call: impl FnOnce() -> R) -> Option<R> {
    let exchange = |ctx: u32| {
        CONTEXTS.with(|contexts| {
            contexts
                .borrow_mut()
                .get_mut(ctx as usize)
                .map(Context::swap)
                .is_some()
        })
    };
    if !exchange(ctx) {
        return None;
    }
    let result = call();
    exchange(ctx);
    Some(result)
}

pub(crate) fn unknown(ctx: u32) -> String {
    format!("unknown stdlib context {ctx}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn manifest() -> Vec<u8> {
        serde_json::to_vec(&json!({ "graph": { "steps": [] } })).unwrap()
    }

    fn init(prior: u32) -> u32 {
        let id = open(prior);
        run(id, || {
            direct_json::reset_value_store();
            let manifest = DirectJsonManifest::parse(&manifest()).expect("manifest");
            MANIFEST.with(|slot| *slot.borrow_mut() = Some(manifest));
        })
        .expect("context");
        id
    }

    /// A value big enough to be interned, so it travels as a handle.
    fn intern_large(tag: &str) -> Value {
        let source = direct_json::build_source(
            serde_json::to_vec(&json!({ "big": tag.repeat(20 * 1024) }))
                .unwrap()
                .as_slice(),
            b"{}",
            b"{}",
        )
        .expect("source");
        serde_json::from_slice(&source).unwrap()
    }

    #[test]
    fn contexts_keep_their_own_manifest_and_values() {
        let a = init(u32::MAX);
        let b = init(u32::MAX);
        assert_ne!(a, b);

        let in_a = run(a, || intern_large("a")).unwrap();
        let in_b = run(b, || intern_large("b")).unwrap();
        // Interleaved: each context resolves its own handle.
        let back_a = run(a, || direct_json::materialize(in_a.clone())).unwrap();
        let back_b = run(b, || direct_json::materialize(in_b.clone())).unwrap();
        assert_eq!(back_a["data"]["big"], json!("a".repeat(20 * 1024)));
        assert_eq!(back_b["data"]["big"], json!("b".repeat(20 * 1024)));

        // A handle from one context is plain data in another: each context
        // has its own nonce.
        let crossed = run(b, || direct_json::materialize(in_a.clone())).unwrap();
        assert_eq!(crossed, in_a);
    }

    #[test]
    fn prior_context_is_reset_and_reused() {
        let first = init(u32::MAX);
        let handle = run(first, || intern_large("a")).unwrap();
        let again = init(first);
        assert_eq!(again, first);
        // The reset rotated the nonce: the old handle is plain data now.
        let after = run(again, || direct_json::materialize(handle.clone())).unwrap();
        assert_eq!(after, handle);
    }

    #[test]
    fn an_unknown_context_runs_nothing() {
        assert_eq!(run(u32::MAX - 1, || 1), None);
    }
}
