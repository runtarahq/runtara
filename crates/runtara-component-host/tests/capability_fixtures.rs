//! Fixture-driven capability smoke harness.
//!
//! Every `tests/fixtures/<agent_id>/<capability_id>.json` describes one
//! happy-path call: the input the dispatcher should accept and what the
//! returned output must contain. The runner walks the fixtures directory,
//! loads the production bundle (`target/wasm32-wasip1/release/`), and
//! invokes each fixture through `ComponentDispatcherService::test_capability`.
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
    workspace_root().join("target/wasm32-wasip1/release")
}

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
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

fn load_all_fixtures(root: &Path) -> Vec<LoadedFixture> {
    let mut out = Vec::new();
    let Ok(agents) = fs::read_dir(root) else {
        return out;
    };
    for agent_entry in agents.flatten() {
        if !agent_entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        let agent_id = agent_entry.file_name().to_string_lossy().to_string();
        for fixture_entry in fs::read_dir(agent_entry.path())
            .into_iter()
            .flatten()
            .flatten()
        {
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

    let fixtures = load_all_fixtures(&fixtures_dir());
    assert!(
        !fixtures.is_empty(),
        "no fixtures found under {}",
        fixtures_dir().display()
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
