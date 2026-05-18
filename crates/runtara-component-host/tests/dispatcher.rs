//! End-to-end smoke test for `ComponentDispatcherService`.
//!
//! Loads runtara_agent_crypto.wasm from the workspace target directory and
//! runs `test_capability` through the same API surface that
//! `AgentTestingService` will call into. Skipped if the .wasm is missing.

use std::path::{Path, PathBuf};

use runtara_component_host::{ComponentDispatcherService, DispatcherEnv, TestCapabilityRequest};

fn agent_components_dir() -> PathBuf {
    let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    let p1 = workspace.join("target/wasm32-wasip1/release");
    let p2 = workspace.join("target/wasm32-wasip2/release");
    if p1.join("runtara_agent_crypto.wasm").exists() {
        p1
    } else {
        p2
    }
}

fn has_crypto_wasm(dir: &Path) -> bool {
    dir.join("runtara_agent_crypto.wasm").exists()
}

fn env() -> DispatcherEnv {
    DispatcherEnv {
        proxy_url: "http://localhost:9999".into(),
        agent_service_url: "http://localhost:9998".into(),
        object_model_url: "http://localhost:9997".into(),
        core_http_url: "http://localhost:9996".into(),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn dispatcher_lists_agents_and_capabilities() -> anyhow::Result<()> {
    let dir = agent_components_dir();
    if !has_crypto_wasm(&dir) {
        eprintln!(
            "SKIP: crypto wasm not built — `cargo component build --release --target wasm32-wasip2 -p runtara-agent-crypto` first"
        );
        return Ok(());
    }

    let dispatcher = ComponentDispatcherService::from_dir(&dir, env()).await?;
    assert!(
        dispatcher.has_agent("crypto"),
        "crypto agent should be loaded"
    );
    let caps = dispatcher
        .capabilities_of("crypto")
        .expect("crypto metadata cached");
    assert!(caps.iter().any(|c| c.id == "hash"));
    assert!(caps.iter().any(|c| c.id == "hmac"));
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn dispatcher_invokes_crypto_hash() -> anyhow::Result<()> {
    let dir = agent_components_dir();
    if !has_crypto_wasm(&dir) {
        eprintln!("SKIP: crypto wasm not built");
        return Ok(());
    }

    let dispatcher = ComponentDispatcherService::from_dir(&dir, env()).await?;
    let result = dispatcher
        .test_capability(TestCapabilityRequest {
            tenant_id: "tenant-test".into(),
            agent_id: "crypto".into(),
            capability_id: "hash".into(),
            input: serde_json::json!({ "value": "hello" }),
            connection: None,
        })
        .await?;

    assert!(result.success, "expected success, got {:?}", result.error);
    let out = result.output.expect("output present");
    let hex = out.get("hex").and_then(|v| v.as_str()).expect("hex string");
    assert_eq!(
        hex,
        "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
    );
    assert!(result.execution_time_ms > 0.0);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn dispatcher_returns_guest_error_for_unknown_capability() -> anyhow::Result<()> {
    let dir = agent_components_dir();
    if !has_crypto_wasm(&dir) {
        eprintln!("SKIP: crypto wasm not built");
        return Ok(());
    }

    let dispatcher = ComponentDispatcherService::from_dir(&dir, env()).await?;
    let result = dispatcher
        .test_capability(TestCapabilityRequest {
            tenant_id: "tenant-test".into(),
            agent_id: "crypto".into(),
            capability_id: "no-such-thing".into(),
            input: serde_json::json!({}),
            connection: None,
        })
        .await?;

    assert!(!result.success);
    let err = result.error.expect("error envelope");
    assert_eq!(err.code, "UNKNOWN_CAPABILITY");
    assert_eq!(err.category, "permanent");
    Ok(())
}
