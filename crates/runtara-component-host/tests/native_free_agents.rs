//! Proves the compression and xlsx agents do their real work *inside* the wasm
//! sandbox — no `$RUNTARA_AGENT_SERVICE_URL`, no host handler, no network.
//!
//! Both agents used to be thin forwarders to a native host implementation
//! (`runtara_agents::{compression,xlsx}`) because their dependencies were
//! believed not to build for `wasm32-wasip2`. They do: `zip` needs
//! `default-features = false, features = ["deflate"]` to drop its optional
//! C-backed backends (which these capabilities never used), and `calamine` is
//! pure Rust already.
//!
//! The dispatcher below is configured with unroutable agent-service and proxy
//! URLs on purpose: if any capability still tried to forward, it would fail.
//!
//! Requires the components to be built — run `scripts/build-agent-components.sh`.

use std::path::PathBuf;

use base64::{Engine as _, engine::general_purpose::STANDARD};
use runtara_component_host::{ComponentDispatcherService, DispatcherEnv, TestCapabilityRequest};
use serde_json::{Value, json};

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

fn wasm_path(stem: &str) -> PathBuf {
    let p = workspace_root().join(format!("target/wasm32-wasip2/release/{stem}.wasm"));
    assert!(
        p.exists(),
        "requires {}; run scripts/build-agent-components.sh",
        p.display()
    );
    p
}

/// Bundle both agents (.wasm + sidecar meta.json) into one tmp dir, mirroring
/// the production layout.
fn build_bundle() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().expect("tempdir");

    for (stem, info) in [
        (
            "runtara_agent_compression",
            serde_json::to_vec_pretty(&runtara_agent_compression::agent_info()).unwrap(),
        ),
        (
            "runtara_agent_xlsx",
            serde_json::to_vec_pretty(&runtara_agent_xlsx::agent_info()).unwrap(),
        ),
    ] {
        std::fs::copy(wasm_path(stem), tmp.path().join(format!("{stem}.wasm")))
            .unwrap_or_else(|e| panic!("copy {stem}.wasm: {e}"));
        std::fs::write(tmp.path().join(format!("{stem}.meta.json")), info)
            .unwrap_or_else(|e| panic!("write {stem}.meta.json: {e}"));
    }

    tmp
}

/// Every URL points at a closed port. A surviving forward fails the test rather
/// than silently succeeding against a real host.
fn env() -> DispatcherEnv {
    DispatcherEnv {
        proxy_url: "http://127.0.0.1:1/unroutable".into(),
        agent_service_url: "http://127.0.0.1:1/unroutable".into(),
        object_model_url: "http://127.0.0.1:1/unroutable".into(),
        core_http_url: "http://127.0.0.1:1/unroutable".into(),
    }
}

async fn invoke(
    dispatcher: &ComponentDispatcherService,
    agent_id: &str,
    capability_id: &str,
    input: Value,
) -> anyhow::Result<Value> {
    let result = dispatcher
        .test_capability(TestCapabilityRequest {
            tenant_id: "tenant-test".into(),
            agent_id: agent_id.into(),
            capability_id: capability_id.into(),
            input,
            connection: None,
        })
        .await?;
    Ok(serde_json::to_value(result)?)
}

#[tokio::test(flavor = "multi_thread")]
async fn compression_round_trips_a_zip_inside_wasm() -> anyhow::Result<()> {
    let bundle = build_bundle();
    let dispatcher = ComponentDispatcherService::from_dir(bundle.path(), env()).await?;
    assert!(dispatcher.has_agent("compression"));

    let created = invoke(
        &dispatcher,
        "compression",
        "create-archive",
        json!({
            "files": [
                { "file": { "content": STANDARD.encode("hello"), "filename": "a.txt" } },
                { "file": { "content": STANDARD.encode("x,y"), "filename": "b.csv" } }
            ],
            "format": "zip",
            "compression_level": 6
        }),
    )
    .await?;
    println!("create-archive -> {created}");

    let archive_b64 = created
        .pointer("/output/content")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("no archive content in {created}"))
        .to_string();

    // A real zip: PK\x03\x04 magic, and it round-trips through the same sandbox.
    let raw = STANDARD.decode(&archive_b64).expect("archive is base64");
    assert_eq!(&raw[..4], b"PK\x03\x04", "not a zip archive");

    let listed = invoke(
        &dispatcher,
        "compression",
        "list-archive",
        json!({ "archive": { "content": archive_b64 } }),
    )
    .await?;
    println!("list-archive -> {listed}");
    assert_eq!(listed.pointer("/output/total_count"), Some(&json!(2)));

    let extracted = invoke(
        &dispatcher,
        "compression",
        "extract-file",
        json!({ "archive": { "content": archive_b64 }, "file_path": "b.csv" }),
    )
    .await?;
    println!("extract-file -> {extracted}");

    let content = extracted
        .pointer("/output/content")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("no extracted content in {extracted}"));
    assert_eq!(STANDARD.decode(content).unwrap(), b"x,y");

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn xlsx_parses_a_workbook_inside_wasm() -> anyhow::Result<()> {
    let bundle = build_bundle();
    let dispatcher = ComponentDispatcherService::from_dir(bundle.path(), env()).await?;
    assert!(dispatcher.has_agent("xlsx"));

    // Same fixture as the agent's own unit tests: one sheet "Orders",
    // header row + two data rows.
    let workbook = include_str!("fixtures/orders_xlsx.b64").trim();

    let sheets = invoke(
        &dispatcher,
        "xlsx",
        "get-sheets",
        json!({ "data": workbook }),
    )
    .await?;
    println!("get-sheets -> {sheets}");
    assert_eq!(sheets.pointer("/output/0/name"), Some(&json!("Orders")));
    assert_eq!(sheets.pointer("/output/0/rows"), Some(&json!(3)));

    let parsed = invoke(
        &dispatcher,
        "xlsx",
        "from-xlsx",
        json!({ "data": workbook, "has_headers": true }),
    )
    .await?;
    println!("from-xlsx -> {parsed}");
    assert_eq!(parsed.pointer("/output/0/sku"), Some(&json!("ABC-1")));
    assert_eq!(parsed.pointer("/output/0/qty"), Some(&json!(4)));
    assert_eq!(parsed.pointer("/output/1/sku"), Some(&json!("XYZ-9")));

    Ok(())
}
