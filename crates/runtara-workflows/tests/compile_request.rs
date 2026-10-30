// Copyright (C) 2026 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! A compile handed to a compiler elsewhere: the host makes the request, the
//! compiler compiles it with no filesystem, fetching only the components it
//! reads, and the host applies the outcome. The result is what compiling in
//! process makes.
//!
//! Needs the shared components (`RUNTARA_AGENT_COMPONENTS_DIR`, or
//! `target/wasm32-wasip2/release`); runs with `direct-wasm-integration-tests`.
#![cfg(feature = "direct-wasm-integration-tests")]

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use runtara_workflows::compile_request::{apply, apply_failure, compile, request};
use runtara_workflows::{CompilationInput, DirectWorkflowCompileOptions, compile_workflow_direct};

fn components_dir() -> PathBuf {
    std::env::var_os("RUNTARA_AGENT_COMPONENTS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/wasm32-wasip2/release")
        })
}

fn input(graph: serde_json::Value) -> CompilationInput {
    let catalog =
        runtara_dsl::agent_meta::AgentCatalog::from_meta_dir(&components_dir()).expect("catalog");
    CompilationInput {
        tenant_id: "acme".into(),
        workflow_id: "remote".into(),
        version: 3,
        execution_graph: serde_json::from_value(graph).expect("graph"),
        track_events: false,
        child_workflows: vec![],
        agent_catalog: Some(Arc::new(catalog)),
        progress_callback: None,
        agent_slug: None,
    }
}

fn options(output_dir: PathBuf, extra: PathBuf) -> DirectWorkflowCompileOptions {
    DirectWorkflowCompileOptions {
        output_dir,
        components_dir: components_dir(),
        extra_component_dirs: vec![extra],
        source_checksum: Some("source".into()),
    }
}

fn agent_graph() -> serde_json::Value {
    serde_json::json!({
        "name": "Remote",
        "entryPoint": "echo",
        "steps": {
            "echo": {"id": "echo", "stepType": "Agent", "agentId": "utils", "capabilityId": "return-input",
                     "inputMapping": {"value": {"valueType": "immediate", "value": 7}}},
            "finish": {"id": "finish", "stepType": "Finish",
                       "inputMapping": {"out": {"valueType": "reference", "value": "steps.echo.outputs"}}}
        },
        "executionPlan": [{"fromStep": "echo", "toStep": "finish"}],
        "variables": {}, "inputSchema": {}, "outputSchema": {}
    })
}

#[test]
fn a_compile_elsewhere_makes_what_one_in_process_does() {
    let temp = tempfile::tempdir().unwrap();
    let extra = temp.path().join("staged");
    let local = compile_workflow_direct(
        input(agent_graph()),
        options(temp.path().join("local"), extra.clone()),
    )
    .expect("in process");

    let remote_options = options(temp.path().join("remote"), extra);
    let (req, paths) = request(&input(agent_graph()), &remote_options).expect("request");
    // Over the wire, as JSON.
    let req = serde_json::from_slice(&serde_json::to_vec(&req).unwrap()).unwrap();
    let fetched = Arc::new(Mutex::new(BTreeSet::new()));
    let seen = fetched.clone();
    let (outcome, wasm) = compile(req, move |digest| {
        seen.lock().unwrap().insert(digest.to_owned());
        std::fs::read(&paths[digest])
    })
    .expect("compiled elsewhere");
    let outcome = serde_json::from_slice(&serde_json::to_vec(&outcome).unwrap()).unwrap();
    let remote = apply(outcome, &wasm, "remote", 3, &remote_options).expect("applied");

    assert_eq!(
        std::fs::read(&remote.binary_path).unwrap(),
        std::fs::read(&local.binary_path).unwrap()
    );
    assert_eq!(remote.binary_checksum, local.binary_checksum);
    assert_eq!(remote.package_size, local.package_size);
    assert_eq!(remote.trusted_pins, local.trusted_pins);
    assert_eq!(remote.default_variables, local.default_variables);
    for file in [
        "manifest.json",
        "artifact-metadata.json",
        "workflow-logic.wasm",
    ] {
        assert_eq!(
            std::fs::read(remote.build_dir.join(file)).unwrap(),
            std::fs::read(local.build_dir.join(file)).unwrap(),
            "{file}"
        );
    }
    let all = std::fs::read_dir(components_dir()).unwrap().count();
    let fetched = fetched.lock().unwrap().len();
    assert!(
        fetched > 0 && fetched < all,
        "only what it read: {fetched} of {all}"
    );
}

#[test]
fn a_failed_compile_elsewhere_fails_as_in_process() {
    let temp = tempfile::tempdir().unwrap();
    let mut graph = agent_graph();
    graph["steps"]["echo"]["agentId"] = serde_json::json!("no-such-agent");
    let opts = options(temp.path().join("remote"), temp.path().join("staged"));
    let local = compile_workflow_direct(input(graph.clone()), opts.clone()).unwrap_err();
    let (req, paths) = request(&input(graph), &opts).unwrap();
    let failure = compile(req, move |digest| std::fs::read(&paths[digest])).unwrap_err();
    let remote = apply_failure(failure, &opts);
    assert_eq!(remote.kind(), local.kind());
    assert_eq!(remote.to_string(), local.to_string());
}
