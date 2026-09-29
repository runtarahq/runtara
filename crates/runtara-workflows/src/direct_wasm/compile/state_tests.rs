// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Hermetic compiler coverage of SetState / GetState: the published/local
//! mode each compile picks, and the `runtara:workflow/state` import only where
//! a step publishes.
use super::*;
use crate::direct_wasm::component::WorkflowRole;
use crate::direct_wasm::manifest::DirectStateMode;
use serde_json::{Value, json};

fn schema() -> Value {
    json!({"stage": {"type": "string"}, "count": {"type": "integer"}})
}

/// `set -> get -> finish`, durable unless told otherwise.
fn state_graph(durable: bool) -> Value {
    json!({
        "entryPoint": "set",
        "durable": durable,
        "stateSchema": schema(),
        "steps": {
            "set": {"id": "set", "stepType": "SetState", "values": {
                "stage": {"valueType": "immediate", "value": "approval"},
                "count": {"valueType": "reference", "value": "data.count"}
            }},
            "get": {"id": "get", "stepType": "GetState"},
            "finish": {"id": "finish", "stepType": "Finish", "inputMapping": {
                "stage": {"valueType": "reference", "value": "steps.get.outputs.stage"}
            }}
        },
        "executionPlan": [
            {"fromStep": "set", "toStep": "get"},
            {"fromStep": "get", "toStep": "finish"}
        ]
    })
}

fn embedding_parent() -> Value {
    json!({
        "entryPoint": "embed",
        "steps": {
            "embed": {"id": "embed", "stepType": "EmbedWorkflow",
                "childWorkflowId": "child", "childVersion": "latest"},
            "finish": {"id": "finish", "stepType": "Finish"}
        },
        "executionPlan": [{"fromStep": "embed", "toStep": "finish"}]
    })
}

fn compile(
    graph: Value,
    children: Vec<crate::ChildWorkflowInput>,
    abi: WorkflowRole,
) -> DirectCompilationResult {
    thread_local! {
        static DIRS: std::cell::RefCell<Vec<tempfile::TempDir>> = const {
            std::cell::RefCell::new(Vec::new())
        };
    }
    let dir = tempfile::tempdir().expect("tempdir");
    let result = compile_direct_workflow_with_abi(
        DirectCompilationInput {
            workflow_id: "state".into(),
            version: 1,
            source_checksum: None,
            execution_graph: serde_json::from_value(graph).expect("graph"),
            child_workflows: children,
            output_dir: dir.path().into(),
            track_events: true,
            agent_catalog: None,
            agent_slug: None,
        },
        abi,
        false,
    )
    .expect("compiles");
    DIRS.with(|dirs| dirs.borrow_mut().push(dir));
    result
}

fn imports_state(result: &DirectCompilationResult) -> bool {
    let world = fs::read_to_string(&result.world_wit_path).unwrap();
    let logic = fs::read(&result.workflow_logic_wasm_path).unwrap();
    let in_logic = logic
        .windows(runtara_wit::workflow::STATE.len())
        .any(|window| window == runtara_wit::workflow::STATE.as_bytes());
    assert_eq!(world.contains(runtara_wit::workflow::STATE), in_logic);
    assert_eq!(result.component_artifacts.run_state, in_logic);
    in_logic
}

#[test]
fn a_durable_top_level_run_publishes_through_the_host() {
    assert!(imports_state(&compile(
        state_graph(true),
        vec![],
        WorkflowRole::Root
    )));
}

#[test]
fn local_state_never_imports_the_host_interface() {
    // A non-durable workflow keeps its state local.
    assert!(!imports_state(&compile(
        state_graph(false),
        vec![],
        WorkflowRole::Root
    )));
    // So does a published workflow-agent, which runs in its caller's
    // instance.
    assert!(!imports_state(&compile(
        state_graph(true),
        vec![],
        WorkflowRole::PublishedAgent
    )));
    // And an embedded child.
    let child = crate::ChildWorkflowInput {
        step_id: "embed".into(),
        workflow_id: "child".into(),
        version_requested: "latest".into(),
        version_resolved: 1,
        execution_graph: serde_json::from_value(state_graph(true)).unwrap(),
    };
    assert!(!imports_state(&compile(
        embedding_parent(),
        vec![child],
        WorkflowRole::Root
    )));
}

#[test]
fn state_steps_compile_in_loops_splits_and_on_error_routes() {
    let mut graph = json!({
        "entryPoint": "loop",
        "stateSchema": schema(),
        "steps": {
            "loop": {"id": "loop", "stepType": "Split",
                "config": {"value": {"valueType": "reference", "value": "data.items"}},
                "subgraph": state_graph(true)},
            "risky": {"id": "risky", "stepType": "SetState", "values": {
                "count": {"valueType": "reference", "value": "data.count"}}},
            "recover": {"id": "recover", "stepType": "SetState", "values": {
                "stage": {"valueType": "immediate", "value": "failed"}}},
            "finish": {"id": "finish", "stepType": "Finish"}
        },
        "executionPlan": [
            {"fromStep": "loop", "toStep": "risky"},
            {"fromStep": "risky", "toStep": "finish"},
            {"fromStep": "risky", "toStep": "recover", "label": "onError"},
            {"fromStep": "recover", "toStep": "finish"}
        ]
    });
    // The Split body carries no schema of its own: the root's governs it.
    graph["steps"]["loop"]["subgraph"]
        .as_object_mut()
        .unwrap()
        .remove("stateSchema");
    for abi in [WorkflowRole::Root, WorkflowRole::PublishedAgent] {
        compile(graph.clone(), vec![], abi);
    }
    graph["durable"] = json!(false);
    compile(graph, vec![], WorkflowRole::Root);
}

#[test]
fn configure_run_state_publishes_only_the_durable_root() {
    let child_graph: runtara_dsl::ExecutionGraph =
        serde_json::from_value(state_graph(true)).unwrap();
    let parent: runtara_dsl::ExecutionGraph = serde_json::from_value(embedding_parent()).unwrap();
    let mut manifest =
        crate::direct_wasm::manifest::build_direct_workflow_manifest_with_child_workflows_and_agent_catalog(
            &parent,
            &[crate::direct_wasm::manifest::DirectManifestChildWorkflowInput {
                step_id: "embed",
                workflow_id: "child",
                version_requested: "latest",
                version_resolved: 1,
                execution_graph: &child_graph,
            }],
            None,
        )
        .unwrap();
    manifest.configure_run_state(true, true);
    assert_eq!(manifest.graph.state_mode, DirectStateMode::Published);
    assert_eq!(
        manifest.child_workflows[0].graph.state_mode,
        DirectStateMode::Local { checkpoint: true }
    );
    // The child's schema rides its own graph for the stdlib.
    let child_schema = manifest.child_workflows[0]
        .graph
        .state_schema
        .as_ref()
        .expect("the child declares state");
    assert_eq!(child_schema["count"]["type"], json!("integer"));
    assert!(manifest.graph.state_schema.is_none());
    assert!(!manifest.publishes_state());

    manifest.configure_run_state(false, false);
    assert_eq!(
        manifest.graph.state_mode,
        DirectStateMode::Local { checkpoint: false }
    );
}
