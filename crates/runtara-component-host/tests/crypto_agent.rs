//! End-to-end smoke test for Phase 1: load the runtara-agent-crypto component
//! into wasmtime, call `list-capabilities()`, then `invoke("hash", ...)` and
//! assert the SHA-256 output.
//!
//! Requires `cargo component build --release --target wasm32-wasip2
//! -p runtara-agent-crypto` first. Skipped (with a clear message) if the
//! .wasm file isn't present.

use std::path::PathBuf;
use std::sync::Arc;

use runtara_component_host::{
    CallContext, EngineConfig, HostState, build_engine, build_linker, instantiate, load_agent,
};

fn agent_wasm_path() -> PathBuf {
    // crates/runtara-component-host -> workspace root -> target/...
    let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    // cargo component drops the output under wasm32-wasip1 even though the
    // target triple we asked for is wasm32-wasip2 — see cargo-component
    // adapter behavior. Both names are checked.
    let p1 = workspace.join("target/wasm32-wasip1/release/runtara_agent_crypto.wasm");
    let p2 = workspace.join("target/wasm32-wasip2/release/runtara_agent_crypto.wasm");
    if p1.exists() { p1 } else { p2 }
}

#[tokio::test(flavor = "multi_thread")]
async fn crypto_list_capabilities_and_hash() -> anyhow::Result<()> {
    let wasm = agent_wasm_path();
    if !wasm.exists() {
        eprintln!(
            "SKIP: {} not found. Run `cargo component build --release --target wasm32-wasip2 -p runtara-agent-crypto` first.",
            wasm.display()
        );
        return Ok(());
    }

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
    let (mut store, agent) = instantiate(&engine, &loaded.pre, state).await?;

    let caps = agent
        .runtara_agent_capabilities()
        .call_list_capabilities(&mut store)
        .await?;
    assert_eq!(caps.len(), 2, "expected hash + hmac, got {:?}", caps);
    assert!(caps.iter().any(|c| c.id == "hash"));
    assert!(caps.iter().any(|c| c.id == "hmac"));

    // Fresh store for the invoke call — components don't share Store across
    // calls in this Phase 1 dispatcher model.
    let state2 = HostState::new(ctx.clone());
    let (mut store2, agent2) = instantiate(&engine, &loaded.pre, state2).await?;
    let result = agent2
        .runtara_agent_capabilities()
        .call_invoke(&mut store2, "hash", r#"{"data":"hello"}"#, None)
        .await?
        .map_err(|e| anyhow::anyhow!("guest error: {}: {}", e.code, e.message))?;

    // SHA-256("hello") = 2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824
    assert!(result.contains("2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"));
    Ok(())
}
