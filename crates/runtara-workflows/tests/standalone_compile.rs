// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! End-to-end tests for the `runtara-compile` CLI binary.
//!
//! Validation and child-resolution behavior runs unconditionally. The full
//! compile test composes against real stdlib/runtime components, so it skips
//! (with a note) when no components directory is available — point
//! `RUNTARA_AGENT_COMPONENTS_DIR` at a built components dir or build
//! `target/wasm32-wasip2/release` first.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_runtara-compile")
}

fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Components dir with real stdlib/runtime components, or `None` to skip.
fn shared_components_dir() -> Option<PathBuf> {
    let dir = std::env::var_os("RUNTARA_AGENT_COMPONENTS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| workspace_root().join("target/wasm32-wasip2/release"));
    let required = [
        "runtara_workflow_stdlib.wasm",
        "runtara_workflow_runtime.wasm",
    ];
    required
        .iter()
        .all(|f| dir.join(f).is_file())
        .then_some(dir)
}

fn run(cli_args: &[&str]) -> Output {
    Command::new(bin())
        .args(cli_args)
        .output()
        .expect("runtara-compile should spawn")
}

fn stderr_of(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn stdout_of(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

#[test]
fn validate_only_passes_through_nested_children() {
    // Validation needs a components dir only as the agent-catalog source; an
    // empty dir is a valid (agent-less) catalog for an agent-less workflow.
    // Root is the nested *child* fixture (it has a proper inputSchema), so
    // child-aware validation runs through its grandchild and
    // great-grandchild.
    let components = tempfile::tempdir().expect("tempdir");
    let workflow = fixture_path("embed_workflow_nested_child.json");

    let output = run(&[
        "--workflow",
        workflow.to_str().unwrap(),
        "--components-dir",
        components.path().to_str().unwrap(),
        "--child",
        &format!(
            "grandchild_workflow={}",
            fixture_path("embed_workflow_nested_grandchild.json").display()
        ),
        "--child",
        &format!(
            "great_grandchild_workflow={}",
            fixture_path("embed_workflow_nested_great_grandchild.json").display()
        ),
        "--validate",
    ]);

    assert!(
        output.status.success(),
        "expected success, stderr: {}",
        stderr_of(&output)
    );
    assert!(stdout_of(&output).contains("child workflow(s) are valid"));
}

#[test]
fn validation_error_deep_in_a_child_blocks_compile_with_attribution() {
    // Corrupt the great-grandchild by dropping its inputSchema: its own
    // data.* reference becomes invalid (E052) and the grandchild's mapping
    // toward it becomes E054 — both two levels below the root, both must be
    // attributed to the child workflow they live in.
    let components = tempfile::tempdir().expect("tempdir");
    let broken_dir = tempfile::tempdir().expect("tempdir");
    let mut broken: serde_json::Value = serde_json::from_slice(
        &std::fs::read(fixture_path("embed_workflow_nested_great_grandchild.json")).unwrap(),
    )
    .unwrap();
    broken.as_object_mut().unwrap().remove("inputSchema");
    let broken_path = broken_dir.path().join("broken_great_grandchild.json");
    std::fs::write(&broken_path, serde_json::to_vec(&broken).unwrap()).unwrap();

    let output = run(&[
        "--workflow",
        fixture_path("embed_workflow_nested_child.json")
            .to_str()
            .unwrap(),
        "--components-dir",
        components.path().to_str().unwrap(),
        "--child",
        &format!(
            "grandchild_workflow={}",
            fixture_path("embed_workflow_nested_grandchild.json").display()
        ),
        "--child",
        &format!("great_grandchild_workflow={}", broken_path.display()),
    ]);

    assert!(
        !output.status.success(),
        "deep child error must block compile"
    );
    let stderr = stderr_of(&output);
    assert!(
        stderr.contains("in child workflow 'grandchild_workflow'"),
        "stderr: {stderr}"
    );
    assert!(
        stderr.contains("in child workflow 'great_grandchild_workflow'"),
        "stderr: {stderr}"
    );
    assert!(stderr.contains("Validation failed"), "stderr: {stderr}");
}

#[test]
fn missing_child_workflow_is_an_actionable_error() {
    let components = tempfile::tempdir().expect("tempdir");
    let workflow = fixture_path("embed_workflow_nested_parent.json");

    let output = run(&[
        "--workflow",
        workflow.to_str().unwrap(),
        "--components-dir",
        components.path().to_str().unwrap(),
        "--validate",
    ]);

    assert!(!output.status.success(), "missing child must fail");
    let stderr = stderr_of(&output);
    assert!(stderr.contains("child_workflow"), "stderr: {stderr}");
    assert!(stderr.contains("--child"), "stderr: {stderr}");
}

#[test]
fn missing_grandchild_workflow_is_detected_through_the_child() {
    // Provide the direct child but not the grandchild it embeds: resolution
    // must follow the nesting and name the grandchild reference.
    let components = tempfile::tempdir().expect("tempdir");
    let parent = fixture_path("embed_workflow_nested_parent.json");
    let child = fixture_path("embed_workflow_nested_child.json");

    let output = run(&[
        "--workflow",
        parent.to_str().unwrap(),
        "--components-dir",
        components.path().to_str().unwrap(),
        "--child",
        &format!("child_workflow={}", child.display()),
        "--validate",
    ]);

    assert!(!output.status.success(), "missing grandchild must fail");
    let stderr = stderr_of(&output);
    assert!(stderr.contains("grandchild_workflow"), "stderr: {stderr}");
    assert!(stderr.contains("call_grandchild"), "stderr: {stderr}");
}

#[test]
fn compiles_deeply_nested_child_workflows_to_composed_wasm() {
    let Some(components_dir) = shared_components_dir() else {
        eprintln!("skipping: no components dir with stdlib/runtime wasm available");
        return;
    };
    let temp = tempfile::tempdir().expect("tempdir");
    let output_wasm = temp.path().join("parent.wasm");
    let build_dir = temp.path().join("builds");

    let parent = fixture_path("embed_workflow_nested_parent.json");
    let child = format!(
        "child_workflow={}",
        fixture_path("embed_workflow_nested_child.json").display()
    );
    let grandchild = format!(
        "grandchild_workflow={}",
        fixture_path("embed_workflow_nested_grandchild.json").display()
    );
    let great_grandchild = format!(
        "great_grandchild_workflow={}",
        fixture_path("embed_workflow_nested_great_grandchild.json").display()
    );

    let output = run(&[
        "--workflow",
        parent.to_str().unwrap(),
        "--workflow-id",
        "nested-parent",
        "--components-dir",
        components_dir.to_str().unwrap(),
        "--child",
        &child,
        "--child",
        &grandchild,
        "--child",
        &great_grandchild,
        "--build-dir",
        build_dir.to_str().unwrap(),
        "--output",
        output_wasm.to_str().unwrap(),
        // The committed emitter fixtures intentionally reference data.*
        // without an inputSchema, which the platform validator rejects.
        "--no-validate",
        "--verbose",
    ]);

    assert!(
        output.status.success(),
        "compile failed.\nstdout: {}\nstderr: {}",
        stdout_of(&output),
        stderr_of(&output)
    );

    // The composed artifact is a valid component.
    let wasm = std::fs::read(&output_wasm).expect("output wasm exists");
    wasmparser::Validator::new()
        .validate_all(&wasm)
        .expect("composed workflow.wasm should validate");

    // The artifact metadata records the full flat child closure:
    // child, grandchild, AND great-grandchild.
    let metadata_path = build_dir
        .join("nested-parent-v1-direct")
        .join("artifact-metadata.json");
    let metadata: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&metadata_path).expect("artifact metadata exists"))
            .expect("artifact metadata parses");
    let children = metadata["childWorkflows"]
        .as_array()
        .expect("childWorkflows array");
    let ids: HashMap<&str, &serde_json::Value> = children
        .iter()
        .map(|c| (c["workflowId"].as_str().unwrap(), c))
        .collect();
    assert_eq!(children.len(), 3, "metadata: {metadata:#}");
    assert!(ids.contains_key("child_workflow"));
    assert!(ids.contains_key("grandchild_workflow"));
    assert!(ids.contains_key("great_grandchild_workflow"));

    let stdout = stdout_of(&output);
    assert!(stdout.contains("children:"), "stdout: {stdout}");
    assert!(
        stdout.contains("great_grandchild_workflow"),
        "stdout: {stdout}"
    );
}

#[test]
fn analyze_reports_supported_for_nested_embed() {
    let components = tempfile::tempdir().expect("tempdir");
    let parent = fixture_path("embed_workflow_nested_parent.json");

    let output = run(&[
        "--workflow",
        parent.to_str().unwrap(),
        "--components-dir",
        components.path().to_str().unwrap(),
        "--child",
        &format!(
            "child_workflow={}",
            fixture_path("embed_workflow_nested_child.json").display()
        ),
        "--child",
        &format!(
            "grandchild_workflow={}",
            fixture_path("embed_workflow_nested_grandchild.json").display()
        ),
        "--child",
        &format!(
            "great_grandchild_workflow={}",
            fixture_path("embed_workflow_nested_great_grandchild.json").display()
        ),
        "--no-validate",
        "--analyze",
    ]);

    assert!(
        output.status.success(),
        "analyze failed, stderr: {}",
        stderr_of(&output)
    );
    let report: serde_json::Value =
        serde_json::from_str(&stdout_of(&output)).expect("support report is JSON");
    assert_eq!(report["supported"], serde_json::Value::Bool(true));
}
