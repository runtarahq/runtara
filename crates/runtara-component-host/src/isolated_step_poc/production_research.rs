//! Production-design experiments, not a production scheduler or a performance gate.

use super::*;
use crate::registry::{LoadedAgent, build_linker, instantiate, load_agent};
use crate::{CallContext, ErrorInfo, HostState};
use std::path::PathBuf;
use std::time::Instant;
use wasmtime::component::{Instance, TypedFunc};

type AgentInvoke = TypedFunc<(String, Vec<u8>), (Result<Vec<u8>, ErrorInfo>,)>;

fn context() -> Arc<CallContext> {
    Arc::new(CallContext::for_test(
        "research",
        "http://127.0.0.1:1",
        "",
        "",
        "",
    ))
}

fn agent_path(agent: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/wasm32-wasip2/release")
        .join(format!("runtara_agent_{agent}.wasm"))
}

fn invoke_func(
    store: &mut Store<HostState>,
    instance: &Instance,
    agent: &LoadedAgent,
) -> Result<AgentInvoke> {
    let interface = instance
        .get_export_index(&mut *store, None, &agent.capabilities_iface)
        .context("capabilities")?;
    let index = instance
        .get_export_index(&mut *store, Some(&interface), "invoke")
        .context("invoke")?;
    Ok(instance.get_typed_func(&mut *store, index)?)
}

async fn invoke(
    store: &mut Store<HostState>,
    func: AgentInvoke,
    capability: &str,
    input: &[u8],
) -> Result<Vec<u8>> {
    let (out,) = func
        .call_async(store, (capability.into(), input.to_vec()))
        .await?;
    out.map_err(|error| anyhow!("{}: {}", error.code, error.message))
}

fn stats(mut samples: Vec<f64>) -> serde_json::Value {
    samples.sort_by(f64::total_cmp);
    let at = |q: f64| samples[((samples.len() - 1) as f64 * q).ceil() as usize];
    serde_json::json!({"samples":samples.len(), "p50_us":at(0.5), "p95_us":at(0.95), "min_us":samples[0], "max_us":samples[samples.len()-1]})
}

fn rss_kib() -> Option<u64> {
    let output = std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()
        .ok()?;
    String::from_utf8(output.stdout).ok()?.trim().parse().ok()
}

/// Manual because timing/RSS depend on the host and the staged release guests.
/// No performance threshold is asserted; all functional outputs are checked.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "manual release-profile research; needs the staged real agent bundle"]
async fn production_research_benchmark() -> Result<()> {
    ensure!(
        !cfg!(debug_assertions),
        "run this experiment with --release"
    );
    let engine = build_engine(&EngineConfig {
        cache_dir: None,
        ..EngineConfig::default()
    })?;
    let linker = build_linker(&engine)?;
    let mut reports = Vec::new();
    for agent_id in ["utils", "crypto"] {
        let path = agent_path(agent_id);
        let bytes = std::fs::read(&path)
            .with_context(|| format!("build agent components first: {}", path.display()))?;
        let mut compile_times = Vec::new();
        let mut native_size = 0;
        for _ in 0..3 {
            let start = Instant::now();
            let component = Component::new(&engine, &bytes)?;
            compile_times.push(start.elapsed().as_secs_f64() * 1e6);
            native_size = component.serialize()?.len();
        }
        let agent = load_agent(&engine, &linker, &path, agent_id)?;
        let mut cases = Vec::new();
        for payload_size in [16, 4096, 1024 * 1024] {
            let data = "x".repeat(payload_size);
            let (capability, input) = if agent_id == "utils" {
                (
                    "return-input",
                    serde_json::to_vec(&serde_json::json!({"value":data}))?,
                )
            } else {
                (
                    "hash",
                    serde_json::to_vec(&serde_json::json!({"data":data}))?,
                )
            };
            let (mut reused, instance) =
                instantiate(&engine, &agent.pre, HostState::new(context())).await?;
            let func = invoke_func(&mut reused, &instance, &agent)?;
            let expected = invoke(&mut reused, func, capability, &input).await?;
            if agent_id == "utils" {
                assert_eq!(
                    serde_json::from_slice::<serde_json::Value>(&expected)?,
                    serde_json::json!(data)
                );
            } else {
                use sha2::Digest;
                let result: serde_json::Value = serde_json::from_slice(&expected)?;
                assert_eq!(
                    result["hash"],
                    format!("{:x}", sha2::Sha256::digest(data.as_bytes()))
                );
            }
            let n = if payload_size < 1024 * 1024 { 100 } else { 20 };
            let mut reuse_times = Vec::new();
            let mut fresh_times = Vec::new();
            let mut instantiate_times = Vec::new();
            let mut max_memory = 0;
            // Alternate paths, after warmup, to reduce ordering bias.
            for _ in 0..n {
                let start = Instant::now();
                let out = invoke(&mut reused, func, capability, &input).await?;
                reuse_times.push(start.elapsed().as_secs_f64() * 1e6);
                assert_eq!(out, expected);

                let start = Instant::now();
                let (mut store, instance) =
                    instantiate(&engine, &agent.pre, HostState::new(context())).await?;
                let fresh_func = invoke_func(&mut store, &instance, &agent)?;
                instantiate_times.push(start.elapsed().as_secs_f64() * 1e6);
                let out = invoke(&mut store, fresh_func, capability, &input).await?;
                max_memory = max_memory.max(store.data().limiter.memory_peak_bytes);
                drop(store);
                fresh_times.push(start.elapsed().as_secs_f64() * 1e6);
                assert_eq!(out, expected);
            }
            cases.push(serde_json::json!({
                "payload_bytes":payload_size, "input_bytes":input.len(), "output_bytes":expected.len(),
                "reuse_store_call":stats(reuse_times), "cached_fresh_store_call_and_drop":stats(fresh_times),
                "fresh_store_setup_and_export_lookup":stats(instantiate_times),
                "largest_guest_memory_bytes":max_memory
            }));
        }
        // This is a resident-store probe, not concurrent throughput. RSS deltas
        // include allocator history and code pages; per-memory peaks are NOT RSS.
        let mut resident = Vec::new();
        for count in [1, 4, 16] {
            let before = rss_kib();
            let mut stores = Vec::new();
            let mut peak_sum = 0;
            for _ in 0..count {
                let (mut store, instance) =
                    instantiate(&engine, &agent.pre, HostState::new(context())).await?;
                let func = invoke_func(&mut store, &instance, &agent)?;
                let (cap, input) = if agent_id == "utils" {
                    ("return-input", br#"{"value":42}"#.as_slice())
                } else {
                    ("hash", br#"{"data":"x"}"#.as_slice())
                };
                invoke(&mut store, func, cap, input).await?;
                peak_sum += store.data().limiter.memory_peak_bytes;
                stores.push(store);
            }
            let held = rss_kib();
            drop(stores);
            resident.push(serde_json::json!({"held_stores":count,"rss_before_kib":before,"rss_held_kib":held,"rss_after_drop_kib":rss_kib(),"sum_of_largest_guest_memories_bytes":peak_sum}));
        }
        reports.push(serde_json::json!({"agent":agent_id,"raw_wasm_bytes":bytes.len(),"serialized_native_bytes":native_size,"uncached_compile":stats(compile_times),"cases":cases,"resident_stores":resident}));
    }
    println!(
        "RESEARCH_JSON={}",
        serde_json::to_string(&serde_json::json!({
            "os":std::env::consts::OS,"arch":std::env::consts::ARCH,"host_profile":"release",
            "wasmtime":"46.0.1","disk_cache":false,"pooling_allocator":false,"reports":reports
        }))?
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn per_memory_limit_is_not_a_store_aggregate_limit() -> Result<()> {
    let engine = build_engine(&EngineConfig {
        cache_dir: None,
        ..EngineConfig::default()
    })?;
    let module = wasmtime::Module::new(
        &engine,
        r#"(module
        (memory (export "a") 16) (memory (export "b") 16) (memory (export "c") 16))"#,
    )?;
    let mut state = HostState::new(context());
    state.set_limits(1024 * 1024, 100);
    let mut store = Store::new(&engine, state);
    store.set_epoch_deadline(1 << 40);
    store.limiter(|state| &mut state.limiter);
    let instance = wasmtime::Instance::new_async(&mut store, &module, &[]).await?;
    let total: usize = ["a", "b", "c"]
        .into_iter()
        .map(|name| {
            instance
                .get_memory(&mut store, name)
                .unwrap()
                .data_size(&store)
        })
        .sum();
    assert_eq!(total, 3 * 1024 * 1024);
    assert_eq!(store.data().limiter.memory_peak_bytes, 1024 * 1024);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn cached_code_does_not_share_mutable_guest_state() -> Result<()> {
    let engine = build_engine(&EngineConfig {
        cache_dir: None,
        ..EngineConfig::default()
    })?;
    let module = wasmtime::Module::new(
        &engine,
        r#"(module (global $n (mut i32) (i32.const 0))
      (func (export "next") (result i32) (global.set $n (i32.add (global.get $n) (i32.const 1))) (global.get $n)))"#,
    )?;
    let linker = wasmtime::Linker::<()>::new(&engine);
    let pre = linker.instantiate_pre(&module)?;
    let mut a = Store::new(&engine, ());
    a.set_epoch_deadline(1 << 40);
    let mut b = Store::new(&engine, ());
    b.set_epoch_deadline(1 << 40);
    let a_instance = pre.instantiate_async(&mut a).await?;
    let b_instance = pre.instantiate_async(&mut b).await?;
    let af = a_instance.get_typed_func::<(), i32>(&mut a, "next")?;
    let bf = b_instance.get_typed_func::<(), i32>(&mut b, "next")?;
    assert_eq!(af.call_async(&mut a, ()).await?, 1);
    assert_eq!(af.call_async(&mut a, ()).await?, 2);
    assert_eq!(bf.call_async(&mut b, ()).await?, 1);
    Ok(())
}

#[tokio::test]
async fn shared_root_and_child_admission_can_deadlock() -> Result<()> {
    let budget = Arc::new(tokio::sync::Semaphore::new(1));
    let parent = budget.clone().acquire_owned().await?;
    assert!(
        tokio::time::timeout(Duration::from_millis(50), budget.clone().acquire_owned())
            .await
            .is_err()
    );
    drop(parent);
    let child = tokio::time::timeout(Duration::from_secs(1), budget.acquire_owned()).await??;
    drop(child);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn poc_pending_cancel_can_lose_to_an_already_ready_call() -> Result<()> {
    let engine = build_engine(&EngineConfig {
        cache_dir: None,
        ..EngineConfig::default()
    })?;
    let component = Component::new(&engine, RECOVERY)?;
    let cancel = Arc::new(Cancel {
        flag: AtomicBool::new(false),
        wake: Notify::new(),
    });
    cancel.request(&engine);
    let (_, inbox) = watch::channel(0);
    let events = Arc::new(watch::channel(Vec::new()).0);
    // No epoch ticker: a short ready call can finish before the next tick.
    // This pins a PoC limitation, not the desired production race policy.
    let result = run_child(engine, component, 1, 17, cancel, inbox, events).await?;
    assert_eq!(
        result, 17,
        "the current result-first select has no durable cancellation fence"
    );
    Ok(())
}
