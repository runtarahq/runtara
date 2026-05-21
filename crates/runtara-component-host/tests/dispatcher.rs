//! End-to-end smoke test for `ComponentDispatcherService`.
//!
//! Builds a tmp `bundles/` directory by copying the freshly-built
//! `runtara_agent_crypto.wasm` and the source `meta.json` together, then runs
//! the dispatcher against it. Skipped if the .wasm is missing — run
//! `cargo component build --release --target wasm32-wasip2 -p
//! runtara-agent-crypto` first.

use std::path::{Path, PathBuf};

use runtara_component_host::{ComponentDispatcherService, DispatcherEnv, TestCapabilityRequest};

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

fn crypto_wasm_path() -> Option<PathBuf> {
    let p = workspace_root().join("target/wasm32-wasip2/release/runtara_agent_crypto.wasm");
    p.exists().then_some(p)
}

/// Build a one-agent bundle dir (mirrors the production layout) and return its
/// path. Returns `None` if the crypto .wasm hasn't been built yet. The
/// `meta.json` sidecar is emitted on the fly by calling the agent crate's
/// host-only `agent_info()` and serializing — same source-of-truth the
/// production `runtara-agent-bundle-emit` binary uses.
fn build_test_bundle() -> Option<tempfile::TempDir> {
    let wasm = crypto_wasm_path()?;
    let tmp = tempfile::tempdir().expect("tempdir");
    std::fs::copy(&wasm, tmp.path().join("runtara_agent_crypto.wasm")).expect("copy crypto.wasm");
    let info = runtara_agent_crypto::agent_info();
    let json = serde_json::to_vec_pretty(&info).expect("serialize crypto agent_info");
    std::fs::write(tmp.path().join("runtara_agent_crypto.meta.json"), json)
        .expect("write crypto.meta.json");
    Some(tmp)
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
    let Some(bundle) = build_test_bundle() else {
        eprintln!("SKIP: crypto wasm not built");
        return Ok(());
    };

    let dispatcher = ComponentDispatcherService::from_dir(bundle.path(), env()).await?;
    assert!(
        dispatcher.has_agent("crypto"),
        "crypto agent should be loaded"
    );
    let info = dispatcher
        .agent_info_of("crypto")
        .expect("crypto metadata cached");
    assert_eq!(info.id, "crypto");
    assert_eq!(info.name, "Crypto");
    assert!(info.capabilities.iter().any(|c| c.id == "hash"));
    assert!(info.capabilities.iter().any(|c| c.id == "hmac"));
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn dispatcher_invokes_crypto_hash() -> anyhow::Result<()> {
    let Some(bundle) = build_test_bundle() else {
        eprintln!("SKIP: crypto wasm not built");
        return Ok(());
    };

    let dispatcher = ComponentDispatcherService::from_dir(bundle.path(), env()).await?;
    let result = dispatcher
        .test_capability(TestCapabilityRequest {
            tenant_id: "tenant-test".into(),
            agent_id: "crypto".into(),
            capability_id: "hash".into(),
            input: serde_json::json!({ "data": "hello" }),
            connection: None,
        })
        .await?;

    assert!(result.success, "expected success, got {:?}", result.error);
    let out = result.output.expect("output present");
    let hash = out
        .get("hash")
        .and_then(|v| v.as_str())
        .expect("hash string");
    assert_eq!(
        hash,
        "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
    );
    assert_eq!(
        out.get("algorithm").and_then(|v| v.as_str()),
        Some("sha256")
    );
    assert_eq!(out.get("format").and_then(|v| v.as_str()), Some("hex"));
    assert!(result.execution_time_ms > 0.0);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn dispatcher_returns_guest_error_for_unknown_capability() -> anyhow::Result<()> {
    let Some(bundle) = build_test_bundle() else {
        eprintln!("SKIP: crypto wasm not built");
        return Ok(());
    };

    let dispatcher = ComponentDispatcherService::from_dir(bundle.path(), env()).await?;
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

/// Drift detector: every capability declared in meta.json must be reachable
/// via `invoke(cap_id, ...)`. Catches the "added a capability to JSON but
/// forgot the match arm" class of bug — and conversely "added a capability
/// to Rust but forgot to declare it in meta.json".
///
/// We feed a deliberately-malformed input (`null` / `{}`) and accept anything
/// that ISN'T `UNKNOWN_CAPABILITY` as proof the dispatch arm exists. A
/// genuine input validation error (`INPUT_DESERIALIZATION_ERROR` or similar)
/// means the cap_id was routed correctly.
#[tokio::test(flavor = "multi_thread")]
async fn dispatcher_drift_detector_every_declared_cap_is_routed() -> anyhow::Result<()> {
    let Some(bundle) = build_test_bundle() else {
        eprintln!("SKIP: crypto wasm not built");
        return Ok(());
    };

    let dispatcher = ComponentDispatcherService::from_dir(bundle.path(), env()).await?;

    for agent_id in dispatcher
        .agent_ids()
        .map(str::to_string)
        .collect::<Vec<_>>()
    {
        let info = dispatcher
            .agent_info_of(&agent_id)
            .expect("dispatcher knows about this agent");
        let cap_ids: Vec<String> = info.capabilities.iter().map(|c| c.id.clone()).collect();

        for capability_id in cap_ids {
            let result = dispatcher
                .test_capability(TestCapabilityRequest {
                    tenant_id: "tenant-drift".into(),
                    agent_id: agent_id.clone(),
                    capability_id: capability_id.clone(),
                    input: serde_json::json!({}),
                    connection: None,
                })
                .await?;

            if let Some(err) = result.error {
                assert_ne!(
                    err.code, "UNKNOWN_CAPABILITY",
                    "agent `{agent_id}` declares capability `{capability_id}` in meta.json but the .wasm does not route it (UNKNOWN_CAPABILITY)"
                );
            }
        }
    }
    Ok(())
}

/// Production bundle-shaped load: point the dispatcher at
/// `target/wasm32-wasip2/release/` (where the bundle script stages all 23
/// `.wasm` + `.meta.json` pairs), verify every loaded capability is routed,
/// and assert the count matches expectations. Skipped if the bundle hasn't
/// been built — run `scripts/build-agent-components.sh` first.
#[tokio::test(flavor = "multi_thread")]
async fn dispatcher_loads_full_production_bundle() -> anyhow::Result<()> {
    let bundle_dir = workspace_root().join("target/wasm32-wasip2/release");
    let crypto_wasm = bundle_dir.join("runtara_agent_crypto.wasm");
    let crypto_meta = bundle_dir.join("runtara_agent_crypto.meta.json");
    if !crypto_wasm.exists() || !crypto_meta.exists() {
        eprintln!("SKIP: bundle not built — run scripts/build-agent-components.sh first");
        return Ok(());
    }

    let dispatcher = ComponentDispatcherService::from_dir(&bundle_dir, env()).await?;
    let loaded: Vec<String> = dispatcher
        .agent_ids()
        .map(str::to_string)
        .collect::<Vec<_>>();
    assert!(
        loaded.len() >= 23,
        "expected at least 23 agents in the bundle, got {}: {:?}",
        loaded.len(),
        loaded
    );

    for agent_id in &loaded {
        let info = dispatcher
            .agent_info_of(agent_id)
            .expect("dispatcher knows about this agent");
        for cap in &info.capabilities {
            let result = dispatcher
                .test_capability(TestCapabilityRequest {
                    tenant_id: "tenant-full-bundle".into(),
                    agent_id: agent_id.clone(),
                    capability_id: cap.id.clone(),
                    input: serde_json::json!({}),
                    connection: None,
                })
                .await?;
            if let Some(err) = result.error {
                assert_ne!(
                    err.code, "UNKNOWN_CAPABILITY",
                    "agent `{agent_id}` declares capability `{}` in meta.json but the .wasm does not route it (UNKNOWN_CAPABILITY)",
                    cap.id
                );
            }
        }
    }
    Ok(())
}

#[allow(dead_code)]
fn _bundle_path_unused(_: &Path) {}
