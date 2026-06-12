//! Wasmtime engine builder for the runtara component host.
//!
//! One `Engine` per process — it owns the JIT code cache and Cranelift state.
//! Workflow runs and test-dispatcher calls share the same engine, which is the
//! whole point of embedding wasmtime instead of shelling out to the CLI.

use anyhow::Result;
use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::sync::Arc;
use wasmtime::{Cache, CacheConfig, Config, Engine, OptLevel, Strategy};

#[derive(Clone, Debug)]
pub struct EngineConfig {
    /// Enforce per-call deadlines via epoch interruption. Cheaper than fuel
    /// because it has zero cost on the happy path — wasmtime checks an atomic
    /// at branch points instead of decrementing a counter per basic block.
    pub enable_epoch_interruption: bool,
    /// On-disk cache for compiled artifacts, keyed by content hash + compiler
    /// config, so a server restart doesn't re-pay the Cranelift compile for
    /// every deployed image. `None` disables the disk cache (compiles stay
    /// in-memory only, the pre-cache behavior).
    pub cache_dir: Option<PathBuf>,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            enable_epoch_interruption: true,
            cache_dir: default_cache_dir(),
        }
    }
}

/// `<DATA_DIR>/wasmtime-cache`, absolutized because wasmtime rejects relative
/// cache paths. Rooted under the data dir rather than `$HOME` (wasmtime's own
/// default) so containers don't need a writable home.
fn default_cache_dir() -> Option<PathBuf> {
    let data_dir = runtara_dsl::paths::get_data_dir();
    let abs = if data_dir.is_absolute() {
        data_dir
    } else {
        std::env::current_dir().ok()?.join(data_dir)
    };
    Some(abs.join("wasmtime-cache"))
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

    if let Some(dir) = &cfg.cache_dir {
        let mut cache_cfg = CacheConfig::new();
        cache_cfg.with_directory(dir);
        match Cache::new(cache_cfg) {
            Ok(cache) => {
                c.cache(Some(cache));
            }
            // A broken cache (read-only volume, bad mount) must degrade to
            // slower compiles, never block the engine.
            Err(e) => tracing::warn!(
                "wasmtime disk cache disabled, compiles will not persist \
                 across restarts ({}): {e:#}",
                dir.display()
            ),
        }
    }

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

    #[test]
    fn disk_cache_creates_directory() {
        let dir = tempfile::tempdir().expect("tempdir");
        let cache_dir = dir.path().join("wasmtime-cache");
        let cfg = EngineConfig {
            cache_dir: Some(cache_dir.clone()),
            ..EngineConfig::default()
        };
        build_engine(&cfg).expect("build engine with disk cache");
        assert!(cache_dir.is_dir(), "cache directory should be created");
    }

    #[test]
    fn builds_engine_without_disk_cache() {
        let cfg = EngineConfig {
            cache_dir: None,
            ..EngineConfig::default()
        };
        build_engine(&cfg).expect("build engine without cache");
    }
}
