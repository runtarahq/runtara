//! Wasmtime engine builder for the runtara component host.
//!
//! One `Engine` per process — it owns the JIT code cache and Cranelift state.
//! Workflow runs and test-dispatcher calls share the same engine, which is the
//! whole point of embedding wasmtime instead of shelling out to the CLI.

use anyhow::Result;
use std::num::NonZeroUsize;
use std::sync::Arc;
use wasmtime::{Config, Engine, OptLevel, Strategy};

#[derive(Clone, Debug)]
pub struct EngineConfig {
    /// Enforce per-call deadlines via epoch interruption. Cheaper than fuel
    /// because it has zero cost on the happy path — wasmtime checks an atomic
    /// at branch points instead of decrementing a counter per basic block.
    pub enable_epoch_interruption: bool,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            enable_epoch_interruption: true,
        }
    }
}

/// Build the shared wasmtime engine.
///
/// Returns an `Arc<Engine>` because the engine is cloned cheaply and shared
/// across the registry, the dispatcher, and (eventually) the workflow runner.
pub fn build_engine(cfg: &EngineConfig) -> Result<Arc<Engine>> {
    let mut c = Config::new();
    c.wasm_component_model(true);
    c.async_stack_size(2 * 1024 * 1024);
    c.consume_fuel(false);
    c.epoch_interruption(cfg.enable_epoch_interruption);
    c.cranelift_opt_level(OptLevel::Speed);
    c.strategy(Strategy::Cranelift);
    c.parallel_compilation(true);
    c.wasm_backtrace_max_frames(NonZeroUsize::new(64));

    Ok(Arc::new(Engine::new(&c)?))
}

/// Duration of one epoch tick as driven by [`spawn_epoch_ticker`]. A deadline
/// of N ticks is "N × `EPOCH_TICK`" of wall-clock budget.
pub const EPOCH_TICK: std::time::Duration = std::time::Duration::from_millis(100);

/// Spawn the epoch ticker for the given engine. Ticks every [`EPOCH_TICK`];
/// call once per engine at process startup.
pub fn spawn_epoch_ticker(engine: Arc<Engine>) {
    std::thread::Builder::new()
        .name("runtara-wasmtime-epoch".into())
        .spawn(move || {
            loop {
                std::thread::sleep(EPOCH_TICK);
                engine.increment_epoch();
            }
        })
        .expect("spawn wasmtime epoch ticker");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_engine_with_component_model() {
        let engine = build_engine(&EngineConfig::default()).expect("build engine");
        // If we got here, wasm_component_model + async_support coexist fine.
        assert!(Arc::strong_count(&engine) == 1);
    }
}
