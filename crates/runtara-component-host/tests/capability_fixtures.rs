//! Fixture-driven capability smoke harness.
//!
//! Fixtures live next to the agent they exercise:
//!   `crates/agents/runtara-agent-<x>/fixtures/<capability_id>.json`.
//! Each one describes a happy-path call: the input the dispatcher should
//! accept and what the returned output must contain. The runner walks every
//! `crates/agents/runtara-agent-*/fixtures/` directory, loads the production bundle
//! (`target/wasm32-wasip2/release/`), and invokes each fixture through
//! `ComponentDispatcherService::test_capability`.
//!
//! Keeping fixtures inside the agent crate keeps each agent self-contained —
//! moving or extracting an agent crate carries its tests + fixtures along.
//! The harness itself stays centralized here because driving a WASM component
//! requires the dispatcher (wasmtime + wac compose plumbing), which is too
//! heavy to instantiate per agent crate.
//!
//! This complements the per-component unit tests in each `runtara-agent-<x>`
//! crate: the unit tests cover capability logic in isolation, this runner
//! proves the same logic still works after going through `wac compose`, the
//! WIT `invoke(cap_id, ...)` shape, and JSON marshalling on both sides.
//!
//! Skipped when the bundle isn't built; run
//! `scripts/build-agent-components.sh` first.
//!
//! Fixture format:
//! ```json
//! {
//!   "input": { ... },           // JSON value passed verbatim to test_capability
//!   "expect": {
//!     "success": true,          // required: assert result.success matches
//!     "output_contains": {...}, // optional: assert these fields appear in output
//!     "output_equals": "..."    // optional: assert output equals this value
//!   }
//! }
//! ```
//!
//! Adding a fixture: drop `<capability>.json` into your agent crate's
//! `fixtures/` directory. The filename stem (without `.json`) must be the
//! capability id exactly as it appears in `meta.json`. The agent_id is
//! looked up from that crate's staged `meta.json`, so hyphen↔underscore
//! variation between crate names and agent ids is handled automatically.

use std::fs;
use std::path::{Path, PathBuf};

use runtara_component_host::{ComponentDispatcherService, DispatcherEnv, TestCapabilityRequest};
use serde::Deserialize;
use serde_json::Value;

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

fn bundle_dir() -> PathBuf {
    workspace_root().join("target/wasm32-wasip2/release")
}

fn agents_crates_dir() -> PathBuf {
    workspace_root().join("crates").join("agents")
}

fn env() -> DispatcherEnv {
    DispatcherEnv {
        proxy_url: "http://localhost:9999".into(),
        agent_service_url: "http://localhost:9998".into(),
        object_model_url: "http://localhost:9997".into(),
        core_http_url: "http://localhost:9996".into(),
    }
}

#[derive(Debug, Deserialize)]
struct Fixture {
    input: Value,
    expect: Expect,
}

#[derive(Debug, Deserialize)]
struct Expect {
    success: bool,
    #[serde(default)]
    output_contains: Option<Value>,
    #[serde(default)]
    output_equals: Option<Value>,
}

/// One fixture file = one (agent_id, capability_id, Fixture) triple.
struct LoadedFixture {
    agent_id: String,
    capability_id: String,
    path: PathBuf,
    fixture: Fixture,
}

/// For a crate named `runtara-agent-<x>`, the matching wasm artifact is
/// `runtara_agent_<x with underscores>.wasm`, and its meta.json sits next to
/// it carrying the authoritative `id` field. Looking up the agent_id this way
/// keeps the fixture mapping correct even when the crate name and agent id
/// differ in punctuation (e.g. `runtara-agent-ai-tools` → `ai-tools`).
fn agent_id_for_crate(crate_dir_name: &str, bundle: &Path) -> Option<String> {
    let suffix = crate_dir_name.strip_prefix("runtara-agent-")?;
    let wasm_stem = format!("runtara_agent_{}", suffix.replace('-', "_"));
    let meta_path = bundle.join(format!("{wasm_stem}.meta.json"));
    let body = fs::read_to_string(&meta_path).ok()?;
    let parsed: Value = serde_json::from_str(&body).ok()?;
    parsed
        .get("id")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

fn load_all_fixtures(crates_root: &Path, bundle: &Path) -> Vec<LoadedFixture> {
    let mut out = Vec::new();
    let Ok(entries) = fs::read_dir(crates_root) else {
        return out;
    };
    for crate_entry in entries.flatten() {
        let crate_name = crate_entry.file_name().to_string_lossy().to_string();
        if !crate_name.starts_with("runtara-agent-") {
            continue;
        }
        let fixtures = crate_entry.path().join("fixtures");
        if !fixtures.is_dir() {
            continue;
        }
        let Some(agent_id) = agent_id_for_crate(&crate_name, bundle) else {
            // No meta.json for this crate's wasm — bundle wasn't built for it.
            // Skip silently; the bundle-presence check will report up top.
            continue;
        };
        for fixture_entry in fs::read_dir(&fixtures).into_iter().flatten().flatten() {
            let path = fixture_entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let capability_id = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_string();
            let body = fs::read_to_string(&path).expect("read fixture");
            let fixture: Fixture = serde_json::from_str(&body)
                .unwrap_or_else(|e| panic!("parse fixture {}: {e}", path.display()));
            out.push(LoadedFixture {
                agent_id: agent_id.clone(),
                capability_id,
                path,
                fixture,
            });
        }
    }
    out
}

fn assert_output_matches(output: &Value, expect: &Expect, fixture_path: &Path) {
    if let Some(eq) = &expect.output_equals {
        assert_eq!(
            output,
            eq,
            "fixture {} output_equals mismatch",
            fixture_path.display()
        );
    }
    if let Some(contains) = &expect.output_contains {
        let Value::Object(want) = contains else {
            panic!(
                "fixture {} output_contains must be an object",
                fixture_path.display()
            );
        };
        let Value::Object(got) = output else {
            panic!(
                "fixture {} expected object output, got {}",
                fixture_path.display(),
                output
            );
        };
        for (k, v) in want {
            let actual = got.get(k).unwrap_or_else(|| {
                panic!(
                    "fixture {} output missing key '{k}' (got {})",
                    fixture_path.display(),
                    Value::Object(got.clone())
                )
            });
            assert_eq!(
                actual,
                v,
                "fixture {} output key '{k}' mismatch",
                fixture_path.display()
            );
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn capability_fixtures_drive_production_bundle() -> anyhow::Result<()> {
    let bundle = bundle_dir();
    // Use any one of the wasm files as a probe — if it's missing the bundle
    // hasn't been built and the test is skipped.
    if !bundle.join("runtara_agent_crypto.wasm").exists() {
        eprintln!(
            "SKIP: bundle not built at {} — run scripts/build-agent-components.sh first",
            bundle.display()
        );
        return Ok(());
    }

    let fixtures = load_all_fixtures(&agents_crates_dir(), &bundle);
    assert!(
        !fixtures.is_empty(),
        "no fixtures found under crates/agents/runtara-agent-*/fixtures/"
    );

    let dispatcher = ComponentDispatcherService::from_dir(&bundle, env()).await?;

    for LoadedFixture {
        agent_id,
        capability_id,
        path,
        fixture,
    } in &fixtures
    {
        let result = dispatcher
            .test_capability(TestCapabilityRequest {
                tenant_id: "tenant-fixture".into(),
                agent_id: agent_id.clone(),
                capability_id: capability_id.clone(),
                input: fixture.input.clone(),
                connection: None,
            })
            .await?;

        assert_eq!(
            result.success,
            fixture.expect.success,
            "fixture {} expected success={} but got success={} error={:?}",
            path.display(),
            fixture.expect.success,
            result.success,
            result.error
        );

        if fixture.expect.success {
            let output = result
                .output
                .as_ref()
                .unwrap_or_else(|| panic!("fixture {} succeeded but no output", path.display()));
            assert_output_matches(output, &fixture.expect, path);
        }
    }

    Ok(())
}
