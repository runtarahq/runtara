//! Deadline race contract before DSL integration. The parent chooses and cleans
//! up standard subtasks; the native fixture only supplies I/O and readiness.
use super::*;
use runtara_component_host::{CallContext, HostState};

#[derive(Clone, Copy)]
enum Case {
    Headers,
    Body,
    Complete,
    BothReady,
    AlreadyDue,
    MaximumWait,
    ReturnsDuringCancel,
}

async fn run_deadline(case: Case) -> anyhow::Result<()> {
    run_deadline_with_parent(case, include_str!("deadline-parent.wat")).await
}

async fn run_deadline_with_parent(case: Case, source: &str) -> anyhow::Result<()> {
    let pending = matches!(
        case,
        Case::Headers | Case::Body | Case::AlreadyDue | Case::ReturnsDuringCancel
    );
    let timeout = match case {
        Case::Headers | Case::Body | Case::BothReady | Case::ReturnsDuringCancel => 20u64,
        Case::AlreadyDue => 0,
        Case::MaximumWait => u64::MAX,
        Case::Complete => 60_000,
    };
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let base = format!("http://{}", listener.local_addr()?);
    let input = serde_json::to_vec(&serde_json::json!({
        "url":format!("{base}/first"), "timeout_ms":300_000, "response_type":"text"
    }))?;
    let second = serde_json::to_vec(&serde_json::json!({
        "url":format!("{base}/after"), "response_type":"text"
    }))?;
    let parent = source.replace("{{TIMEOUT}}", &(timeout as i64).to_string());
    let bytes = if matches!(case, Case::ReturnsDuringCancel) {
        real_agent::compose_agent_bytes(
            &parent,
            "http",
            "http-request",
            &input,
            "http-request",
            &second,
            &wat::parse_str(include_str!("return-during-cancel.wat"))?,
        )?
    } else {
        real_agent::compose_agent_with_parent(
            &parent,
            "http",
            "http-request",
            &input,
            "http-request",
            &second,
        )?
    };
    let engine = runtara_component_host::build_engine(&runtara_component_host::EngineConfig {
        cache_dir: None,
        enable_epoch_interruption: false,
    })?;
    let component = Component::new(&engine, bytes)?;
    let started = Arc::new(Notify::new());
    let cleaned = Arc::new(Notify::new());
    let requests = Arc::new(AtomicUsize::new(0));
    let mut server = tokio::spawn({
        let started = started.clone();
        let cleaned = cleaned.clone();
        let requests = requests.clone();
        async move {
            let (mut socket, _) = listener.accept().await?;
            let request = real_agent::request_headers(&mut socket).await?;
            anyhow::ensure!(request.starts_with(b"GET /first "));
            requests.fetch_add(1, Ordering::SeqCst);
            if pending {
                if matches!(case, Case::Body) {
                    socket
                        .write_all(
                            b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\nConnection: close\r\n\r\nx",
                        )
                        .await?;
                }
                started.notify_one();
                let mut byte = [0];
                match socket.read(&mut byte).await {
                    Ok(0) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::ConnectionReset => {}
                    other => anyhow::bail!("pending request was not locally closed: {other:?}"),
                }
            } else {
                socket
                    .write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok",
                    )
                    .await?;
                socket.shutdown().await?;
                started.notify_one();
            }
            drop(socket);
            cleaned.notify_one();
            let (mut socket, _) = listener.accept().await?;
            let request = real_agent::request_headers(&mut socket).await?;
            anyhow::ensure!(request.starts_with(b"GET /after "));
            requests.fetch_add(1, Ordering::SeqCst);
            socket
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok")
                .await?;
            anyhow::Ok(())
        }
    });
    let result = tokio::time::timeout(Duration::from_secs(5), async {
        let mut linker = runtara_component_host::build_linker(&engine)?;
        if matches!(case, Case::ReturnsDuringCancel) {
            // Synthetic cancellation callback returns normally after dropping
            // its pending I/O. The six other cases use the built HTTP Agent.
            let native_calls = Arc::new(AtomicUsize::new(0));
            let native_base = base.clone();
            linker
                .root()
                .func_wrap_concurrent("request", move |_, (): ()| {
                    let first = native_calls.fetch_add(1, Ordering::SeqCst) == 0;
                    let url = format!("{}/{}", native_base, if first { "first" } else { "after" });
                    Box::pin(async move {
                        let response = reqwest::Client::builder()
                            .no_proxy()
                            .build()?
                            .get(url)
                            .send()
                            .await?;
                        response.bytes().await?;
                        Ok((42u32,))
                    })
                })?;
        }
        let sibling_started = Arc::new(Notify::new());
        linker.root().func_wrap_concurrent("sibling", {
            let sibling_started = sibling_started.clone();
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
        linker
            .root()
            .func_wrap_concurrent("ready-barrier", move |_, (): ()| {
                let started = started.clone();
                let sibling_started = sibling_started.clone();
                Box::pin(async move {
                    started.notified().await;
                    sibling_started.notified().await;
                    if matches!(case, Case::BothReady) {
                        // Require both actual completion events below; this delay
                        // arranges readiness, it does not select the race winner.
                        tokio::time::sleep(Duration::from_millis(100)).await;
                    }
                    Ok(())
                })
            })?;
        let mut store = Store::new(
            &engine,
            HostState::new(Arc::new(CallContext::placeholder_for_metadata())),
        );
        let instance = linker.instantiate_async(&mut store, &component).await?;
        let run = instance.get_typed_func::<(), (Vec<u8>,)>(&mut store, "run")?;
        let before = std::time::Instant::now();
        let (output,) = run.call_async(&mut store, ()).await?;
        let output: serde_json::Value = serde_json::from_slice(&output)?;
        anyhow::ensure!(
            output["status_code"] == 200 && output["body"] == "ok",
            "recovery/reuse failed: {output}"
        );
        let timed_out = instance
            .get_typed_func::<(), (u32,)>(&mut store, "timed-out")?
            .call_async(&mut store, ())
            .await?
            .0;
        let ready = instance
            .get_typed_func::<(), (u32,)>(&mut store, "ready-mask")?
            .call_async(&mut store, ())
            .await?
            .0;
        let resolution = instance
            .get_typed_func::<(), (u32,)>(&mut store, "cancel-resolution")?
            .call_async(&mut store, ())
            .await?
            .0;
        anyhow::ensure!(
            timed_out == u32::from(pending),
            "wrong selected outcome: {timed_out}, ready={ready}, resolution={resolution}"
        );
        if pending {
            anyhow::ensure!(
                ready == 2
                    && resolution
                        == if matches!(case, Case::ReturnsDuringCancel) {
                            2
                        } else {
                            4
                        },
                "deadline did not cancel the target: ready={ready}, resolution={resolution}"
            );
            anyhow::ensure!(before.elapsed() >= Duration::from_millis(timeout));
        } else if matches!(case, Case::BothReady) {
            anyhow::ensure!(
                ready == 3,
                "fixture did not establish simultaneous readiness: {ready}"
            );
        } else {
            anyhow::ensure!(ready == 1 && resolution == 0);
        }
        anyhow::ensure!(
            requests.load(Ordering::SeqCst) == 2,
            "unexpected retry or missing reuse"
        );
        anyhow::Ok(())
    })
    .await;
    if !matches!(result, Ok(Ok(()))) {
        server.abort();
        let _ = server.await;
        return result?;
    }
    match tokio::time::timeout(Duration::from_secs(2), &mut server).await {
        Ok(result) => result?,
        Err(error) => {
            server.abort();
            let _ = server.await;
            Err(error.into())
        }
    }
}

#[tokio::test]
async fn deadline_cancels_pending_headers_and_preserves_sibling_and_reuse() -> anyhow::Result<()> {
    run_deadline(Case::Headers).await
}
#[tokio::test]
async fn deadline_cancels_partial_body_and_preserves_sibling_and_reuse() -> anyhow::Result<()> {
    run_deadline(Case::Body).await
}
#[tokio::test]
async fn deadline_completion_cancels_unused_long_timer() -> anyhow::Result<()> {
    run_deadline(Case::Complete).await
}
#[tokio::test]
async fn deadline_completion_wins_when_both_events_are_observed_ready() -> anyhow::Result<()> {
    run_deadline(Case::BothReady).await
}
#[tokio::test]
async fn deadline_already_due_wait_cancels_pending_operation() -> anyhow::Result<()> {
    // Zero remaining wait is an expired deadline, not authored timeout: 0.
    run_deadline(Case::AlreadyDue).await
}
#[tokio::test]
async fn deadline_maximum_wait_can_be_cancelled_without_overflow() -> anyhow::Result<()> {
    run_deadline(Case::MaximumWait).await
}

#[tokio::test]
async fn deadline_selected_timeout_rejects_a_value_returned_during_cleanup() -> anyhow::Result<()> {
    run_deadline(Case::ReturnsDuringCancel).await
}

#[tokio::test]
async fn deadline_contract_detects_failure_to_observe_all_ready_events() -> anyhow::Result<()> {
    let source = include_str!("deadline-parent.wat");
    let drain = "(br $drain)";
    assert_eq!(source.matches(drain).count(), 1);
    let broken = source.replace(drain, "(br $drained)");
    let error = run_deadline_with_parent(Case::BothReady, &broken)
        .await
        .unwrap_err();
    let message = error.to_string();
    anyhow::ensure!(
        message.contains("fixture did not establish simultaneous readiness")
            || message.contains("wrong selected outcome"),
        "unexpected failure: {error:#}"
    );
    Ok(())
}

#[tokio::test]
async fn deadline_contract_detects_accepting_a_late_cleanup_value() -> anyhow::Result<()> {
    let source = include_str!("deadline-parent.wat");
    let resolution = "(global.set $resolution (call $cancel-drop (global.get $target-handle)))";
    assert_eq!(source.matches(resolution).count(), 1);
    let broken = source.replace(resolution, &format!("{resolution}\n          (if (i32.eq (global.get $resolution) (i32.const 2)) (then (global.set $timed-out (i32.const 0))))"));
    let error = run_deadline_with_parent(Case::ReturnsDuringCancel, &broken)
        .await
        .unwrap_err();
    anyhow::ensure!(
        error.to_string().contains("wrong selected outcome: 0"),
        "unexpected failure: {error:#}"
    );
    Ok(())
}
