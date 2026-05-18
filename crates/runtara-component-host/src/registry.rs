//! Minimal `ComponentRegistry` — loads agent components from disk and holds
//! pre-instantiated handles for fast per-call instantiation.
//!
//! Phase 1 scope is a single-component load + invoke; the full registry
//! (manifest-driven, multi-tenant, hot-reloadable) lands later.

use std::path::Path;
use std::sync::Arc;

use anyhow::Result;
use wasmtime::Engine;
use wasmtime::component::{Component, Linker};

use crate::bindings::{Agent, AgentPre};
use crate::host_state::HostState;

/// One agent component loaded into the engine.
pub struct LoadedAgent {
    pub agent_id: String,
    pub pre: AgentPre<HostState>,
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

/// Load a single agent component from a `.wasm` path. Returns an `AgentPre`
/// wrapper ready for fast per-call `instantiate_async`.
pub fn load_agent(
    engine: &Engine,
    linker: &Linker<HostState>,
    wasm_path: impl AsRef<Path>,
    agent_id: impl Into<String>,
) -> Result<Arc<LoadedAgent>> {
    let wasm_path = wasm_path.as_ref();
    let component = Component::from_file(engine, wasm_path)?;
    let instance_pre = linker.instantiate_pre(&component)?;
    let pre = AgentPre::new(instance_pre)?;
    Ok(Arc::new(LoadedAgent {
        agent_id: agent_id.into(),
        pre,
    }))
}

/// Instantiate the agent in a fresh `Store` and return the typed wrapper.
///
/// The store starts with `epoch_deadline = u64::MAX` (no deadline). Callers
/// that want per-call timeouts should override via `store.set_epoch_deadline`
/// after this returns.
pub async fn instantiate(
    engine: &Engine,
    pre: &AgentPre<HostState>,
    state: HostState,
) -> Result<(wasmtime::Store<HostState>, Agent)> {
    let mut store = wasmtime::Store::new(engine, state);
    store.set_epoch_deadline(u64::MAX);
    let agent = pre.instantiate_async(&mut store).await?;
    Ok((store, agent))
}
