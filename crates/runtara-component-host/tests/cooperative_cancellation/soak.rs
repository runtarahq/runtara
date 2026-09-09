//! Repeated cancellation against one Engine and one Component.
//!
//! The single-cycle proofs in `real_agent` each build their own Engine, so they
//! say nothing about what a long-lived process retains. This runs many cancel
//! cycles through the *same* prepared component, taking a fresh Store per cycle
//! and sampling the process between them, which is what the release gate means
//! by component-state preservation, bounded pending operations and retained
//! memory or handles after quiescence.
//!
//! Manual: it is `#[ignore]`d because a meaningful sample takes minutes.
//!
//! ```sh
//! RUNTARA_SOAK_CYCLES=200 cargo test -p runtara-component-host \
//!   --features component-integration-tests --test cooperative_cancellation \
//!   --release repeated_cancellation_soak -- --ignored --nocapture
//! ```
use super::*;
use runtara_component_host::{CallContext, HostState};

/// One RSS/descriptor sample, taken between cycles rather than during one.
#[derive(Debug, Clone, Copy)]
struct Sample {
    cycle: usize,
    rss_bytes: u64,
    descriptors: usize,
}

/// Resident set size of this process, or `None` off Linux.
fn rss_bytes() -> Option<u64> {
    let statm = std::fs::read_to_string("/proc/self/statm").ok()?;
    let pages: u64 = statm.split_whitespace().nth(1)?.parse().ok()?;
    Some(pages * 4096)
}

/// Open descriptors held by this process, or `None` off Linux.
fn descriptors() -> Option<usize> {
    Some(std::fs::read_dir("/proc/self/fd").ok()?.count())
}

/// Serve one pair of requests per cycle: a `/pending` that never answers and is
/// expected to be closed by guest cleanup, then an `/ok` that completes.
///
/// The per-cycle `started` and `cleaned` gates arrive over `handles`, so the
/// guest's cancellation point is driven by the request actually being in
/// flight — the same ordering the single-cycle proof uses.
async fn serve(
    listener: tokio::net::TcpListener,
    mut handles: tokio::sync::mpsc::Receiver<(Arc<Notify>, Arc<Notify>)>,
) -> anyhow::Result<()> {
    let mut cycle = 0usize;
    while let Some((started, cleaned)) = handles.recv().await {
        let (mut socket, _) = listener.accept().await?;
        let request = real_agent::request_headers(&mut socket).await?;
        anyhow::ensure!(
            request.starts_with(b"GET /pending "),
            "cycle {cycle}: expected the pending request first"
        );
        // Withhold headers entirely, then let the guest reach its cancellation
        // point. The client must go away because it was cancelled, not because
        // the server answered or hung up.
        started.notify_one();
        let mut byte = [0];
        match socket.read(&mut byte).await {
            Ok(0) => {}
            Err(error) if error.kind() == std::io::ErrorKind::ConnectionReset => {}
            other => anyhow::bail!("cycle {cycle}: pending request was not closed: {other:?}"),
        }
        drop(socket);
        cleaned.notify_one();

        let (mut socket, _) = listener.accept().await?;
        let request = real_agent::request_headers(&mut socket).await?;
        anyhow::ensure!(
            request.starts_with(b"GET /ok "),
            "cycle {cycle}: expected the reuse request second"
        );
        socket
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok")
            .await?;
        cycle += 1;
    }
    Ok(())
}

/// Run one cancel-then-reuse cycle on a fresh Store over an already prepared
/// component. Only the Store and the per-cycle linker closures are new, so
/// anything that grows across cycles is retention, not setup cost.
async fn cycle(
    engine: &wasmtime::Engine,
    component: &Component,
    started: Arc<Notify>,
    cleaned: Arc<Notify>,
) -> anyhow::Result<serde_json::Value> {
    let sibling_started = Arc::new(Notify::new());

    let mut linker = runtara_component_host::build_linker(engine)?;
    linker.root().func_wrap_concurrent("sibling", {
        let sibling_started = sibling_started.clone();
        let cleaned = cleaned.clone();
        move |_, (): ()| {
            let sibling_started = sibling_started.clone();
            let cleaned = cleaned.clone();
            Box::pin(async move {
                sibling_started.notify_one();
                cleaned.notified().await;
                Ok((7u32,))
            })
        }
    })?;
    linker.root().func_wrap_concurrent("signal", {
        let started = started.clone();
        let sibling_started = sibling_started.clone();
        move |_, (): ()| {
            let started = started.clone();
            let sibling_started = sibling_started.clone();
            Box::pin(async move {
                started.notified().await;
                sibling_started.notified().await;
                Ok(())
            })
        }
    })?;

    let state = HostState::new(Arc::new(CallContext::placeholder_for_metadata()));
    let mut store = Store::new(engine, state);
    let instance = linker.instantiate_async(&mut store, component).await?;
    let run = instance.get_typed_func::<(), (Vec<u8>,)>(&mut store, "run")?;
    let (output,) = run.call_async(&mut store, ()).await?;
    Ok(serde_json::from_slice(&output)?)
}

#[tokio::test]
#[ignore = "manual capacity soak; minutes per run"]
async fn repeated_cancellation_soak() -> anyhow::Result<()> {
    let cycles: usize = std::env::var("RUNTARA_SOAK_CYCLES")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(100);
    // Early cycles pay for lazy allocator growth and first-touch pages, so they
    // are reported but excluded from the growth bound.
    let warmup = (cycles / 5).max(1);

    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let base = format!("http://{}", listener.local_addr()?);
    let pending = serde_json::to_vec(
        &serde_json::json!({"url":format!("{base}/pending"), "timeout_ms":120000}),
    )?;
    let reuse = serde_json::to_vec(
        &serde_json::json!({"url":format!("{base}/ok"), "response_type":"text"}),
    )?;
    let bytes =
        real_agent::compose_agent("http", "http-request", &pending, "http-request", &reuse)?;

    let engine = runtara_component_host::build_engine(&runtara_component_host::EngineConfig {
        cache_dir: None,
        enable_epoch_interruption: false,
    })?;
    // Prepared once. A per-cycle Component would measure compilation, not what
    // repeated cancellation leaves behind.
    let component = Component::new(&engine, bytes)?;
    let (handles, receiver) = tokio::sync::mpsc::channel(1);
    let server = tokio::spawn(serve(listener, receiver));

    let mut samples = Vec::new();
    for index in 0..cycles {
        let started = Arc::new(Notify::new());
        let cleaned = Arc::new(Notify::new());
        handles.send((started.clone(), cleaned.clone())).await?;
        let output = tokio::time::timeout(
            Duration::from_secs(30),
            cycle(&engine, &component, started, cleaned),
        )
        .await
        .map_err(|_| anyhow::anyhow!("cycle {index} did not finish within 30s"))??;
        anyhow::ensure!(
            output["status_code"] == 200 && output["body"] == "ok" && output["success"] == true,
            "cycle {index}: reuse call did not succeed: {output}"
        );
        if let (Some(rss_bytes), Some(descriptors)) = (rss_bytes(), descriptors()) {
            samples.push(Sample {
                cycle: index,
                rss_bytes,
                descriptors,
            });
        }
    }

    drop(handles);
    tokio::time::timeout(Duration::from_secs(30), server)
        .await
        .map_err(|_| anyhow::anyhow!("fixture server did not drain"))???;

    println!("SOAK_CYCLES={cycles}");
    if samples.is_empty() {
        println!("SOAK_SAMPLES=unavailable (procfs only)");
        return Ok(());
    }

    let after_warmup: Vec<Sample> = samples
        .iter()
        .copied()
        .filter(|sample| sample.cycle >= warmup)
        .collect();
    let first = after_warmup.first().expect("a sample past warmup");
    let last = after_warmup.last().expect("a sample past warmup");
    let peak = after_warmup
        .iter()
        .map(|sample| sample.rss_bytes)
        .max()
        .expect("a sample past warmup");
    let descriptor_peak = after_warmup
        .iter()
        .map(|sample| sample.descriptors)
        .max()
        .expect("a sample past warmup");
    let growth = last.rss_bytes as i64 - first.rss_bytes as i64;
    let measured = after_warmup.len();

    println!(
        "SOAK_JSON={}",
        serde_json::json!({
            "cycles": cycles,
            "warmup_cycles": warmup,
            "measured_cycles": measured,
            "rss_bytes": {
                "first_after_warmup": first.rss_bytes,
                "last": last.rss_bytes,
                "peak": peak,
                "growth": growth,
                "growth_per_cycle": growth as f64 / measured as f64,
            },
            "descriptors": {
                "first_after_warmup": first.descriptors,
                "last": last.descriptors,
                "peak": descriptor_peak,
            },
        })
    );

    // Descriptors are the sharp signal: a cancelled request that leaked its
    // socket shows up here immediately and cannot be explained by allocator
    // behaviour.
    anyhow::ensure!(
        last.descriptors <= first.descriptors,
        "descriptors grew across {measured} cycles: {} -> {}",
        first.descriptors,
        last.descriptors
    );
    // RSS is noisier, so this only has to catch a real per-cycle leak rather
    // than pin an absolute footprint.
    let per_cycle = growth as f64 / measured as f64;
    anyhow::ensure!(
        per_cycle < 64.0 * 1024.0,
        "RSS grew {per_cycle:.0} bytes per cycle across {measured} cycles"
    );
    Ok(())
}
