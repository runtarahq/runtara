//! Raw `wasi:http` is denied for workflows and agents alike.
//!
//! Guests get no runtime address (`RUNTARA_HTTP_URL` is gone), but the SDK's
//! HTTP backend would still fall back to `http://127.0.0.1:8003`, which is the
//! embedded core's default port. What keeps a composed artifact off that
//! loopback is the outbound guard: `wasi:http/outgoing-handler.handle` must
//! fail with `HTTP-request-denied` before any connection is attempted.
use super::*;
use test_support::{Ticker, bounded, spec};

/// `(result discriminant, error-code case)` the fixture reports for a denied
/// request: `err(HTTP-request-denied)`.
const DENIED: [u8; 2] = [1, 15];

/// The fixture component, aimed at `authority`.
fn raw_http_component(authority: &str) -> String {
    include_str!("raw_http_test.wat")
        .replace("{{AUTHORITY_LEN}}", &authority.len().to_string())
        .replace("{{AUTHORITY}}", authority)
}

/// A loopback listener the guest aims at; it must never see a connection.
fn listener() -> std::net::TcpListener {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind loopback listener");
    listener
        .set_nonblocking(true)
        .expect("nonblocking listener");
    listener
}

async fn assert_never_connected(listener: &std::net::TcpListener) {
    // Give any connection the host might have spawned time to land.
    tokio::time::sleep(Duration::from_millis(200)).await;
    match listener.accept() {
        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
        other => panic!("raw wasi:http reached the listener: {other:?}"),
    }
}

#[tokio::test]
async fn a_workflow_cannot_reach_a_loopback_listener_over_raw_wasi_http() -> anyhow::Result<()> {
    let listener = listener();
    let authority = listener.local_addr()?.to_string();
    let engine = crate::build_engine(&crate::EngineConfig {
        cache_dir: None,
        ..Default::default()
    })?;
    let _ticker = Ticker::new(engine.clone());
    let executor = WorkflowExecutor::new(engine.clone())?;
    let prepared = executor
        .prepare_precompiled(Component::new(&engine, raw_http_component(&authority))?)
        .await?;
    let mut run = spec();
    // A guest-visible address changes nothing: the guard does not read env.
    run.env
        .insert("RUNTARA_HTTP_URL".into(), format!("http://{authority}"));
    let result =
        bounded(executor.execute_invoke(prepared.instance_pre(), run, b"{}".to_vec())).await;
    assert!(
        matches!(result.exit, InvokeExit::Completed(ref bytes) if bytes == &DENIED),
        "raw wasi:http must fail with HTTP-request-denied: {:?}",
        result.exit
    );
    assert_never_connected(&listener).await;
    Ok(())
}

#[tokio::test]
async fn an_agent_cannot_reach_a_loopback_listener_over_raw_wasi_http() -> anyhow::Result<()> {
    use crate::lifecycle::{WorkflowErrorInfo, WorkflowOutcome};
    let listener = listener();
    let authority = listener.local_addr()?.to_string();
    let engine = crate::build_engine(&crate::EngineConfig {
        cache_dir: None,
        enable_epoch_interruption: false,
    })?;
    let component = Component::new(&engine, raw_http_component(&authority))?;
    let state = crate::HostState::new(Arc::new(crate::CallContext::for_test("tenant-a")));
    let mut store = wasmtime::Store::new(&engine, state);
    let linker = crate::build_linker(&engine)?;
    let instance = linker.instantiate_async(&mut store, &component).await?;
    let interface = instance
        .get_export_index(&mut store, None, crate::lifecycle::ENTRY_INTERFACE_NAME)
        .expect("workflow entry export");
    let export = instance
        .get_export_index(&mut store, Some(&interface), "invoke")
        .expect("invoke export");
    let invoke = instance
        .get_typed_func::<(String, Vec<u8>), (Result<WorkflowOutcome, WorkflowErrorInfo>,)>(
            &mut store, export,
        )?;
    let (result,) = invoke
        .call_async(
            &mut store,
            (
                crate::lifecycle::ENTRY_CAPABILITY.to_owned(),
                b"{}".to_vec(),
            ),
        )
        .await?;
    let Ok(WorkflowOutcome::Completed(bytes)) = result else {
        panic!("expected the fixture to report the handler result: {result:?}")
    };
    assert_eq!(
        bytes, DENIED,
        "raw wasi:http must fail with HTTP-request-denied"
    );
    assert_never_connected(&listener).await;
    Ok(())
}
