// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Tier A — exhaustive compile smoke battery over every workflow fixture.
//!
//! When the generated (Rust-codegen) compiler was deleted, the A/B parity suite
//! that exercised these fixtures went with it. This battery is its replacement:
//! it walks `tests/fixtures/` and asserts that EVERY fixture lowers through the
//! direct WebAssembly emitter to the outcome we expect — a valid component, or a
//! deliberate rejection. It needs no wasmtime and no staged components, so it
//! runs everywhere and is the fast safety net under the (gated) execution
//! battery in `direct_wasm_execute.rs`.
//!
//! Each fixture is classified automatically so new fixtures can't slip in
//! untested:
//!
//! - `EMBED` — has an `EmbedWorkflow` step. Standalone emit must *require*
//!   children (proving the embed contract); the full child-wired lowering is
//!   asserted in `src/direct_wasm/compile/tests.rs`.
//! - `KNOWN_UNSUPPORTED` — graph shapes the direct emitter genuinely cannot
//!   lower (e.g. an arbitrary back-edge cycle rather than a `While`/`maxRetries`
//!   loop). Currently empty — every corpus fixture validates and emits — but
//!   kept as a conscious escape hatch so a future unsupported shape is an
//!   explicit table edit, not a silent runtime failure.
//! - `VALID` — everything else: must emit a component that validates.
//!
//! The completeness guard at the end fails if any fixture file went unvisited.

use std::fs;
use std::path::PathBuf;

use runtara_workflows::ExecutionGraph;
use runtara_workflows::direct_wasm::{DirectCompilationInput, compile_direct_workflow};
use wasmparser::Validator;

/// Graph shapes the direct emitter genuinely cannot lower (it linearizes any
/// DAG — diamonds included — but not an arbitrary back-edge cycle; structured
/// loops use `While`/Agent `maxRetries`). Currently empty: the whole corpus
/// validates and emits. Kept as an explicit escape hatch — adding a fixture
/// here must be a conscious decision, asserted to emit-fail, not a silent gap.
const KNOWN_UNSUPPORTED: &[&str] = &[];

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
}

/// Load a fixture as an `ExecutionGraph`, unwrapping the `{ "executionGraph": .. }`
/// envelope some embed-input fixtures use.
fn load_graph(json: &str) -> Result<ExecutionGraph, String> {
    let value: serde_json::Value = serde_json::from_str(json).map_err(|e| e.to_string())?;
    let graph_value = value
        .get("executionGraph")
        .cloned()
        .unwrap_or_else(|| value.clone());
    serde_json::from_value(graph_value).map_err(|e| e.to_string())
}

fn has_embed_step(graph: &ExecutionGraph) -> bool {
    graph
        .steps
        .values()
        .any(|s| matches!(s, runtara_dsl::Step::EmbedWorkflow(_)))
}

fn emit(graph: ExecutionGraph) -> Result<PathBuf, String> {
    let temp = tempfile::tempdir().map_err(|e| format!("tempdir: {e}"))?;
    let result = compile_direct_workflow(DirectCompilationInput {
        workflow_id: "smoke/fixture".to_string(),
        version: 1,
        source_checksum: None,
        execution_graph: graph,
        child_workflows: vec![],
        output_dir: temp.path().to_path_buf(),
        track_events: false,
        agent_catalog: None,
        agent_slug: None,
    })
    .map_err(|e| e.to_string())?;
    if !result.support_report.supported {
        return Err(format!(
            "support report unsupported: {:?}",
            result.support_report.unsupported
        ));
    }
    // Keep the wasm bytes alive past `temp` by reading them out.
    let wasm = fs::read(&result.wasm_path).map_err(|e| format!("read wasm: {e}"))?;
    Validator::new_with_features(wasmparser::WasmFeatures::all())
        .validate_all(&wasm)
        .map_err(|e| format!("wasm did not validate as a component: {e}"))?;
    Ok(result.wasm_path)
}

/// One assertion per fixture, accumulating failures so a single run reports the
/// full picture instead of stopping at the first bad fixture.
fn check(name: &str, json: &str) -> Result<(), String> {
    let graph = match load_graph(json) {
        Ok(g) => g,
        Err(e) => {
            // Only KNOWN_UNSUPPORTED is allowed to be unparseable-by-design; any
            // other parse failure is a real regression.
            return Err(format!("failed to parse as ExecutionGraph: {e}"));
        }
    };

    if has_embed_step(&graph) {
        // Embed parents must declare their need for children: standalone emit
        // (no children supplied) must fail. The child-wired success path is
        // covered in src/direct_wasm/compile/tests.rs.
        return match emit(graph) {
            Err(_) => Ok(()),
            Ok(_) => Err(
                "EmbedWorkflow fixture emitted with NO child workflows supplied; \
                 expected it to require children"
                    .to_string(),
            ),
        };
    }

    if KNOWN_UNSUPPORTED.contains(&name) {
        return match emit(graph) {
            Err(_) => Ok(()),
            Ok(_) => Err(
                "fixture is listed KNOWN_UNSUPPORTED but now emits a valid component \
                 — the direct emitter gained support; move it to VALID"
                    .to_string(),
            ),
        };
    }

    // VALID: must emit a component that validates.
    emit(graph).map(|_| ())
}

#[test]
fn every_fixture_lowers_as_expected() {
    let mut entries: Vec<_> = fs::read_dir(fixtures_dir())
        .expect("read fixtures dir")
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().map(|x| x == "json").unwrap_or(false))
        .collect();
    entries.sort();
    assert!(
        entries.len() >= 100,
        "expected the full fixture corpus (>=100), found {} — did the fixtures dir move?",
        entries.len()
    );

    let mut failures: Vec<String> = Vec::new();
    let mut counts = (0usize, 0usize, 0usize); // (valid, embed, known_unsupported)

    for path in &entries {
        let name = path.file_stem().unwrap().to_string_lossy().to_string();
        let json = fs::read_to_string(path).expect("read fixture");

        // Tally category for the summary line (cheap re-parse; ignore errors here).
        if let Ok(g) = load_graph(&json) {
            if has_embed_step(&g) {
                counts.1 += 1;
            } else if KNOWN_UNSUPPORTED.contains(&name.as_str()) {
                counts.2 += 1;
            } else {
                counts.0 += 1;
            }
        }

        if let Err(why) = check(&name, &json) {
            failures.push(format!(
                "  {}: {}",
                path.file_name().unwrap().to_string_lossy(),
                why
            ));
        }
    }

    eprintln!(
        "fixture smoke: {} fixtures — {} VALID, {} EMBED, {} KNOWN_UNSUPPORTED",
        entries.len(),
        counts.0,
        counts.1,
        counts.2,
    );

    assert!(
        failures.is_empty(),
        "{} fixture(s) did not lower as expected:\n{}",
        failures.len(),
        failures.join("\n"),
    );
}
