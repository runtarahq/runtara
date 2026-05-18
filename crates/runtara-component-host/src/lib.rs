//! Embedded wasmtime host for runtara agent components.
//!
//! Phase 1 scope (in progress):
//!
//! - `engine` — shared `wasmtime::Engine` builder with component-model on
//!   and epoch-interruption for per-call deadlines.
//! - `host_state` — `WasiView` + `WasiHttpView` impls with defensive
//!   `X-Org-Id` injection on outbound HTTP.
//!
//! Not yet landed (next steps):
//!
//! - `bindings` — `wasmtime::component::bindgen!` against the WIT; needs
//!   WASI WIT deps vendored or remapped to wasmtime-wasi's built-in
//!   bindings.
//! - `registry` — load components from a manifest, pre-instantiate, cache.
//! - `dispatcher` — `ComponentDispatcherService` replacing the legacy
//!   `DispatcherService`.
//!
//! See docs/wasm-components-migration-plan.md § 6.

pub mod bindings;
pub mod engine;
pub mod host_state;
pub mod registry;

pub use engine::{EngineConfig, build_engine, spawn_epoch_ticker};
pub use host_state::{CallContext, HostState};
pub use registry::{LoadedAgent, build_linker, instantiate, load_agent};

/// The canonical WIT source this host is designed against.
pub const AGENT_WIT: &str = runtara_agent_wit::RUNTARA_AGENT_WIT;
