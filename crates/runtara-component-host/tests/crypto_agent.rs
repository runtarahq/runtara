//! End-to-end smoke test: load `runtara_agent_crypto.wasm` directly into
//! wasmtime and call the (now-only) `invoke` export. The explicit suite fails
//! if the .wasm is missing — `cargo component build --release --target wasm32-wasip2 -p
//! runtara-agent-crypto` first.

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
    // (it also leaves a malformed intermediate under wasm32-wasip1/ — don't
    // touch that one).
    workspace.join("target/wasm32-wasip2/release/runtara_agent_crypto.wasm")
}

#[tokio::test(flavor = "multi_thread")]
async fn crypto_invoke_hash() -> anyhow::Result<()> {
    let wasm = agent_wasm_path();
    assert!(
        wasm.exists(),
        "component-integration-tests requires {}; run scripts/build-agent-components.sh",
        wasm.display()
    );

    let engine = build_engine(&EngineConfig::default())?;
    let linker = build_linker(&engine)?;
    let loaded = load_agent(&engine, &linker, &wasm, "crypto")?;

    let ctx = Arc::new(CallContext::for_test(
        "tenant-test",
        "http://localhost:9999",
        "http://localhost:9998",
        "http://localhost:9997",
        "http://localhost:9996",
    ));
    let state = HostState::new(ctx.clone());
    let (mut store, instance) = instantiate(&engine, &loaded.pre, state).await?;

    // Dynamic lookup against the interface name the registry cached at load
    // time (per-agent or legacy shared layout).
    let iface_idx = instance
        .get_export_index(&mut store, None, &loaded.capabilities_iface)
        .expect("capabilities interface export");
    let invoke_idx = instance
        .get_export_index(&mut store, Some(&iface_idx), "invoke")
        .expect("invoke export inside capabilities");
    type InvokeFunc = wasmtime::component::TypedFunc<
        (String, Vec<u8>),
        (Result<Vec<u8>, runtara_component_host::ErrorInfo>,),
    >;
    let invoke: InvokeFunc = instance.get_typed_func(&mut store, invoke_idx)?;
    let (result,) = invoke
        .call_async(
            &mut store,
            ("hash".to_string(), br#"{"data":"hello"}"#.to_vec()),
        )
        .await?;
    let result = result.map_err(|e| anyhow::anyhow!("guest error: {}: {}", e.code, e.message))?;

    // SHA-256("hello") = 2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824
    let out: serde_json::Value = serde_json::from_slice(&result)?;
    assert_eq!(
        out.get("hash").and_then(|v| v.as_str()),
        Some("2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824")
    );
    Ok(())
}
