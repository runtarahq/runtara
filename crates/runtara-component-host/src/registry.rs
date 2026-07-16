//! Minimal `ComponentRegistry` — loads agent components from disk and holds
//! pre-instantiated handles for fast per-call instantiation.
//!
//! As of Phase 3.5 (per-agent WIT packages) the dispatcher no longer goes
//! through the bindgen-generated `Agent` wrapper at invoke time: each agent
//! now exports its `capabilities` interface under its own package
//! (`runtara:agent-crypto/capabilities@0.3.0`, etc.), so the host has to find
//! the export by name dynamically. We introspect the component at load time,
//! record the `capabilities` interface name, and resolve `invoke` against
//! that name at each invocation.

use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result, anyhow};
use wasmtime::Engine;
use wasmtime::component::types::ComponentItem;
use wasmtime::component::{Component, InstancePre, Linker};

use crate::host_state::HostState;

/// One agent component loaded into the engine.
pub struct LoadedAgent {
    pub agent_id: String,
    pub pre: InstancePre<HostState>,
    /// Fully-qualified name of this component's `capabilities` interface
    /// export — e.g. `"runtara:agent-crypto/capabilities@0.3.0"` for a
    /// per-agent WIT package, or `"runtara:agent/capabilities@0.3.0"` for
    /// the legacy shared-WIT layout. Cached at load time so dispatch can
    /// `instance.get_export_index(None, &name)` straight away.
    pub capabilities_iface: String,
}

/// Build a shared `Linker` configured to satisfy every WASI import an agent
/// component might pull in via its own guest world — `wasi:cli/command`
/// (env, stdio, clocks, random, filesystem, sockets) and `wasi:http/proxy`
/// for outbound HTTP.
pub fn build_linker(engine: &Engine) -> Result<Linker<HostState>> {
    let mut linker = Linker::<HostState>::new(engine);
    wasmtime_wasi::p2::add_to_linker_async(&mut linker)?;
    // `add_only_http_to_linker_async` is the slim version that skips
    // re-adding wasi:io (which wasi::p2::add_to_linker_async already added).
    wasmtime_wasi_http::p2::add_only_http_to_linker_async(&mut linker)?;
    Ok(linker)
}

/// Load a single agent component from a `.wasm` path. Returns a `LoadedAgent`
/// ready for fast per-call `instantiate_async`, with the `capabilities`
/// interface name resolved from the component's exports.
pub fn load_agent(
    engine: &Engine,
    linker: &Linker<HostState>,
    wasm_path: impl AsRef<Path>,
    agent_id: impl Into<String>,
) -> Result<Arc<LoadedAgent>> {
    let wasm_path = wasm_path.as_ref();
    let agent_id = agent_id.into();
    let component = Component::from_file(engine, wasm_path)?;
    let capabilities_iface = find_capabilities_iface(engine, &component)
        .with_context(|| format!("agent `{agent_id}` at {}", wasm_path.display()))?;
    let pre = linker.instantiate_pre(&component)?;
    Ok(Arc::new(LoadedAgent {
        agent_id,
        pre,
        capabilities_iface,
    }))
}

/// Walk the component's exports and return the name of the first one whose
/// path matches `<namespace>:<package>/capabilities@<version>`. Agent
/// components only ever export one such interface; if more than one shows
/// up the first wins (deterministic via the iteration order wasmtime
/// returns).
fn find_capabilities_iface(engine: &Engine, component: &Component) -> Result<String> {
    let ty = component.component_type();
    for (name, item) in ty.exports(engine) {
        if !matches!(item.ty, ComponentItem::ComponentInstance(_)) {
            continue;
        }
        if is_capabilities_iface_name(name) {
            return Ok(name.to_string());
        }
    }
    Err(anyhow!(
        "component does not export an interface matching `*/capabilities@*` — \
         agents must export `runtara:agent-<id>/capabilities@<version>` (per-agent WIT) \
         or the legacy `runtara:agent/capabilities@<version>`"
    ))
}

fn is_capabilities_iface_name(name: &str) -> bool {
    // Shape: `<ns>:<pkg>/capabilities@<version>`. The `/capabilities@` substring
    // is unique enough to identify our interface across both the legacy and
    // per-agent WIT layouts.
    name.contains("/capabilities@")
}

/// A "no effective deadline" epoch-delta for stores that don't want a per-call
/// timeout. `Store::set_epoch_deadline(delta)` sets the deadline to
/// `current_epoch + delta`, so passing `u64::MAX` overflows once an epoch
/// ticker has advanced `current_epoch` past 0. This delta is astronomically far
/// in the future (~thousands of years at the ticker's 100 ms cadence) yet far
/// enough below `u64::MAX` that adding it to a live epoch can never overflow.
const NO_DEADLINE_EPOCH_DELTA: u64 = 1 << 40;

/// Instantiate the agent in a fresh `Store` and return the raw component
/// `Instance`. The store's memory/table caps (`HostState::limiter`) are
/// installed before instantiation so instantiation-time growth is bounded too;
/// set them on the `HostState` via `HostState::set_limits` beforehand.
///
/// The store starts with no effective epoch deadline. Callers that want a
/// per-call timeout should install an epoch deadline callback and override the
/// deadline (see `dispatcher::call_with_guards`) after this returns, and the
/// engine must have an epoch ticker running.
pub async fn instantiate(
    engine: &Engine,
    pre: &InstancePre<HostState>,
    state: HostState,
) -> Result<(wasmtime::Store<HostState>, wasmtime::component::Instance)> {
    let mut store = wasmtime::Store::new(engine, state);
    store.limiter(|s| &mut s.limiter);
    store.set_epoch_deadline(NO_DEADLINE_EPOCH_DELTA);
    let instance = pre.instantiate_async(&mut store).await?;
    Ok((store, instance))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capabilities_iface_name_matches_both_layouts() {
        assert!(is_capabilities_iface_name(
            "runtara:agent/capabilities@0.3.0"
        ));
        assert!(is_capabilities_iface_name(
            "runtara:agent-crypto/capabilities@0.3.0"
        ));
        assert!(is_capabilities_iface_name(
            "runtara:agent-object-model/capabilities@5.0.0"
        ));
        assert!(!is_capabilities_iface_name("runtara:agent/types@0.3.0"));
        assert!(!is_capabilities_iface_name("wasi:cli/run@0.2.0"));
    }
}
