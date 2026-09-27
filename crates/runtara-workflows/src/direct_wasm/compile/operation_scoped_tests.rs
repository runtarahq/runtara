// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Hermetic compiler classification of operation-scoped (suspending or
//! control) Agent sites: the `check_sites` backstops, byte-identical
//! manifests without such sites, and the serialized parallel windows.
use super::*;
use crate::direct_wasm::component::{RuntimeBinding, WorkflowAbi};
use crate::direct_wasm::plan::plan_contains_operation_scoped;
use runtara_dsl::agent_meta::AgentCatalog;
use serde_json::{Value, json};
use std::sync::Arc;

/// `control` (start, get; wait suspends), `waiter` (pause suspends), `utils`.
fn catalog() -> Arc<AgentCatalog> {
    let capability = |id: &str, suspends: bool| {
        json!({"id": id, "name": id, "inputType": "Input", "inputs": [],
            "output": {"type": "object"}, "hasSideEffects": false, "isIdempotent": true,
            "rateLimited": false, "suspends": suspends})
    };
    let agent = |id: &str, capabilities: Vec<Value>| {
        json!({"id": id, "name": id, "description": "fixture", "hasSideEffects": false,
            "supportsConnections": false, "integrationIds": [], "capabilities": capabilities})
    };
    Arc::new(
        AgentCatalog::from_json(
            &json!([
                agent(
                    "control",
                    vec![
                        capability("start", false),
                        capability("get", false),
                        capability("wait", true)
                    ]
                ),
                agent("waiter", vec![capability("pause", true)]),
                agent("utils", vec![capability("plain", false)]),
            ])
            .to_string(),
        )
        .expect("fixture catalog"),
    )
}

fn agent(id: &str, agent: &str, capability: &str) -> Value {
    json!({"id": id, "stepType": "Agent", "agentId": agent, "capabilityId": capability,
        "maxRetries": 0, "timeout": 60_000})
}

fn single(step: Value) -> Value {
    let id = step["id"].as_str().unwrap().to_string();
    json!({"entryPoint": id, "steps": {id.clone(): step,
        "finish": {"id": "finish", "stepType": "Finish"}},
        "executionPlan": [{"fromStep": id, "toStep": "finish"}]})
}

fn input(graph: Value, dir: &Path, catalog: Option<Arc<AgentCatalog>>) -> DirectCompilationInput {
    DirectCompilationInput {
        workflow_id: "scoped".into(),
        version: 1,
        source_checksum: None,
        execution_graph: serde_json::from_value(graph).expect("graph"),
        child_workflows: vec![],
        output_dir: dir.into(),
        track_events: false,
        agent_catalog: catalog,
        agent_slug: None,
    }
}

/// Compile into a fresh directory kept for the rest of the test.
fn compile(graph: Value, abi: WorkflowAbi) -> Result<DirectCompilationResult, DirectCompileError> {
    thread_local! {
        static DIRS: std::cell::RefCell<Vec<tempfile::TempDir>> = const {
            std::cell::RefCell::new(Vec::new())
        };
    }
    let dir = tempfile::tempdir().expect("tempdir");
    let result =
        compile_direct_workflow_with_abi(input(graph, dir.path(), Some(catalog())), abi, false);
    DIRS.with(|dirs| dirs.borrow_mut().push(dir));
    result
}

fn refusal(result: Result<DirectCompilationResult, DirectCompileError>) -> String {
    result.map(|_| ()).expect_err("refused").to_string()
}

#[test]
fn control_sites_compile_under_the_invoke_abi_and_are_marked_in_the_manifest() {
    let result = compile(
        single(agent("get", "control", "get")),
        WorkflowAbi::InvokeHostImports,
    )
    .expect("a top-level control read compiles");
    let manifest: DirectWorkflowManifest =
        serde_json::from_slice(&fs::read(&result.manifest_path).unwrap()).unwrap();
    let site = &manifest.graph.agents[0];
    assert!(site.operation_scoped && !site.suspends);
    let json: Value = serde_json::from_slice(&manifest.to_canonical_json().unwrap()).unwrap();
    assert_eq!(json["graph"]["agents"][0]["operationScoped"], true);

    // Ordinary sites carry neither flag, so their manifests are unchanged.
    let result = compile(
        single(agent("plain", "utils", "plain")),
        WorkflowAbi::InvokeHostImports,
    )
    .expect("compiles");
    let json: Value = serde_json::from_slice(&fs::read(&result.manifest_path).unwrap()).unwrap();
    let site = &json["graph"]["agents"][0];
    assert!(site.get("operationScoped").is_none() && site.get("suspends").is_none());
}

#[test]
fn the_backstops_refuse_what_validation_reports() {
    let control = || single(agent("get", "control", "get"));
    let text = refusal(compile(control(), WorkflowAbi::AgentCapabilities));
    assert!(
        text.contains("cannot be published as a workflow-agent"),
        "{text}"
    );
    let text = refusal(compile(control(), WorkflowAbi::CliRunHttp));
    assert!(text.contains("CliRunHttp"), "{text}");

    let dir = tempfile::tempdir().unwrap();
    let text = refusal(compile_direct_workflow_with_abi(
        input(control(), dir.path(), None),
        WorkflowAbi::InvokeHostImports,
        false,
    ));
    assert!(text.contains("without the agent catalog"), "{text}");

    let dir = tempfile::tempdir().unwrap();
    let text = refusal(compile_direct_workflow_composed_configured(
        input(control(), dir.path(), Some(catalog())),
        dir.path(),
        RuntimeBinding::Composed,
        WorkflowAbi::InvokeHostImports,
        false,
    ));
    assert!(text.contains("composed runtime binding"), "{text}");

    let dir = tempfile::tempdir().unwrap();
    let text = refusal(compile_direct_workflow_with_scoped_agents(
        input(control(), dir.path(), Some(catalog())),
        WorkflowAbi::InvokeHostImports,
        false,
        ["control".to_string()].into(),
    ));
    assert!(text.contains("scoped isolation"), "{text}");

    // A suspending step without durability or a timeout.
    let mut step = agent("wait", "waiter", "pause");
    step["durable"] = json!(false);
    let text = refusal(compile(single(step), WorkflowAbi::InvokeHostImports));
    assert!(text.contains("must be durable"), "{text}");
    let mut step = agent("wait", "waiter", "pause");
    step["timeout"] = Value::Null;
    let text = refusal(compile(single(step), WorkflowAbi::InvokeHostImports));
    assert!(text.contains("needs a timeout"), "{text}");

    // An AiAgent tool, but not the AiAgent's continuation.
    let ai = |label: Option<&str>| {
        let mut edge = json!({"fromStep": "ai", "toStep": "get"});
        if let Some(label) = label {
            edge["label"] = json!(label);
        }
        json!({"entryPoint": "ai", "steps": {
            "ai": {"id": "ai", "stepType": "AiAgent", "connectionId": "llm",
                "config": {"systemPrompt": {"valueType": "immediate", "value": "s"},
                    "userPrompt": {"valueType": "immediate", "value": "u"}}},
            "get": agent("get", "control", "get"),
            "finish": {"id": "finish", "stepType": "Finish"}},
            "executionPlan": [edge, {"fromStep": if label.is_some() { "ai" } else { "get" },
                "toStep": "finish"}]})
    };
    let text = refusal(compile(ai(Some("lookup")), WorkflowAbi::InvokeHostImports));
    assert!(text.contains("AiAgent tool"), "{text}");
    compile(ai(None), WorkflowAbi::InvokeHostImports)
        .expect("a control step after an AiAgent is an ordinary site");
}

/// A plan node's `operation_scoped` flag, found through a Split body.
fn split_body_is_scoped(result: &DirectCompilationResult) -> bool {
    let manifest: DirectWorkflowManifest =
        serde_json::from_slice(&fs::read(&result.manifest_path).unwrap()).unwrap();
    let config = DirectCoreConfig::new(&manifest, &manifest.to_canonical_json().unwrap(), false)
        .expect("core config");
    plan_contains_operation_scoped(&config.run_plan)
}

#[test]
fn a_parallel_split_body_with_a_control_step_runs_sequentially() {
    let split = |body: Value| {
        single(json!({"id": "loop", "stepType": "Split",
            "config": {"value": {"valueType": "immediate", "value": [1, 2, 3]}, "parallelism": 4},
            "subgraph": single(body)}))
    };
    let plain = compile(
        split(agent("call", "utils", "plain")),
        WorkflowAbi::InvokeHostImports,
    )
    .expect("compiles");
    assert!(
        plain.parallel_pools.contains_key("utils"),
        "an ordinary body gets a window: {:?}",
        plain.parallel_pools
    );
    assert!(!split_body_is_scoped(&plain));

    let scoped = compile(
        split(agent("call", "control", "get")),
        WorkflowAbi::InvokeHostImports,
    )
    .expect("compiles");
    assert!(
        scoped.parallel_pools.is_empty(),
        "a control body is serialized: {:?}",
        scoped.parallel_pools
    );
    assert!(split_body_is_scoped(&scoped));
}

#[test]
fn a_branch_group_with_a_control_step_runs_sequentially() {
    let fan_out = |left: Value| {
        json!({"entryPoint": "start", "steps": {
            "start": agent("start", "utils", "plain"), "left": left,
            "right": agent("right", "utils", "plain"),
            "join": {"id": "join", "stepType": "Finish"}},
            "executionPlan": [
                {"fromStep": "start", "toStep": "left"}, {"fromStep": "start", "toStep": "right"},
                {"fromStep": "left", "toStep": "join"}, {"fromStep": "right", "toStep": "join"}]})
    };
    let plain = compile(
        fan_out(agent("left", "utils", "plain")),
        WorkflowAbi::InvokeHostImports,
    )
    .expect("compiles");
    assert!(
        !plain.parallel_pools.is_empty(),
        "ordinary branches run concurrently"
    );
    let scoped = compile(
        fan_out(agent("left", "control", "get")),
        WorkflowAbi::InvokeHostImports,
    )
    .expect("compiles");
    assert!(
        scoped.parallel_pools.is_empty(),
        "a group holding a control step serializes: {:?}",
        scoped.parallel_pools
    );
}
