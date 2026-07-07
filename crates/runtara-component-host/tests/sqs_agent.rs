//! End-to-end smoke test: load `runtara_agent_sqs.wasm` directly into wasmtime
//! and drive the `invoke` export. SQS capabilities all require a live AWS
//! connection, so we don't exercise a happy path here (that needs a mock/live
//! endpoint); instead we prove the full dispatch pipeline through the real
//! compiled component:
//!   - a wired capability routes to its executor, deserializes a realistic
//!     input (including the KMS/attributes maps), and returns the structured
//!     `SQS_MISSING_CONNECTION` error when no connection is attached;
//!   - an unknown capability id returns `UNKNOWN_CAPABILITY` (so the dispatcher
//!     genuinely distinguishes wired from unknown, unlike a bare success flag).
//!
//! Skipped if the .wasm is missing — build it first with
//! `cargo component build --release --target wasm32-wasip2 -p runtara-agent-sqs`.

use std::path::PathBuf;
use std::sync::Arc;

use runtara_component_host::{
    CallContext, EngineConfig, HostState, build_engine, build_linker, instantiate, load_agent,
};

fn agent_wasm_path() -> PathBuf {
    let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    // cargo-component drops the finalized component under wasm32-wasip2/
    // (the wasm32-wasip1/ artifact is a malformed intermediate — don't use it).
    workspace.join("target/wasm32-wasip2/release/runtara_agent_sqs.wasm")
}

type InvokeFunc = wasmtime::component::TypedFunc<
    (
        String,
        Vec<u8>,
        Option<runtara_component_host::ConnectionInfo>,
    ),
    (Result<Vec<u8>, runtara_component_host::ErrorInfo>,),
>;

#[tokio::test(flavor = "multi_thread")]
async fn sqs_dispatch_pipeline() -> anyhow::Result<()> {
    let wasm = agent_wasm_path();
    if !wasm.exists() {
        eprintln!(
            "SKIP: {} not found. Run `cargo component build --release --target wasm32-wasip2 -p runtara-agent-sqs` first.",
            wasm.display()
        );
        return Ok(());
    }

    let engine = build_engine(&EngineConfig::default())?;
    let linker = build_linker(&engine)?;
    let loaded = load_agent(&engine, &linker, &wasm, "sqs")?;

    let ctx = Arc::new(CallContext::for_test(
        "tenant-test",
        "http://localhost:9999",
        "http://localhost:9998",
        "http://localhost:9997",
        "http://localhost:9996",
    ));
    let state = HostState::new(ctx.clone());
    let (mut store, instance) = instantiate(&engine, &loaded.pre, state).await?;

    let iface_idx = instance
        .get_export_index(&mut store, None, &loaded.capabilities_iface)
        .expect("capabilities interface export");
    let invoke_idx = instance
        .get_export_index(&mut store, Some(&iface_idx), "invoke")
        .expect("invoke export inside capabilities");
    let invoke: InvokeFunc = instance.get_typed_func(&mut store, invoke_idx)?;

    // 1. A wired message capability: realistic input deserializes and the
    //    executor runs, short-circuiting on the missing connection.
    let (result,) = invoke
        .call_async(
            &mut store,
            (
                "queue-send-message".to_string(),
                br#"{"queue_url":"https://sqs.us-east-1.amazonaws.com/123456789012/q","message_body":"hello"}"#.to_vec(),
                None,
            ),
        )
        .await?;
    let err = result.expect_err("send-message with no connection must error");
    assert_eq!(
        err.code, "SQS_MISSING_CONNECTION",
        "expected missing-connection error, got {}: {}",
        err.code, err.message
    );

    // 2. The KMS-bearing queue capability: proves the attributes HashMap + the
    //    typed KMS fields deserialize through the real component (a deserialize
    //    failure would surface a different code than SQS_MISSING_CONNECTION).
    let (result,) = invoke
        .call_async(
            &mut store,
            (
                "queue-create-queue".to_string(),
                br#"{"queue_name":"orders.fifo","kms_master_key_id":"alias/my-key","attributes":{"VisibilityTimeout":"30"},"fifo_queue":true}"#.to_vec(),
                None,
            ),
        )
        .await?;
    let err = result.expect_err("create-queue with no connection must error");
    assert_eq!(
        err.code, "SQS_MISSING_CONNECTION",
        "KMS input should deserialize then fail on connection, got {}: {}",
        err.code, err.message
    );

    // 3. An unknown capability id is rejected distinctly.
    let (result,) = invoke
        .call_async(
            &mut store,
            ("queue-does-not-exist".to_string(), b"{}".to_vec(), None),
        )
        .await?;
    let err = result.expect_err("unknown capability must error");
    assert_eq!(err.code, "UNKNOWN_CAPABILITY");

    Ok(())
}
