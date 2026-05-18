//! Canonical WIT package for runtara agent components.
//!
//! The artifact of this crate is the `wit/` directory, consumed by:
//! - guest agents via `[package.metadata.component.target] path = "../runtara-agent-wit/wit"`
//! - host code via `wasmtime::component::bindgen!({ path: "wit", world: "agent" })`
//!
//! The Rust surface here is a thin convenience: the canonical WIT source as a
//! `&'static str`, so callers can include or vendor it without a path lookup.

/// Source for the canonical `runtara:agent@0.1.0` WIT package.
pub const RUNTARA_AGENT_WIT: &str = include_str!("../wit/runtara-agent.wit");
