//! Spike S1 (docs/wasip3-parallelism.md §5 Phase 1): stackful async lift,
//! hand-emitted with the production wasm-tools pins, running on wasmtime 46.
//!
//! Exit criteria checked here:
//!   1. Components emitted with wasm-encoder/wit-component 0.247 validate on
//!      wasmtime 46's post-1.249 validator.
//!   2. The textual-wac pipeline (wac-parser/-resolver/-graph 0.10, the exact
//!      production composition path) round-trips async-typed worlds.
//!   3. Real overlap: `run-both` (two async-lowered subtasks joined in one
//!      waitable-set from a stackful-lifted export) completes in ~max(a,b),
//!      the `run-seq` baseline in ~a+b.
//!   4. Epoch interruption stays enabled the whole time (no spurious traps).

mod emit;
mod wit;

use std::collections::HashMap;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use wasmtime::component::{Component, Linker};
use wasmtime::{Config, Engine, Store};

fn compose(dir: &std::path::Path) -> Result<Vec<u8>> {
    use wac_graph::EncodeOptions;
    use wac_parser::Document;
    use wac_resolver::{FileSystemPackageResolver, packages};

    let document = Document::parse(wit::COMPOSE_WAC).context("parse wac")?;
    let keys = packages(&document).context("collect wac packages")?;
    let overrides: HashMap<String, std::path::PathBuf> = [
        ("demo:plugin-a", "demo-plugin-a.wasm"),
        ("demo:plugin-b", "demo-plugin-b.wasm"),
        ("demo:orchestrator", "demo-orchestrator.wasm"),
    ]
    .into_iter()
    .map(|(pkg, file)| (pkg.to_string(), dir.join(file)))
    .collect();
    let resolver = FileSystemPackageResolver::new(dir, overrides, false);
    let resolved = resolver.resolve(&keys).context("resolve wac packages")?;
    let resolution = document.resolve(resolved).context("resolve wac document")?;
    resolution
        .encode(EncodeOptions {
            define_components: true,
            validate: true,
            ..Default::default()
        })
        .context("encode composed component")
}

struct Ctx;

#[tokio::main]
async fn main() -> Result<()> {
    // ── Emit the three components with the production pins ────────────────
    let dir = tempfile::tempdir()?;
    std::fs::write(dir.path().join("demo-plugin-a.wasm"), emit::plugin_a()?)?;
    std::fs::write(dir.path().join("demo-plugin-b.wasm"), emit::plugin_b()?)?;
    std::fs::write(
        dir.path().join("demo-orchestrator.wasm"),
        emit::orchestrator()?,
    )?;
    println!("[emit] three components emitted (wasm-encoder/wit-component 0.247)");

    // ── Compose through the production textual-wac pipeline ───────────────
    let composed = compose(dir.path())?;
    println!("[wac] composed + wac-validated ({} bytes)", composed.len());

    // ── wasmtime 46: CM-async config, epoch interruption ON ───────────────
    let mut config = Config::new();
    config.wasm_component_model(true);
    config.wasm_component_model_async(true);
    config.wasm_component_model_more_async_builtins(true);
    config.wasm_component_model_async_stackful(true);
    config.epoch_interruption(true);
    let engine = Engine::new(&config)?;

    // Epoch ticker: mirrors the production embedded runner's interruption
    // ring so the spike proves CM-async and epoch deadlines coexist.
    {
        let engine = engine.clone();
        std::thread::spawn(move || {
            loop {
                std::thread::sleep(Duration::from_millis(100));
                engine.increment_epoch();
            }
        });
    }

    let component = Component::new(&engine, &composed)
        .map_err(|e| anyhow::anyhow!("wasmtime 46 rejected composed bytes: {e:?}"))?;
    println!("[wasmtime] composed component validated + compiled on 46");

    let mut linker: Linker<Ctx> = Linker::new(&engine);
    let mut env = linker.instance("demo:host/env@0.1.0")?;
    env.func_wrap_concurrent("sleep", |_accessor, (ms,): (u64,)| {
        Box::pin(async move {
            tokio::time::sleep(Duration::from_millis(ms)).await;
            Ok(())
        })
    })?;

    let mut store = Store::new(&engine, Ctx);
    store.set_epoch_deadline(10);
    store.epoch_deadline_callback(|_| Ok(wasmtime::UpdateDeadline::Yield(10)));

    let instance = linker.instantiate_async(&mut store, &component).await?;
    let runner_idx = instance
        .get_export_index(&mut store, None, "demo:app/runner@0.1.0")
        .context("runner interface export missing")?;
    let get = |store: &mut Store<Ctx>, name: &str| -> Result<wasmtime::component::Func> {
        let idx = instance
            .get_export_index(&mut *store, Some(&runner_idx), name)
            .with_context(|| format!("{name} export missing"))?;
        instance
            .get_func(&mut *store, idx)
            .with_context(|| format!("{name} not a func"))
    };
    let run_both = get(&mut store, "run-both")?;
    let run_seq = get(&mut store, "run-seq")?;

    async fn call(
        store: &mut Store<Ctx>,
        func: wasmtime::component::Func,
        ms: u64,
    ) -> Result<(u64, Duration)> {
        let typed = func.typed::<(u64,), (u64,)>(&mut *store)?;
        let start = Instant::now();
        let (result,) = typed.call_async(&mut *store, (ms,)).await?;
        Ok((result, start.elapsed()))
    }

    // Warm-up (Cranelift JIT paths, lazy init) so timings are honest.
    let _ = call(&mut store, run_seq, 5).await?;

    const MS: u64 = 150;
    let (seq_sum, seq_elapsed) = call(&mut store, run_seq, MS).await?;
    println!("[run-seq ] sum={seq_sum} wall={}ms", seq_elapsed.as_millis());
    let (both_sum, both_elapsed) = call(&mut store, run_both, MS).await?;
    println!("[run-both] sum={both_sum} wall={}ms", both_elapsed.as_millis());

    // ── Assertions ─────────────────────────────────────────────────────────
    if seq_sum != 2 * MS || both_sum != 2 * MS {
        bail!("wrong results: seq={seq_sum} both={both_sum}, expected {}", 2 * MS);
    }
    let seq_ms = seq_elapsed.as_millis() as u64;
    let both_ms = both_elapsed.as_millis() as u64;
    if seq_ms < 2 * MS - 20 {
        bail!("run-seq finished implausibly fast ({seq_ms}ms < {}ms)", 2 * MS);
    }
    // Overlap: both plugins sleep MS concurrently → strictly less than the
    // sequential 2*MS, with generous slack for scheduling noise.
    if both_ms >= 2 * MS - 30 {
        bail!("NO OVERLAP: run-both took {both_ms}ms (sequential is ~{}ms)", 2 * MS);
    }
    println!(
        "\nPASS: overlap proven — run-both {both_ms}ms vs run-seq {seq_ms}ms (target ~{MS}ms vs ~{}ms)",
        2 * MS
    );
    Ok(())
}
