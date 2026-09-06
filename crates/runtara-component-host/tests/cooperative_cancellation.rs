//! Standard Component Model cancellation proof. No custom task API, catalog,
//! launcher or per-call Store. This fixture precedes real Agent binding changes.
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    sync::Notify,
};
use wasmtime::{
    Store,
    component::{Component, Linker},
};

type Trace = Arc<Mutex<Vec<u32>>>;
struct RequestGuard {
    trace: Trace,
    cleaned: Arc<Notify>,
}
impl Drop for RequestGuard {
    fn drop(&mut self) {
        self.trace.lock().unwrap().push(40);
        self.cleaned.notify_one();
    }
}

const COMPOSED: &str = include_str!("cooperative_cancellation/composed.wat");

async fn run_proof(
    source: &str,
    request: Option<(String, bool)>,
    started: Arc<Notify>,
) -> anyhow::Result<()> {
    let engine = runtara_component_host::build_engine(&runtara_component_host::EngineConfig {
        cache_dir: None,
        enable_epoch_interruption: false,
    })?;
    let component = Component::new(&engine, source)?;
    let trace = Trace::default();
    let sibling_started = Arc::new(Notify::new());
    let cleaned = Arc::new(Notify::new());
    let calls = Arc::new(AtomicUsize::new(0));
    let mut linker = Linker::<()>::new(&engine);
    linker.root().func_wrap_concurrent("request", {
        let trace = trace.clone();
        let started = started.clone();
        let cleaned = cleaned.clone();
        let calls = calls.clone();
        move |_, (): ()| {
            let trace = trace.clone();
            let started = started.clone();
            let cleaned = cleaned.clone();
            let request = request.clone();
            let first = calls.fetch_add(1, Ordering::SeqCst) == 0;
            Box::pin(async move {
                if first {
                    let _guard = RequestGuard { trace, cleaned };
                    if let Some((url, body_wait)) = request {
                        // Test transport only. The real agent's bindings and
                        // production host-io path still require a separate proof.
                        let response = reqwest::Client::builder()
                            .no_proxy()
                            .build()?
                            .get(url)
                            .send()
                            .await?;
                        assert!(body_wait, "headers must remain pending");
                        started.notify_one();
                        response.bytes().await?;
                        wasmtime::bail!("body must remain pending until cancellation");
                    }
                    started.notify_one();
                    std::future::pending::<()>().await;
                    unreachable!("only standard cancellation can finish the first request")
                }
                Ok((42u32,))
            })
        }
    })?;
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
    linker
        .root()
        .func_wrap_concurrent("signal", move |_, (): ()| {
            let started = started.clone();
            let sibling_started = sibling_started.clone();
            Box::pin(async move {
                started.notified().await;
                sibling_started.notified().await;
                Ok(())
            })
        })?;
    linker.root().func_wrap("trace", {
        let trace = trace.clone();
        move |_, (event,): (u32,)| {
            trace.lock().unwrap().push(event);
            Ok(())
        }
    })?;
    let mut store = Store::new(&engine, ());
    let instance = linker.instantiate_async(&mut store, &component).await?;
    let run = instance.get_typed_func::<(), (u32,)>(&mut store, "run")?;
    let result = tokio::time::timeout(Duration::from_secs(5), run.call_async(&mut store, ()))
        .await
        .inspect_err(|_| {
            // The negative ABI fixture must have reached cancellation, not
            // merely stalled before the signal or while starting its sibling.
            let events = trace.lock().unwrap();
            assert!(events.starts_with(&[10, 11, 12, 13]));
            assert!(!events.contains(&60), "parent continued before resolution");
        })?
        .map_err(|error| anyhow::anyhow!("{error:#}; trace={:?}", trace.lock().unwrap()))?;
    assert_eq!(result, (99,));
    assert_eq!(*trace.lock().unwrap(), [10, 11, 12, 13, 30, 40, 50, 60]);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    Ok(())
}

#[tokio::test]
async fn standard_cancellation_preserves_composed_sibling_and_instance_state() -> anyhow::Result<()>
{
    run_proof(COMPOSED, None, Arc::new(Notify::new())).await
}

#[tokio::test]
async fn callee_can_return_a_value_instead_of_acknowledging_cancellation() -> anyhow::Result<()> {
    // The standard allows a callee to return normally after a cancellation
    // request. The parent must inspect the actual resolution, not force a
    // CANCELLED outcome merely because it requested cancellation.
    let source = COMPOSED
        .replace("(call $cancelled)", "(call $return (i32.const 123))")
        .replace(
            "(if (i32.ne (call $cancel (local.get $target)) (i32.const 4)) (then unreachable))",
            "(if (i32.ne (call $cancel (local.get $target)) (i32.const 2)) (then unreachable))\n      (if (i32.ne (i32.load (i32.const 0)) (i32.const 123)) (then unreachable))",
        );
    run_proof(&source, None, Arc::new(Notify::new())).await
}

async fn cancel_http(body_wait: bool) -> anyhow::Result<()> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let url = format!("http://{}/pending", listener.local_addr()?);
    let started = Arc::new(Notify::new());
    let server_ready = started.clone();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await?;
        let mut bytes = Vec::new();
        while !bytes.ends_with(b"\r\n\r\n") {
            let byte = socket.read_u8().await?;
            bytes.push(byte);
            anyhow::ensure!(bytes.len() <= 8192, "unexpected request size");
        }
        if body_wait {
            socket
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\nConnection: close\r\n\r\nx")
                .await?;
        } else {
            server_ready.notify_one();
        }
        // Never finish the response. The native pending request must be
        // dropped by standard WASM cancellation, closing this connection.
        let mut buffer = [0; 1];
        match socket.read(&mut buffer).await {
            Ok(0) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::ConnectionReset => Ok(()),
            other => anyhow::bail!("expected cancelled connection to close: {other:?}"),
        }
    });
    let result = run_proof(COMPOSED, Some((url, body_wait)), started).await;
    if result.is_err() {
        server.abort();
        let _ = server.await;
        return result;
    }
    // Keep ownership of the server even when this assertion times out.
    let mut server = server;
    match tokio::time::timeout(Duration::from_secs(5), &mut server).await {
        Ok(result) => result?,
        Err(error) => {
            server.abort();
            let _ = server.await;
            Err(error.into())
        }
    }
}

#[tokio::test]
async fn standard_cancellation_closes_pending_http_headers() -> anyhow::Result<()> {
    cancel_http(false).await
}

#[tokio::test]
async fn standard_cancellation_closes_pending_http_body() -> anyhow::Result<()> {
    cancel_http(true).await
}

#[tokio::test]
async fn async_typing_with_synchronous_bindings_does_not_acknowledge_cancellation()
-> anyhow::Result<()> {
    let start = COMPOSED.find("  (component $agent").unwrap();
    let end = COMPOSED.find("  (instance $target").unwrap();
    let mut source = COMPOSED.to_owned();
    source.replace_range(
        start..end,
        include_str!("cooperative_cancellation/synchronous-agent.wat"),
    );
    // Same parent, signal and pending I/O. Only the agent ABI changes. A
    // deadline here is a test watchdog, not a successful cancellation outcome.
    // Dropping the whole Store after the deadline is the test's final cleanup.
    let error = run_proof(&source, None, Arc::new(Notify::new()))
        .await
        .expect_err("sync bindings cannot acknowledge cancellation while blocked in I/O");
    assert!(
        error
            .downcast_ref::<tokio::time::error::Elapsed>()
            .is_some(),
        "expected unresolved cancellation, not a trap or validation failure: {error:#}"
    );
    Ok(())
}

#[cfg(feature = "component-integration-tests")]
#[path = "cooperative_cancellation/real_agent.rs"]
mod real_agent;

#[tokio::test]
async fn cancelling_a_queued_call_resolves_before_entry_and_can_be_dropped() -> anyhow::Result<()> {
    // Hold this component's next entry using standard backpressure. The queued
    // call must never invoke host I/O, increment the agent's counter or disturb
    // either live call. The original proof also checks reuse after cancellation.
    let source = COMPOSED
        .replace("(core func $cancelled (canon task.cancel))", "(core func $cancelled (canon task.cancel))\n(core func $inc (canon backpressure.inc))\n(core func $dec (canon backpressure.dec))")
        .replace("(import \"h\" \"cancelled\" (func $cancelled))", "(import \"h\" \"cancelled\" (func $cancelled))\n(import \"h\" \"inc\" (func $inc))\n(import \"h\" \"dec\" (func $dec))")
        .replace("(export \"cancelled\" (func $cancelled))", "(export \"cancelled\" (func $cancelled))\n(export \"inc\" (func $inc))\n(export \"dec\" (func $dec))")
        .replace("(func (export \"run\") (result i32) (local $status i32)", "(func (export \"run\") (result i32) (local $status i32)\n(call $inc)")
        .replace("(call $return ", "(call $dec)\n(call $return ")
        .replace("(call $cancelled)", "(call $dec)\n(call $cancelled)")
        .replace("(local $target i32) (local $sibling i32)", "(local $target i32) (local $sibling i32) (local $queued i32)")
        .replace("(call $trace (i32.const 13))", r#"(call $trace (i32.const 13))
            (local.set $queued (call $target (i32.const 8)))
            (if (i32.and (local.get $queued) (i32.const 15)) (then unreachable))
            (local.set $queued (i32.shr_u (local.get $queued) (i32.const 4)))
            (if (i32.ne (call $cancel (local.get $queued)) (i32.const 3)) (then unreachable))
            (call $drop (local.get $queued))"#);
    run_proof(&source, None, Arc::new(Notify::new())).await
}

#[cfg(feature = "component-integration-tests")]
#[path = "cooperative_cancellation/real_slack.rs"]
mod real_slack;

#[cfg(feature = "component-integration-tests")]
#[path = "cooperative_cancellation/real_ai.rs"]
mod real_ai;

#[cfg(feature = "component-integration-tests")]
#[path = "cooperative_cancellation/real_object_model.rs"]
mod real_object_model;
