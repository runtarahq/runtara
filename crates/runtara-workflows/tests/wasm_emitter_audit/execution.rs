// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Invoke-ABI audit regressions. Reuses the parent harness's in-process host;
//! these tests require shared components, but no sockets, database or credentials.

use super::{CheckpointingRuntimeHost, direct_e2e_components_dir, run_invoke_once};
use runtara_component_host::{InvokeExit, lifecycle::WorkflowWake};
use runtara_dsl::{ExecutionGraph, agent_meta::AgentCatalog};
use runtara_workflows::{
    compile::ChildWorkflowInput,
    direct_wasm::{
        DirectCompilationInput, DirectCompilationResult, RuntimeBinding, WorkflowAbi,
        compile_direct_workflow_composed_configured,
    },
    validation::validate_workflow,
};
use serde_json::{Value, json};
use std::sync::Arc;

fn immediate(value: Value) -> Value {
    json!({"valueType": "immediate", "value": value})
}

fn finish(id: &str) -> Value {
    json!({"id": id, "stepType": "Finish", "inputMapping": {"ok": immediate(json!(true))}})
}

fn condition(value: bool) -> Value {
    json!({"type": "operation", "op": "EQ", "arguments": [immediate(json!(value)), immediate(json!(true))]})
}

fn loop_step(id: &str, body: Value) -> Value {
    json!({"id": id, "stepType": "While", "condition": condition(true),
        "config": {"maxIterations": 1}, "subgraph": body})
}

fn compile(id: &str, value: Value) -> (tempfile::TempDir, DirectCompilationResult) {
    compile_with_children(id, value, vec![])
}

fn compile_with_children(
    id: &str,
    value: Value,
    children: Vec<ChildWorkflowInput>,
) -> (tempfile::TempDir, DirectCompilationResult) {
    compile_configured(id, value, children, None)
}

fn compile_configured(
    id: &str,
    value: Value,
    children: Vec<ChildWorkflowInput>,
    catalog: Option<Arc<AgentCatalog>>,
) -> (tempfile::TempDir, DirectCompilationResult) {
    let graph: ExecutionGraph = serde_json::from_value(value).expect("audit graph parses");
    let empty = AgentCatalog::default();
    let validation = validate_workflow(&graph, catalog.as_deref().unwrap_or(&empty));
    assert!(
        validation.errors.is_empty(),
        "audit graph must validate: {:?}",
        validation.errors
    );
    let temp = tempfile::tempdir().unwrap();
    let artifact = compile_direct_workflow_composed_configured(
        DirectCompilationInput {
            workflow_id: id.into(),
            version: 1,
            source_checksum: None,
            execution_graph: graph,
            child_workflows: children,
            output_dir: temp.path().into(),
            track_events: false,
            agent_catalog: catalog,
            agent_slug: None,
        },
        direct_e2e_components_dir(),
        RuntimeBinding::HostImport,
        WorkflowAbi::InvokeHostImports,
        false,
    )
    .expect("audit graph compiles and composes");
    (temp, artifact)
}

fn completed(exit: InvokeExit) -> Value {
    match exit {
        InvokeExit::Completed(bytes) => serde_json::from_slice(&bytes).expect("JSON output"),
        other => panic!("expected completion, got {other:?}"),
    }
}

fn run(id: &str, graph: Value) -> Value {
    let (_temp, artifact) = compile(id, graph);
    completed(run_invoke_once(
        &artifact.wasm_path,
        Arc::new(CheckpointingRuntimeHost::new(b"{}")),
        b"{}".to_vec(),
    ))
}

fn early_finish_graph(outer: bool, inner: bool) -> Value {
    json!({"entryPoint": "outer", "steps": {
        "outer": {"id": "outer", "stepType": "Conditional", "condition": condition(outer)},
        "inner": {"id": "inner", "stepType": "Conditional", "condition": condition(inner)},
        "early": {"id": "early", "stepType": "Finish", "inputMapping": {"result": immediate(json!("early"))}},
        "merge": {"id": "merge", "stepType": "Finish", "inputMapping": {"result": immediate(json!("merge"))}}
    }, "executionPlan": [
        {"fromStep": "outer", "toStep": "inner", "label": "true"},
        {"fromStep": "outer", "toStep": "merge", "label": "false"},
        {"fromStep": "inner", "toStep": "early", "label": "true"},
        {"fromStep": "inner", "toStep": "merge", "label": "false"}
    ]})
}

#[test]
fn audit_01_outer_false_reaches_merge() {
    assert_eq!(
        run("audit-outer-false", early_finish_graph(false, true))["result"],
        "merge"
    );
}

#[test]
fn audit_01_inner_false_reaches_merge() {
    assert_eq!(
        run("audit-inner-false", early_finish_graph(true, false))["result"],
        "merge"
    );
}

#[test]
fn audit_01_early_finish_terminates_before_merge() {
    assert_eq!(
        run("audit-early-finish", early_finish_graph(true, true))["result"],
        "early"
    );
}

// Exercise each dispatch form in both the enclosing and nested position.
fn routing_graph(outer_kind: &str, inner_kind: &str, outer: bool, inner: bool) -> Value {
    let mut graph = early_finish_graph(outer, inner);
    for (id, kind, selected) in [("outer", outer_kind, outer), ("inner", inner_kind, inner)] {
        match kind {
            "Conditional" => {}
            "Switch" => {
                graph["steps"][id] = json!({"id":id,"stepType":"Switch","config":{
                    "value":immediate(json!(selected)),"cases":[{"matchType":"EQ","match":true,"route":"true","output":{}}],"default":{}
                }});
                for edge in graph["executionPlan"].as_array_mut().unwrap() {
                    if edge["fromStep"] == id && edge["label"] == "false" {
                        edge["label"] = json!("default");
                    }
                }
            }
            "EdgeRoute" => {
                graph["steps"][id] = json!({"id":id,"stepType":"Log","message":"route"});
                for edge in graph["executionPlan"].as_array_mut().unwrap() {
                    if edge["fromStep"] == id {
                        if edge["label"] == "true" {
                            edge["condition"] = condition(selected);
                        }
                        edge.as_object_mut().unwrap().remove("label");
                    }
                }
            }
            _ => unreachable!(),
        }
    }
    graph
}

#[test]
fn audit_01_all_dispatch_combinations_preserve_selected_finish() {
    for outer_kind in ["Conditional", "Switch", "EdgeRoute"] {
        for inner_kind in ["Conditional", "Switch", "EdgeRoute"] {
            for (outer, inner) in [(false, false), (false, true), (true, false), (true, true)] {
                let output = run(
                    "audit-routing",
                    routing_graph(outer_kind, inner_kind, outer, inner),
                );
                assert_eq!(
                    output,
                    json!({"result":if outer && inner {"early"} else {"merge"}}),
                    "{outer_kind}/{inner_kind}: outer={outer}, inner={inner}"
                );
            }
        }
    }
}

#[test]
fn audit_01_early_finish_skips_durable_continuation() {
    for kind in ["Conditional", "Switch", "EdgeRoute"] {
        for early in [false, true] {
            let mut graph = routing_graph(kind, kind, true, early);
            graph["steps"]["side_effect"] =
                json!({"id":"side_effect","stepType":"Delay","durationMs":immediate(json!(60000))});
            for edge in graph["executionPlan"].as_array_mut().unwrap() {
                if edge["toStep"] == "merge" {
                    edge["toStep"] = json!("side_effect");
                }
            }
            graph["executionPlan"]
                .as_array_mut()
                .unwrap()
                .push(json!({"fromStep":"side_effect","toStep":"merge"}));
            let (_temp, artifact) = compile("audit-side-effect", graph);
            let host = Arc::new(CheckpointingRuntimeHost::new(b"{}"));
            *host.pinned_clock_ms.lock().unwrap() = Some(1_000_000);
            let exit = run_invoke_once(&artifact.wasm_path, host.clone(), b"{}".to_vec());
            if early {
                assert_eq!(completed(exit), json!({"result":"early"}));
                assert!(
                    host.checkpoints.lock().unwrap().is_empty(),
                    "{kind}: skipped Delay must not checkpoint"
                );
            } else {
                assert!(
                    matches!(exit, InvokeExit::Suspended(_)),
                    "{kind}: continuing route must execute Delay"
                );
                assert!(!host.checkpoints.lock().unwrap().is_empty());
                *host.pinned_clock_ms.lock().unwrap() = Some(1_060_001);
                assert_eq!(
                    completed(run_invoke_once(&artifact.wasm_path, host, b"{}".to_vec())),
                    json!({"result":"merge"})
                );
            }
        }
    }
}

#[test]
fn audit_01_implicit_finish_does_not_fall_through() {
    for kind in ["Conditional", "Switch", "EdgeRoute"] {
        let mut graph = routing_graph(kind, kind, true, true);
        graph["steps"]["early"] = json!({"id":"early","stepType":"Log","message":"terminal"});
        assert_eq!(run("audit-implicit-finish", graph), Value::Null);
    }
}

fn scoped_finish_graph(kind: &str) -> Value {
    let body = early_finish_graph(true, true);
    let step = if kind == "While" {
        let mut step = loop_step("scope", body);
        step["config"]["maxIterations"] = json!(2);
        step
    } else {
        json!({"id":"scope","stepType":"Split","config":{
            "value":immediate(json!([{},{}])),"sequential":kind == "Split","parallelism":2
        },"subgraph":body})
    };
    json!({"entryPoint":"scope","steps":{
        "scope":step,"parent_finish":{"id":"parent_finish","stepType":"Finish","inputMapping":{
            "parentContinued":immediate(json!(true)),
            "body":{"valueType":"reference","value":"steps.scope.outputs"}
        }}
    },"executionPlan":[{"fromStep":"scope","toStep":"parent_finish"}]})
}

#[test]
fn audit_01_while_finish_exits_only_its_iteration() {
    let output = run("audit-while-exit", scoped_finish_graph("While"));
    assert_eq!(
        output,
        json!({"parentContinued":true,"body":{"iterations":2,"outputs":{"result":"early"}}})
    );
}

fn assert_split_finish_scope(kind: &str) {
    let output = run("audit-split-exit", scoped_finish_graph(kind));
    assert_eq!(
        output,
        json!({"parentContinued":true,"body":[{"result":"early"},{"result":"early"}]})
    );
}

#[test]
fn audit_01_split_finish_exits_only_its_iteration() {
    assert_split_finish_scope("Split");
}

#[test]
fn audit_01_parallel_split_finish_exits_only_its_iteration() {
    assert_split_finish_scope("ParallelSplit");
}

#[test]
fn audit_01_child_finish_returns_to_parent() {
    let child = early_finish_graph(true, true);
    let parent = json!({"entryPoint":"call","steps":{
        "call":{"id":"call","stepType":"EmbedWorkflow","childWorkflowId":"child","childVersion":"latest"},
        "parent_finish":{"id":"parent_finish","stepType":"Finish","inputMapping":{
            "parentContinued":immediate(json!(true)),"body":{"valueType":"reference","value":"steps.call.outputs"}
        }}
    },"executionPlan":[{"fromStep":"call","toStep":"parent_finish"}]});
    let (_temp, artifact) = compile_with_children(
        "audit-child-exit",
        parent,
        vec![ChildWorkflowInput {
            step_id: "call".into(),
            workflow_id: "child".into(),
            version_requested: "latest".into(),
            version_resolved: 1,
            execution_graph: serde_json::from_value(child).unwrap(),
        }],
    );
    let output = completed(run_invoke_once(
        &artifact.wasm_path,
        Arc::new(CheckpointingRuntimeHost::new(b"{}")),
        b"{}".to_vec(),
    ));
    assert_eq!(
        output,
        json!({"parentContinued":true,"body":{"result":"early"}})
    );
}

fn gc_graph(size: usize, nested: bool) -> Value {
    let inner_body =
        json!({"entryPoint": "inner_finish", "steps": {"inner_finish": finish("inner_finish")}});
    let body = if nested {
        json!({"entryPoint": "inner", "steps": {
            "inner": loop_step("inner", inner_body), "outer_finish": finish("outer_finish")
        }, "executionPlan": [{"fromStep": "inner", "toStep": "outer_finish"}]})
    } else {
        inner_body
    };
    json!({"entryPoint": "filter", "steps": {
        "filter": {"id": "filter", "stepType": "Filter", "config": {
            "value": immediate(json!([{"large": "x".repeat(size)}])), "condition": condition(true)
        }}, "outer": loop_step("outer", body),
        "finish": {"id": "finish", "stepType": "Finish", "inputMapping": {
            "saved": {"valueType": "reference", "value": "steps.filter.outputs"}
        }}
    }, "executionPlan": [{"fromStep": "filter", "toStep": "outer"}, {"fromStep": "outer", "toStep": "finish"}]})
}

fn assert_gc_preserves_value(size: usize, nested: bool) {
    let value = run("audit-gc", gc_graph(size, nested));
    // Compare the full contents without dumping a 100 KB assertion failure.
    assert!(
        value["saved"]["items"][0]["large"].as_str() == Some("x".repeat(size).as_str()),
        "AUDIT-02: outer Filter output must survive loop collection (size={size}, nested={nested})"
    );
}

#[test]
fn audit_02_nested_loop_preserves_small_outer_value() {
    assert_gc_preserves_value(32, true);
}

#[test]
fn audit_02_single_loop_preserves_large_outer_value() {
    assert_gc_preserves_value(100_000, false);
}

#[test]
fn audit_02_nested_loop_preserves_large_outer_value() {
    assert_gc_preserves_value(100_000, true);
}

fn sized_filter(id: &str, value: Value) -> Value {
    json!({"id":id,"stepType":"Filter","config":{"value":immediate(json!([value])),"condition":condition(true)}})
}

fn nested_gc_body(kinds: &[&str], depth: usize, leaf: Value) -> Value {
    let Some((kind, rest)) = kinds.split_first() else {
        return leaf;
    };
    let id = format!("loop_{depth}");
    let done = format!("done_{depth}");
    let body = nested_gc_body(rest, depth + 1, leaf);
    let step = if *kind == "While" {
        json!({"id":id,"stepType":"While","condition":condition(true),"config":{"maxIterations":2},"subgraph":body})
    } else {
        json!({"id":id,"stepType":"Split","config":{"value":immediate(json!([{},{}])),"sequential":true},"subgraph":body})
    };
    json!({"entryPoint":id,"steps":{id.clone():step,done.clone():finish(&done)},"executionPlan":[{"fromStep":id,"toStep":done}]})
}

fn preserve_outer_source(body: Value) -> Value {
    let mut graph = body;
    let entry = graph["entryPoint"].clone();
    graph["steps"]["saved_filter"] =
        sized_filter("saved_filter", json!({"large":"x".repeat(100_000)}));
    // Replace the body's terminal Finish with a continuation that reads the root.
    for step in graph["steps"].as_object_mut().unwrap().values_mut() {
        if step["stepType"] == "Finish" {
            step["inputMapping"] = json!({"saved":{"valueType":"reference","value":"steps.saved_filter.outputs.items.0.large"}});
        }
    }
    graph["steps"]
        .as_object_mut()
        .unwrap()
        .remove("saved_finish");
    graph["entryPoint"] = json!("saved_filter");
    graph["executionPlan"]
        .as_array_mut()
        .unwrap()
        .insert(0, json!({"fromStep":"saved_filter","toStep":entry}));
    graph
}

fn assert_saved_large(output: &Value) {
    assert!(
        output["saved"].as_str() == Some("x".repeat(100_000).as_str()),
        "outer 100 KB value must survive collection"
    );
}

#[test]
fn audit_02_mixed_nested_loops_preserve_outer_source() {
    for kinds in [
        vec!["While", "While"],
        vec!["While", "Split"],
        vec!["Split", "While"],
        vec!["Split", "Split"],
        vec!["While", "Split", "While"],
        vec!["Split", "While", "Split"],
    ] {
        let leaf = json!({"entryPoint":"leaf","steps":{"leaf":finish("leaf")}});
        assert_saved_large(&run(
            "audit-mixed-gc",
            preserve_outer_source(nested_gc_body(&kinds, 0, leaf)),
        ));
    }
}

#[test]
fn audit_02_nested_loop_preserves_value_near_intern_threshold() {
    for size in [16_000, 16_383, 16_384, 16_385, 32_768] {
        assert_gc_preserves_value(size, true);
    }
}

#[test]
fn audit_02_child_loop_preserves_caller_source() {
    let child = nested_gc_body(
        &["While", "Split"],
        0,
        json!({"entryPoint":"leaf","steps":{"leaf":finish("leaf")}}),
    );
    let parent = preserve_outer_source(json!({"entryPoint":"call","steps":{
        "call":{"id":"call","stepType":"EmbedWorkflow","childWorkflowId":"gc-child","childVersion":"latest"},
        "parent_done":finish("parent_done")
    },"executionPlan":[{"fromStep":"call","toStep":"parent_done"}]}));
    let (_temp, artifact) = compile_with_children(
        "audit-child-gc",
        parent,
        vec![ChildWorkflowInput {
            step_id: "call".into(),
            workflow_id: "gc-child".into(),
            version_requested: "latest".into(),
            version_resolved: 1,
            execution_graph: serde_json::from_value(child).unwrap(),
        }],
    );
    assert_saved_large(&completed(run_invoke_once(
        &artifact.wasm_path,
        Arc::new(CheckpointingRuntimeHost::new(b"{}")),
        b"{}".to_vec(),
    )));
}

#[test]
fn audit_02_nested_collection_survives_suspend_and_replay() {
    let leaf = json!({"entryPoint":"delay","steps":{
        "delay":{"id":"delay","stepType":"Delay","durationMs":immediate(json!(60000))},"leaf_done":finish("leaf_done")
    },"executionPlan":[{"fromStep":"delay","toStep":"leaf_done"}]});
    let mut graph = nested_gc_body(&["While", "While"], 0, leaf);
    graph["steps"]["loop_0"]["config"]["maxIterations"] = json!(1);
    graph["steps"]["loop_0"]["subgraph"]["steps"]["loop_1"]["config"]["maxIterations"] = json!(1);
    let (_temp, artifact) = compile("audit-replay-gc", preserve_outer_source(graph));
    let host = Arc::new(CheckpointingRuntimeHost::new(b"{}"));
    *host.pinned_clock_ms.lock().unwrap() = Some(1_000_000);
    let invoke = || run_invoke_once(&artifact.wasm_path, host.clone(), b"{}".to_vec());
    for _ in 0..2 {
        assert!(matches!(invoke(), InvokeExit::Suspended(_)));
    }
    *host.pinned_clock_ms.lock().unwrap() = Some(1_060_001);
    assert_saved_large(&completed(invoke()));
    for bytes in host.checkpoints.lock().unwrap().values() {
        let text = String::from_utf8_lossy(bytes);
        assert!(
            !text.contains("$wfref") && !text.contains("$wfnonce"),
            "checkpoints must contain materialized values"
        );
    }
}

#[test]
fn audit_02_nested_error_handler_preserves_outer_source() {
    let leaf = json!({"entryPoint":"error","steps":{"error":{"id":"error","stepType":"Error","code":"AUDIT_ERROR","message":"expected"}}});
    let mut graph = preserve_outer_source(nested_gc_body(&["While", "Split"], 0, leaf));
    graph["steps"]["handler"] = json!({"id":"handler","stepType":"Finish","inputMapping":{
        "saved":{"valueType":"reference","value":"steps.saved_filter.outputs.items.0.large"},"handled":immediate(json!(true))
    }});
    graph["executionPlan"]
        .as_array_mut()
        .unwrap()
        .push(json!({"fromStep":"loop_0","toStep":"handler","label":"onError"}));
    let output = run("audit-error-gc", graph);
    assert_saved_large(&output);
    assert_eq!(output["handled"], true);
}

#[test]
fn audit_02_parallel_split_preserves_live_chunk_and_outer_values() {
    let inner = nested_gc_body(
        &["While"],
        0,
        json!({"entryPoint":"leaf","steps":{"leaf":finish("leaf")}}),
    );
    let mut body = inner;
    body["steps"]["invoke"] = json!({"id":"invoke","stepType":"Agent","agentId":"utils","capabilityId":"return-input",
        "maxRetries":0,"inputMapping":{"value":{"valueType":"reference","value":"item"}}});
    body["entryPoint"] = json!("invoke");
    body["executionPlan"]
        .as_array_mut()
        .unwrap()
        .insert(0, json!({"fromStep":"invoke","toStep":"loop_0"}));
    body["steps"]["done_0"]["inputMapping"] =
        json!({"value":{"valueType":"reference","value":"steps.invoke.outputs"}});
    let values = vec![
        "a".repeat(100_000),
        "b".repeat(100_000),
        "c".repeat(100_000),
        "d".repeat(100_000),
    ];
    let mut graph = preserve_outer_source(json!({"entryPoint":"parallel","steps":{
        "parallel":{"id":"parallel","stepType":"Split","config":{"value":immediate(json!(values)),"sequential":false,"parallelism":2},"subgraph":body},
        "done":finish("done")
    },"executionPlan":[{"fromStep":"parallel","toStep":"done"}]}));
    graph["steps"]["done"]["inputMapping"]["results"] =
        json!({"valueType":"reference","value":"steps.parallel.outputs"});
    let utils_meta =
        std::fs::read(direct_e2e_components_dir().join("runtara_agent_utils.meta.json")).unwrap();
    let catalog = Arc::new(AgentCatalog::from_agents(vec![
        serde_json::from_slice(&utils_meta).unwrap(),
    ]));
    let (_temp, artifact) = compile_configured("audit-parallel-gc", graph, vec![], Some(catalog));
    let logic = std::fs::read(&artifact.workflow_logic_wasm_path).unwrap();
    assert!(
        logic
            .windows(b"[waitable-set-new]".len())
            .any(|w| w == b"[waitable-set-new]"),
        "must exercise real parallel lowering"
    );
    let output = completed(run_invoke_once(
        &artifact.wasm_path,
        Arc::new(CheckpointingRuntimeHost::new(b"{}")),
        b"{}".to_vec(),
    ));
    assert_saved_large(&output);
    let results = output["results"].as_array().expect("parallel results");
    assert_eq!(results.len(), values.len());
    for (result, expected) in results.iter().zip(values) {
        assert!(
            result["value"].as_str() == Some(expected.as_str()),
            "every parallel item's value survives nested collection: actual type/length={:?}, keys={:?}",
            result["value"]
                .as_str()
                .map(|s| (s.len(), s.chars().next())),
            result.as_object().map(|o| o.keys().collect::<Vec<_>>())
        );
    }
}

#[test]
fn audit_02_nested_growing_accumulator_completes_with_bounded_memory() {
    let accumulator: Value =
        serde_json::from_str(&super::while_accumulator_graph(64 * 1024)).unwrap();
    let mut graph = preserve_outer_source(nested_gc_body(&["While"], 0, accumulator));
    graph["steps"]["done_0"]["inputMapping"]["innerIterations"] =
        json!({"valueType":"reference","value":"steps.loop_0.outputs.outputs.iterations"});
    graph["steps"]["done_0"]["inputMapping"]["outerIterations"] =
        json!({"valueType":"reference","value":"steps.loop_0.outputs.iterations"});
    let (_temp, artifact) = compile("audit-bounded-nested-gc", graph);
    let input = br#"{"count":60}"#.to_vec();
    let host = Arc::new(CheckpointingRuntimeHost::new(&input));
    let executor = super::embedded_executor();
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let result = runtime.block_on(async {
        let pre = executor
            .load_instance_pre(&artifact.wasm_path)
            .await
            .unwrap();
        executor
            .execute_invoke(
                &pre,
                runtara_component_host::WorkflowRunSpec {
                    env: Default::default(),
                    stderr: None,
                    timeout: std::time::Duration::from_secs(60),
                    cancel: None,
                    limits: runtara_component_host::WorkflowLimits {
                        max_memory_bytes: 96 * 1024 * 1024,
                        ..Default::default()
                    },
                    runtime: Some(host),
                },
                input,
            )
            .await
    });
    let output = completed(result.exit);
    assert_saved_large(&output);
    assert_eq!(
        output["innerIterations"], 60,
        "inner loop must actually complete, not fail early"
    );
    assert_eq!(output["outerIterations"], 2);
    assert!(
        result.memory_peak_bytes < 64 * 1024 * 1024,
        "nested accumulator must stay bounded: {} bytes",
        result.memory_peak_bytes
    );
}

fn waits_graph(second_id: &str) -> Value {
    let body = |id: &str| {
        json!({"entryPoint": id, "steps": {
        id: {"id": id, "stepType": "WaitForSignal"}, "done": finish("done")
    }, "executionPlan": [{"fromStep": id, "toStep": "done"}]})
    };
    json!({"entryPoint": "a", "steps": {
        "a": loop_step("a", body("wait")), "b": loop_step("b", body(second_id)), "finish": finish("finish")
    }, "executionPlan": [{"fromStep": "a", "toStep": "b"}, {"fromStep": "b", "toStep": "finish"}]})
}

fn signal_key(exit: InvokeExit) -> String {
    let InvokeExit::Suspended(wakes) = exit else {
        panic!("expected a signal suspension, got {exit:?}")
    };
    assert_eq!(wakes.len(), 1, "one sequential waiter");
    match &wakes[0] {
        WorkflowWake::OnSignal(wait) => wait.checkpoint_id.clone(),
        other => panic!("expected signal wake, got {other:?}"),
    }
}

fn assert_independent_waits(second_id: &str) {
    let (_temp, artifact) = compile("audit-waits", waits_graph(second_id));
    let host = Arc::new(CheckpointingRuntimeHost::new(b"{}"));
    let invoke = || run_invoke_once(&artifact.wasm_path, host.clone(), b"{}".to_vec());
    let first = signal_key(invoke());
    let fields = super::key_fields(&first);
    let old_key = format!("{}/audit-waits/wait/[0]", fields[4][0].as_str().unwrap());
    host.deliver_signal(&old_key, br#"{"stale":true}"#);
    // A replay with no response must address the same first waiter.
    assert_eq!(signal_key(invoke()), first);
    host.deliver_signal(&first, br#"{"approved":true}"#);
    let second = signal_key(invoke());
    assert_ne!(
        first, second,
        "AUDIT-03: sibling loop IDs must distinguish signal keys"
    );
    host.deliver_signal(&second, br#"{"approved":false}"#);
    assert_eq!(completed(invoke()), json!({"ok": true}));
}

#[test]
fn audit_03_distinct_wait_ids_suspend_independently_and_replay_stably() {
    assert_independent_waits("wait_b");
}

#[test]
fn audit_03_same_local_wait_id_suspends_independently() {
    assert_independent_waits("wait");
}

fn audit_loop(id: &str, kind: &str, count: usize, body: Value) -> Value {
    if kind == "While" {
        let mut step = loop_step(id, body);
        step["config"]["maxIterations"] = json!(count);
        step
    } else {
        json!({"id":id,"stepType":"Split","config":{"value":immediate(json!(vec![0;count])),"sequential":true},"subgraph":body})
    }
}

fn assert_sequential_signals(graph: Value, count: usize) {
    let (_temp, artifact) = compile("audit-scoped-waits", graph);
    let host = Arc::new(CheckpointingRuntimeHost::new(b"{}"));
    let invoke = || run_invoke_once(&artifact.wasm_path, host.clone(), b"{}".to_vec());
    let mut keys = std::collections::HashSet::new();
    for n in 0..count {
        let key = signal_key(invoke());
        assert_eq!(signal_key(invoke()), key, "replay stays at waiter {n}");
        assert!(
            keys.insert(key.clone()),
            "each invocation must own its signal: {key}"
        );
        host.deliver_signal(&key, &serde_json::to_vec(&json!({"response":n})).unwrap());
    }
    assert_eq!(completed(invoke()), json!({"ok":true}));
}

#[test]
fn audit_03_sibling_mixed_loops_and_repeated_iterations_wait_independently() {
    for (left, right) in [
        ("While", "While"),
        ("While", "Split"),
        ("Split", "While"),
        ("Split", "Split"),
    ] {
        let mut graph = waits_graph("wait");
        let body = graph["steps"]["a"]["subgraph"].clone();
        graph["steps"]["a"] = audit_loop("a", left, 2, body.clone());
        graph["steps"]["b"] = audit_loop("b", right, 2, body);
        assert_sequential_signals(graph, 4);
    }
}

#[test]
fn audit_03_nested_sibling_loops_keep_the_complete_path() {
    let mut graph = waits_graph("wait");
    for id in ["a", "b"] {
        let leaf = graph["steps"][id]["subgraph"].clone();
        graph["steps"][id]["subgraph"] = json!({"entryPoint":"inner","steps":{
            "inner":audit_loop("inner", "Split", 2, leaf),"inner_done":finish("inner_done")
        },"executionPlan":[{"fromStep":"inner","toStep":"inner_done"}]});
    }
    assert_sequential_signals(graph, 4);
}

#[test]
fn audit_03_sibling_delays_checkpoint_independent_deadlines() {
    let mut graph = waits_graph("wait");
    graph["durable"] = json!(true);
    for id in ["a", "b"] {
        graph["steps"][id]["subgraph"]["steps"]["wait"] =
            json!({"id":"wait","stepType":"Delay","durationMs":immediate(json!(60_000))});
    }
    let (_temp, artifact) = compile("audit-delays", graph);
    let host = Arc::new(CheckpointingRuntimeHost::new(b"{}"));
    let invoke = || run_invoke_once(&artifact.wasm_path, host.clone(), b"{}".to_vec());
    *host.pinned_clock_ms.lock().unwrap() = Some(1_000_000);
    assert!(matches!(invoke(), InvokeExit::Suspended(_)));
    let first = host.checkpoints.lock().unwrap().clone();
    assert!(matches!(invoke(), InvokeExit::Suspended(_)));
    assert_eq!(*host.checkpoints.lock().unwrap(), first);
    *host.pinned_clock_ms.lock().unwrap() = Some(1_060_001);
    assert!(
        matches!(invoke(), InvokeExit::Suspended(_)),
        "second site must establish its own deadline"
    );
    let keys = host
        .checkpoints
        .lock()
        .unwrap()
        .keys()
        .filter(|key| key.starts_with("runtara:v2:[\"delay\""))
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(keys.len(), 2);
    *host.pinned_clock_ms.lock().unwrap() = Some(1_120_002);
    assert_eq!(completed(invoke()), json!({"ok":true}));
}

#[test]
fn audit_03_durable_agent_and_split_caches_keep_sibling_results_on_replay() {
    let reference = |value| json!({"valueType":"reference","value":value});
    let agent_body = json!({"entryPoint":"invoke","steps":{
        "invoke":{"id":"invoke","stepType":"Agent","agentId":"utils","capabilityId":"return-input","inputMapping":{"value":immediate(json!("A"))}},
        "agent_done":{"id":"agent_done","stepType":"Finish","inputMapping":{"value":reference("steps.invoke.outputs")}}
    },"executionPlan":[{"fromStep":"invoke","toStep":"agent_done"}]});
    let body = json!({"entryPoint":"items","steps":{
        "items":audit_loop("items", "Split", 1, agent_body),
        "body_done":{"id":"body_done","stepType":"Finish","inputMapping":{"value":reference("steps.items.outputs.0.value")}}
    },"executionPlan":[{"fromStep":"items","toStep":"body_done"}]});
    let a = loop_step("a", body.clone());
    let mut b = loop_step("b", body);
    b["subgraph"]["steps"]["items"]["subgraph"]["steps"]["invoke"]["inputMapping"]["value"] =
        immediate(json!("B"));
    let graph = json!({"durable":true,"entryPoint":"a","steps":{
        "a":a,"b":b,"done":{"id":"done","stepType":"Finish","inputMapping":{
            "a":reference("steps.a.outputs.outputs.value"),"b":reference("steps.b.outputs.outputs.value")
        }}
    },"executionPlan":[{"fromStep":"a","toStep":"b"},{"fromStep":"b","toStep":"done"}]});
    let meta =
        std::fs::read(direct_e2e_components_dir().join("runtara_agent_utils.meta.json")).unwrap();
    let catalog = Arc::new(AgentCatalog::from_agents(vec![
        serde_json::from_slice(&meta).unwrap(),
    ]));
    let (_temp, artifact) = compile_configured("audit-cache-sites", graph, vec![], Some(catalog));
    let host = Arc::new(CheckpointingRuntimeHost::new(b"{}"));
    for _ in 0..2 {
        assert_eq!(
            completed(run_invoke_once(
                &artifact.wasm_path,
                host.clone(),
                b"{}".to_vec()
            )),
            json!({"a":"A","b":"B"})
        );
    }
    let checkpoints = host.checkpoints.lock().unwrap();
    for kind in ["agent", "split"] {
        assert_eq!(
            checkpoints
                .keys()
                .filter(|key| key.starts_with(&format!("runtara:v2:[\"{kind}\"")))
                .count(),
            2
        );
    }
}

fn timeout_graph(delay_ms: u64, split: bool) -> Value {
    let body = json!({"entryPoint": "delay", "steps": {
        "delay": {"id": "delay", "stepType": "Delay", "durationMs": immediate(json!(delay_ms))},
        "body_finish": finish("body_finish")
    }, "executionPlan": [{"fromStep": "delay", "toStep": "body_finish"}]});
    let step = if split {
        json!({"id": "loop", "stepType": "Split", "config": {
            "value": immediate(json!([{}])), "timeout": 1000, "sequential": true
        }, "subgraph": body})
    } else {
        let mut step = loop_step("loop", body);
        step["config"]["timeout"] = json!(1000);
        step
    };
    json!({"entryPoint": "loop", "steps": {"loop": step, "finish": finish("finish")},
        "executionPlan": [{"fromStep": "loop", "toStep": "finish"}]})
}

fn resume_after_delay(delay_ms: u64, split: bool) -> InvokeExit {
    let (_temp, artifact) = compile("audit-timeout", timeout_graph(delay_ms, split));
    let host = Arc::new(CheckpointingRuntimeHost::new(b"{}"));
    *host.pinned_clock_ms.lock().unwrap() = Some(1_000_000);
    let first = run_invoke_once(&artifact.wasm_path, host.clone(), b"{}".to_vec());
    assert!(
        matches!(first, InvokeExit::Suspended(_)),
        "expected initial suspension, got {first:?}"
    );
    *host.pinned_clock_ms.lock().unwrap() = Some(1_000_000 + delay_ms + 1);
    run_invoke_once(&artifact.wasm_path, host, b"{}".to_vec())
}

#[test]
fn audit_05_while_completes_when_resume_is_within_timeout() {
    assert_eq!(
        completed(resume_after_delay(500, false)),
        json!({"ok": true})
    );
}

#[test]
fn audit_05_split_completes_when_resume_is_within_timeout() {
    assert_eq!(
        completed(resume_after_delay(500, true)),
        json!({"ok": true})
    );
}

fn assert_timeout_failure(exit: InvokeExit, expected_code: &str) {
    let InvokeExit::Failed(error) = exit else {
        panic!("AUDIT-05: 1-second timeout must fail after 60 seconds parked, got {exit:?}")
    };
    assert_eq!(
        error.code, expected_code,
        "failure must be the enclosing loop timeout"
    );
}

#[test]
#[ignore = "AUDIT-05: While deadline is recomputed after suspension"]
fn audit_05_while_timeout_survives_suspend_resume() {
    assert_timeout_failure(resume_after_delay(60_000, false), "WHILE_TIMEOUT");
}

#[test]
#[ignore = "AUDIT-05: Split deadline is recomputed after suspension"]
fn audit_05_split_timeout_survives_suspend_resume() {
    assert_timeout_failure(resume_after_delay(60_000, true), "SPLIT_TIMEOUT");
}
