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
    compile_configured_tracking(id, value, children, catalog, false)
}

fn compile_configured_tracking(
    id: &str,
    value: Value,
    children: Vec<ChildWorkflowInput>,
    catalog: Option<Arc<AgentCatalog>>,
    track_events: bool,
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
            track_events,
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

#[test]
fn run_label_is_persisted_by_compiled_finish_without_changing_output() {
    let mut graph = early_finish_graph(true, true);
    graph["steps"]["early"]["runLabel"] =
        json!({"valueType":"template","value":"Order/{{ 12 }} [done]"});
    graph["steps"]["merge"]["runLabel"] = immediate(json!("unreached"));
    let (_temp, artifact) = compile("run-label-branch", graph);
    let host = Arc::new(CheckpointingRuntimeHost::new(b"{}"));
    let output = completed(run_invoke_once(
        &artifact.wasm_path,
        host.clone(),
        b"{}".to_vec(),
    ));
    assert_eq!(output, json!({"result":"early"}));
    assert_eq!(
        host.run_label.lock().unwrap().as_deref(),
        Some("Order/12 [done]")
    );
    assert_eq!(
        serde_json::from_slice::<Value>(host.completed.lock().unwrap().as_ref().unwrap()).unwrap(),
        output
    );
}

#[test]
fn run_label_invalid_dynamic_value_is_ignored_without_changing_output() {
    for template in ["invalid_{{ 12 }}", "--- ./()[]", "   ", "\u{200b}"] {
        let mut graph = early_finish_graph(true, true);
        graph["steps"]["early"]["runLabel"] = json!({"valueType":"template","value":template});
        let (_temp, artifact) = compile("run-label-invalid", graph);
        let host = Arc::new(CheckpointingRuntimeHost::new(b"{}"));
        assert_eq!(
            completed(run_invoke_once(
                &artifact.wasm_path,
                host.clone(),
                b"{}".to_vec()
            )),
            json!({"result":"early"})
        );
        assert_eq!(
            serde_json::from_slice::<Value>(host.completed.lock().unwrap().as_ref().unwrap())
                .unwrap(),
            json!({"result":"early"})
        );
        assert!(host.run_label.lock().unwrap().is_none());
        assert!(host.failed.lock().unwrap().is_none());
    }
}

#[test]
fn run_label_in_inline_child_does_not_rename_parent() {
    let child: ExecutionGraph = serde_json::from_value(json!({"entryPoint":"finish","steps":{
        "finish":{"id":"finish","stepType":"Finish","runLabel":immediate(json!("child")),"inputMapping":{"ok":immediate(json!(true))}}
    }})).unwrap();
    let root = json!({"entryPoint":"child","steps":{
        "child":{"id":"child","stepType":"EmbedWorkflow","childWorkflowId":"child","childVersion":1},
        "finish":finish("finish")
    },"executionPlan":[{"fromStep":"child","toStep":"finish"}]});
    let (_temp, artifact) = compile_with_children(
        "run-label-parent",
        root,
        vec![ChildWorkflowInput {
            step_id: "child".into(),
            workflow_id: "child".into(),
            version_requested: "1".into(),
            version_resolved: 1,
            execution_graph: child,
        }],
    );
    let host = Arc::new(CheckpointingRuntimeHost::new(b"{}"));
    assert_eq!(
        completed(run_invoke_once(
            &artifact.wasm_path,
            host.clone(),
            b"{}".to_vec()
        )),
        json!({"ok":true})
    );
    assert!(host.run_label.lock().unwrap().is_none());
}

#[test]
fn run_label_resolves_after_resume_and_empty_or_null_remain_unlabelled() {
    for (value, expected) in [
        (json!(" Resumed/42 "), Some("Resumed/42".to_string())),
        (json!("x".repeat(300)), Some("x".repeat(250))),
        (json!("---"), None),
        (json!(42), None),
        (json!({"x":1}), None),
        (json!("   "), None),
        (json!(""), None),
        (Value::Null, None),
    ] {
        let graph = json!({"entryPoint":"delay","inputSchema":{"label":{"type":"string","nullable":true}},"steps":{
            "delay":{"id":"delay","stepType":"Delay","durationMs":immediate(json!(60_000))},
            "finish":{"id":"finish","stepType":"Finish","runLabel":{"valueType":"reference","value":"data.label"},"inputMapping":{"ok":immediate(json!(true))}}
        },"executionPlan":[{"fromStep":"delay","toStep":"finish"}]});
        let (_temp, artifact) = compile("run-label-resume", graph);
        let host = audit_deadline_host(1_000_000);
        let input = serde_json::to_vec(&json!({"label":value})).unwrap();
        assert_at(
            run_invoke_once(&artifact.wasm_path, host.clone(), input.clone()),
            1_060_000,
        );
        assert!(host.run_label.lock().unwrap().is_none());
        assert!(host.completed.lock().unwrap().is_none());
        *host.pinned_clock_ms.lock().unwrap() = Some(1_060_001);
        assert_eq!(
            completed(run_invoke_once(&artifact.wasm_path, host.clone(), input)),
            json!({"ok":true})
        );
        assert_eq!(*host.run_label.lock().unwrap(), expected);
    }
}

#[test]
fn run_label_on_error_handler_finish_completes_the_execution() {
    let mut graph = timeout_graph(60_000, false);
    graph["executionPlan"]
        .as_array_mut()
        .unwrap()
        .push(json!({"fromStep":"loop","toStep":"recovered","label":"onError"}));
    graph["steps"]["recovered"] = finish("recovered");
    graph["steps"]["recovered"]["runLabel"] = immediate(json!("Timeout (recovered)"));
    let (_temp, artifact) = compile("run-label-recovery", graph);
    let host = audit_deadline_host(1_000_000);
    assert_at(
        run_invoke_once(&artifact.wasm_path, host.clone(), b"{}".to_vec()),
        1_001_000,
    );
    *host.pinned_clock_ms.lock().unwrap() = Some(1_001_000);
    assert_eq!(
        completed(run_invoke_once(
            &artifact.wasm_path,
            host.clone(),
            b"{}".to_vec()
        )),
        json!({"ok":true})
    );
    assert_eq!(
        host.run_label.lock().unwrap().as_deref(),
        Some("Timeout (recovered)")
    );
    assert!(host.failed.lock().unwrap().is_none());
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

fn configured_wait_body(timeout: u64, poll: u64, label: &str) -> Value {
    json!({"entryPoint":"wait","steps":{
        "wait":{"id":"wait","stepType":"WaitForSignal","name":label,
            "timeoutMs":immediate(json!(timeout)),"pollIntervalMs":poll,
            "responseSchema":{"decision":{"type":"string","enum":[label]}},
            "action":{"key":label,"correlation":{"site":immediate(json!(label))},"context":{"label":immediate(json!(label))}}},
        "done":finish("done")
    },"executionPlan":[{"fromStep":"wait","toStep":"done"}]})
}

fn assert_configured_waits(artifact: &DirectCompilationResult, labels: [&str; 2]) {
    let host = Arc::new(CheckpointingRuntimeHost::new(b"{}"));
    *host.pinned_clock_ms.lock().unwrap() = Some(1_000_000);
    let invoke = || run_invoke_once(&artifact.wasm_path, host.clone(), b"{}".to_vec());
    for (label, timeout, poll) in [(labels[0], 60_000, 11), (labels[1], 120_000, 22)] {
        let exit = invoke();
        let InvokeExit::Suspended(ref wakes) = exit else {
            panic!("expected configured wait, got {exit:?}");
        };
        let WorkflowWake::OnSignal(ref wait) = wakes[0] else {
            panic!("expected signal wake");
        };
        assert_eq!(
            wait.deadline_ms,
            Some(1_000_000 + timeout),
            "the selected graph's timeout sets the deadline"
        );
        let key = signal_key(exit);
        let events = host.custom_events.lock().unwrap();
        let payloads = events
            .iter()
            .map(|(kind, bytes)| (kind, serde_json::from_slice::<Value>(bytes).unwrap()))
            .collect::<Vec<_>>();
        let event = payloads
            .iter()
            .rev()
            .find(|(_, value)| value["type"] == "external_input_requested")
            .expect("pending-input event");
        assert_eq!(event.1["step_id"], "wait");
        assert_eq!(event.1["step_name"], label);
        assert_eq!(event.1["signal_id"], key);
        assert_eq!(event.1["action_key"], label);
        assert_eq!(event.1["correlation"]["site"], label);
        assert_eq!(event.1["context"]["label"], label);
        assert_eq!(
            event.1["response_schema"]["decision"]["enum"],
            json!([label])
        );
        let debug = payloads
            .iter()
            .rev()
            .find(|(kind, value)| kind.as_str() == "step_debug_start" && value["step_id"] == "wait")
            .expect("wait debug start");
        assert_eq!(debug.1["step_name"], label);
        assert_eq!(debug.1["inputs"]["timeout_ms"], timeout);
        assert_eq!(debug.1["inputs"]["poll_interval_ms"], poll);
        drop(events);
        // Recreate the Store with identical source; a pending wait keeps its key/deadline.
        assert_eq!(signal_key(invoke()), key);
        host.deliver_signal(
            &key,
            &serde_json::to_vec(&json!({"decision":label})).unwrap(),
        );
    }
    assert_eq!(completed(invoke()), json!({"ok":true}));
}

#[test]
fn audit_04_sibling_loop_wait_settings_and_events_follow_their_graph() {
    for (left, right) in [
        ("While", "While"),
        ("While", "Split"),
        ("Split", "While"),
        ("Split", "Split"),
    ] {
        let graph = json!({"entryPoint":"a","steps":{
            "a":audit_loop("a",left,1,configured_wait_body(60_000,11,"First approval")),
            "b":audit_loop("b",right,1,configured_wait_body(120_000,22,"Second approval")),
            "finish":finish("finish")
        },"executionPlan":[{"fromStep":"a","toStep":"b"},{"fromStep":"b","toStep":"finish"}]});
        let (_temp, artifact) =
            compile_configured_tracking("audit-registry-loops", graph, vec![], None, true);
        assert_configured_waits(&artifact, ["First approval", "Second approval"]);
    }
}

#[test]
fn audit_04_embedded_wait_settings_and_events_follow_the_child_graph() {
    let graph = json!({"entryPoint":"a","steps":{
        "a":{"id":"a","stepType":"EmbedWorkflow","childWorkflowId":"child-a","childVersion":"latest"},
        "b":{"id":"b","stepType":"EmbedWorkflow","childWorkflowId":"child-b","childVersion":"latest"},
        "finish":finish("finish")
    },"executionPlan":[{"fromStep":"a","toStep":"b"},{"fromStep":"b","toStep":"finish"}]});
    let children = [("a", 60_000, 11, "Child A"), ("b", 120_000, 22, "Child B")]
        .into_iter()
        .map(|(id, timeout, poll, label)| ChildWorkflowInput {
            step_id: id.into(),
            workflow_id: format!("child-{id}"),
            version_requested: "latest".into(),
            version_resolved: 1,
            execution_graph: serde_json::from_value(configured_wait_body(timeout, poll, label))
                .unwrap(),
        })
        .collect();
    let (_temp, artifact) =
        compile_configured_tracking("audit-registry-children", graph, children, None, true);
    assert_configured_waits(&artifact, ["Child A", "Child B"]);
}

#[test]
fn audit_04_on_wait_graph_can_shadow_its_parent_step_type() {
    let mut graph = configured_wait_body(60_000, 11, "Parent wait");
    graph["steps"]["wait"]["onWait"] = json!({"entryPoint":"wait","steps":{
        "wait":{"id":"wait","stepType":"Finish","name":"Notification finish","inputMapping":{"notification":immediate(json!("sent"))}}
    }});
    let (_temp, artifact) =
        compile_configured_tracking("audit-registry-onwait", graph, vec![], None, true);
    let host = Arc::new(CheckpointingRuntimeHost::new(b"{}"));
    *host.pinned_clock_ms.lock().unwrap() = Some(1_000_000);
    let key = signal_key(run_invoke_once(
        &artifact.wasm_path,
        host.clone(),
        b"{}".to_vec(),
    ));
    let events = host
        .custom_events
        .lock()
        .unwrap()
        .iter()
        .map(|(kind, bytes)| {
            (
                kind.clone(),
                serde_json::from_slice::<Value>(bytes).unwrap(),
            )
        })
        .collect::<Vec<_>>();
    let notification = events
        .iter()
        .find(|(kind, value)| {
            kind == "step_debug_end" && value["step_name"] == "Notification finish"
        })
        .expect("onWait Finish event");
    assert_eq!(notification.1["step_type"], "Finish");
    assert_eq!(notification.1["outputs"]["outputs"]["notification"], "sent");
    host.deliver_signal(&key, b"{}");
    assert_eq!(
        completed(run_invoke_once(&artifact.wasm_path, host, b"{}".to_vec())),
        json!({"ok":true})
    );
}

#[test]
fn audit_04_nested_step_type_and_debug_mapping_are_graph_local() {
    let body = json!({"entryPoint":"same","steps":{
        "same":{"id":"same","stepType":"Filter","name":"Nested filter","config":{"value":immediate(json!([1,2,3])),"condition":condition(true)}},
        "done":finish("done")
    },"executionPlan":[{"fromStep":"same","toStep":"done"}]});
    let graph = json!({"entryPoint":"loop","steps":{
        "loop":loop_step("loop",body),
        "same":{"id":"same","stepType":"Finish","name":"Root finish","inputMapping":{"result":immediate(json!("root"))}}
    },"executionPlan":[{"fromStep":"loop","toStep":"same"}]});
    let (_temp, artifact) =
        compile_configured_tracking("audit-registry-types", graph, vec![], None, true);
    let host = Arc::new(CheckpointingRuntimeHost::new(b"{}"));
    assert_eq!(
        completed(run_invoke_once(
            &artifact.wasm_path,
            host.clone(),
            b"{}".to_vec()
        )),
        json!({"result":"root"})
    );
    let events = host
        .custom_events
        .lock()
        .unwrap()
        .iter()
        .map(|(kind, bytes)| {
            (
                kind.clone(),
                serde_json::from_slice::<Value>(bytes).unwrap(),
            )
        })
        .collect::<Vec<_>>();
    let nested = events
        .iter()
        .find(|(kind, value)| kind == "step_debug_start" && value["step_name"] == "Nested filter")
        .expect("nested filter start");
    assert_eq!(nested.1["step_type"], "Filter");
    assert_eq!(nested.1["inputs"], json!([1, 2, 3]));
    let root = events
        .iter()
        .find(|(kind, value)| kind == "step_debug_end" && value["step_name"] == "Root finish")
        .expect("root Finish end");
    assert_eq!(root.1["step_type"], "Finish");
    assert_eq!(root.1["outputs"]["outputs"], json!({"result":"root"}));
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
fn audit_05_while_timeout_survives_suspend_resume() {
    assert_timeout_failure(resume_after_delay(60_000, false), "WHILE_TIMEOUT");
}

#[test]
fn audit_05_split_timeout_survives_suspend_resume() {
    assert_timeout_failure(resume_after_delay(60_000, true), "SPLIT_TIMEOUT");
}

fn audit_deadline_host(now: u64) -> Arc<CheckpointingRuntimeHost> {
    let host = Arc::new(CheckpointingRuntimeHost::new(b"{}"));
    *host.pinned_clock_ms.lock().unwrap() = Some(now);
    host
}
fn assert_at(exit: InvokeExit, deadline: u64) {
    let InvokeExit::Suspended(wakes) = exit else {
        panic!("expected timed suspension, got {exit:?}")
    };
    assert!(
        matches!(wakes.as_slice(), [WorkflowWake::At(at)] if *at == deadline),
        "{wakes:?}"
    );
}

#[test]
fn audit_05_wakes_clamp_and_early_replay_keeps_the_original_deadline() {
    for split in [false, true] {
        let (_temp, artifact) = compile("audit-clamp", timeout_graph(60_000, split));
        let host = audit_deadline_host(1_000_000);
        let invoke = || run_invoke_once(&artifact.wasm_path, host.clone(), b"{}".to_vec());
        assert_at(invoke(), 1_001_000);
        let recorded = host.checkpoints.lock().unwrap().clone();
        *host.pinned_clock_ms.lock().unwrap() = Some(1_000_999);
        assert_at(invoke(), 1_001_000);
        assert_eq!(*host.checkpoints.lock().unwrap(), recorded);
        *host.pinned_clock_ms.lock().unwrap() = Some(1_001_000);
        assert_timeout_failure(
            invoke(),
            if split {
                "SPLIT_TIMEOUT"
            } else {
                "WHILE_TIMEOUT"
            },
        );
    }
}

#[test]
fn audit_05_completed_loops_do_not_expire_during_later_replay() {
    for split in [false, true] {
        for durable in [false, true] {
            let mut graph = timeout_graph(0, split);
            graph["steps"]["loop"]["subgraph"] =
                json!({"entryPoint":"body_finish","steps":{"body_finish":finish("body_finish")}});
            if split {
                graph["steps"]["loop"]["durable"] = json!(durable);
            }
            graph["steps"]["later"] =
                json!({"id":"later","stepType":"Delay","durationMs":immediate(json!(60_000))});
            graph["executionPlan"] = json!([{"fromStep":"loop","toStep":"later"},{"fromStep":"later","toStep":"finish"}]);
            let (_temp, artifact) = compile("audit-completed", graph);
            let host = audit_deadline_host(1_000_000);
            let invoke = || run_invoke_once(&artifact.wasm_path, host.clone(), b"{}".to_vec());
            assert_at(invoke(), 1_060_000);
            assert!(host.checkpoints.lock().unwrap().iter().any(|(key, value)| {
                key.starts_with("runtara:v2:[\"loop-complete\"") && value == &[1]
            }));
            *host.pinned_clock_ms.lock().unwrap() = Some(1_060_001);
            assert_eq!(completed(invoke()), json!({"ok":true}));
        }
    }
}

#[test]
fn audit_05_final_body_overrun_fails_without_suspending() {
    for split in [false, true] {
        let mut graph = timeout_graph(0, split);
        graph["steps"]["loop"]["subgraph"] =
            json!({"entryPoint":"body_finish","steps":{"body_finish":finish("body_finish")}});
        let (_temp, artifact) =
            compile_configured_tracking("audit-final-overrun", graph, vec![], None, true);
        let host = audit_deadline_host(1_000_000);
        *host.advance_clock_on_step_end.lock().unwrap() = Some(("body_finish".into(), 1_001));
        assert_timeout_failure(
            run_invoke_once(&artifact.wasm_path, host, b"{}".to_vec()),
            if split {
                "SPLIT_TIMEOUT"
            } else {
                "WHILE_TIMEOUT"
            },
        );
    }
}

#[test]
fn audit_05_signal_wakes_respect_the_enclosing_budget() {
    for split in [false, true] {
        for wait_timeout in [None, Some(500), Some(60_000)] {
            let mut graph = timeout_graph(0, split);
            let mut wait = json!({"id":"wait","stepType":"WaitForSignal"});
            if let Some(ms) = wait_timeout {
                wait["timeoutMs"] = immediate(json!(ms));
            }
            graph["steps"]["loop"]["subgraph"] = json!({"entryPoint":"wait","steps":{"wait":wait,"body_finish":finish("body_finish")},"executionPlan":[{"fromStep":"wait","toStep":"body_finish"}]});
            let (_temp, artifact) = compile("audit-wait-budget", graph);
            let host = audit_deadline_host(1_000_000);
            let invoke = || run_invoke_once(&artifact.wasm_path, host.clone(), b"{}".to_vec());
            let first = invoke();
            let InvokeExit::Suspended(ref wakes) = first else {
                panic!("{first:?}")
            };
            let WorkflowWake::OnSignal(ref wait) = wakes[0] else {
                panic!("{wakes:?}")
            };
            assert_eq!(
                wait.deadline_ms,
                Some(1_000_000 + wait_timeout.unwrap_or(1_000).min(1_000))
            );
            let key = signal_key(first);
            *host.pinned_clock_ms.lock().unwrap() = Some(1_000_250);
            host.deliver_signal(&key, b"{}");
            assert_eq!(completed(invoke()), json!({"ok":true}));
        }
    }
}

#[test]
fn audit_05_nested_mixed_loops_keep_the_earliest_budget() {
    for (outer, inner) in [
        ("While", "While"),
        ("While", "Split"),
        ("Split", "While"),
        ("Split", "Split"),
    ] {
        for outer_is_earlier in [false, true] {
            let leaf = timeout_graph(60_000, false)["steps"]["loop"]["subgraph"].clone();
            let mut inner_step = audit_loop("inner", inner, 1, leaf);
            inner_step["config"]["timeout"] = json!(if outer_is_earlier { 2_000 } else { 500 });
            let mut outer_step = audit_loop(
                "outer",
                outer,
                1,
                json!({"entryPoint":"inner","steps":{"inner":inner_step,"done":finish("done")},"executionPlan":[{"fromStep":"inner","toStep":"done"}]}),
            );
            outer_step["config"]["timeout"] = json!(1_000);
            let graph = json!({"entryPoint":"outer","steps":{"outer":outer_step,"finish":finish("finish")},"executionPlan":[{"fromStep":"outer","toStep":"finish"}]});
            let (_temp, artifact) = compile("audit-nested-deadlines", graph);
            let host = audit_deadline_host(1_000_000);
            let invoke = || run_invoke_once(&artifact.wasm_path, host.clone(), b"{}".to_vec());
            let deadline = if outer_is_earlier {
                1_001_000
            } else {
                1_000_500
            };
            assert_at(invoke(), deadline);
            *host.pinned_clock_ms.lock().unwrap() = Some(deadline);
            let owner = if outer_is_earlier { outer } else { inner };
            assert_timeout_failure(
                invoke(),
                if owner == "Split" {
                    "SPLIT_TIMEOUT"
                } else {
                    "WHILE_TIMEOUT"
                },
            );
        }
    }
}

#[test]
fn audit_05_sibling_loop_budgets_start_at_each_invocation() {
    for kind in ["While", "Split"] {
        let leaf = timeout_graph(500, false)["steps"]["loop"]["subgraph"].clone();
        let timed = |id: &str| {
            let mut step = audit_loop(id, kind, 1, leaf.clone());
            step["config"]["timeout"] = json!(1_000);
            step
        };
        let graph = json!({"entryPoint":"a","steps":{"a":timed("a"),"b":timed("b"),"finish":finish("finish")},"executionPlan":[{"fromStep":"a","toStep":"b"},{"fromStep":"b","toStep":"finish"}]});
        let (_temp, artifact) = compile("audit-sibling-budgets", graph);
        let host = audit_deadline_host(1_000_000);
        let invoke = || run_invoke_once(&artifact.wasm_path, host.clone(), b"{}".to_vec());
        assert_at(invoke(), 1_000_500);
        *host.pinned_clock_ms.lock().unwrap() = Some(1_000_600);
        assert_at(invoke(), 1_001_100);
        *host.pinned_clock_ms.lock().unwrap() = Some(1_001_200);
        assert_eq!(completed(invoke()), json!({"ok":true}));
    }
}

#[test]
fn audit_05_invalid_deadline_checkpoint_fails_instead_of_resetting() {
    for split in [false, true] {
        let (_temp, artifact) = compile("audit-invalid-deadline", timeout_graph(60_000, split));
        let host = audit_deadline_host(1_000_000);
        let invoke = || run_invoke_once(&artifact.wasm_path, host.clone(), b"{}".to_vec());
        assert_at(invoke(), 1_001_000);
        for (key, value) in host.checkpoints.lock().unwrap().iter_mut() {
            if key.starts_with("runtara:v2:[\"loop-deadline\"") {
                *value = vec![1, 2, 3];
            }
        }
        assert_timeout_failure(invoke(), "LOOP_DEADLINE_STATE");
    }
}

#[test]
fn audit_05_zero_and_overflowing_timeouts_have_defined_boundaries() {
    for split in [false, true] {
        let mut graph = timeout_graph(60_000, split);
        graph["steps"]["loop"]["config"]["timeout"] = json!(0);
        let (_temp, artifact) = compile("audit-zero-deadline", graph.clone());
        let host = audit_deadline_host(0);
        assert_at(
            run_invoke_once(&artifact.wasm_path, host.clone(), b"{}".to_vec()),
            60_000,
        );
        assert!(
            !host
                .checkpoints
                .lock()
                .unwrap()
                .keys()
                .any(|key| key.contains("loop-deadline"))
        );
        graph["steps"]["loop"]["config"]["timeout"] = json!(u64::MAX);
        let (_temp, artifact) = compile("audit-overflow-deadline", graph);
        let host = audit_deadline_host(1_000_000);
        assert_at(
            run_invoke_once(&artifact.wasm_path, host.clone(), b"{}".to_vec()),
            1_060_000,
        );
        assert!(
            host.checkpoints
                .lock()
                .unwrap()
                .iter()
                .any(|(k, v)| k.starts_with("runtara:v2:[\"loop-deadline\"")
                    && v.as_slice() == u64::MAX.to_le_bytes())
        );
    }
}

#[test]
fn audit_05_split_retry_wait_does_not_extend_the_total_budget() {
    let mut graph: Value = serde_json::from_str(&super::lifecycle_retry_split_graph()).unwrap();
    graph["steps"]["split"]["config"]["timeout"] = json!(1_000);
    let (_temp, artifact) = compile("audit-retry-budget", graph);
    let host = audit_deadline_host(1_000_000);
    let invoke = || run_invoke_once(&artifact.wasm_path, host.clone(), b"{}".to_vec());
    assert_at(invoke(), 1_001_000);
    *host.pinned_clock_ms.lock().unwrap() = Some(1_001_000);
    assert_timeout_failure(invoke(), "SPLIT_TIMEOUT");
}

#[test]
fn audit_05_embedded_child_wake_respects_parent_loop_deadline() {
    for split in [false, true] {
        let mut graph = timeout_graph(0, split);
        graph["steps"]["loop"]["subgraph"] = json!({"entryPoint":"call","steps":{
            "call":{"id":"call","stepType":"EmbedWorkflow","childWorkflowId":"child","childVersion":"latest"},
            "body_finish":finish("body_finish")
        },"executionPlan":[{"fromStep":"call","toStep":"body_finish"}]});
        let child = timeout_graph(60_000, false)["steps"]["loop"]["subgraph"].clone();
        let (_temp, artifact) = compile_with_children(
            "audit-child-budget",
            graph,
            vec![ChildWorkflowInput {
                step_id: "call".into(),
                workflow_id: "child".into(),
                version_requested: "latest".into(),
                version_resolved: 1,
                execution_graph: serde_json::from_value(child).unwrap(),
            }],
        );
        let host = audit_deadline_host(1_000_000);
        let invoke = || run_invoke_once(&artifact.wasm_path, host.clone(), b"{}".to_vec());
        assert_at(invoke(), 1_001_000);
        *host.pinned_clock_ms.lock().unwrap() = Some(1_001_000);
        assert_timeout_failure(
            invoke(),
            if split {
                "SPLIT_TIMEOUT"
            } else {
                "WHILE_TIMEOUT"
            },
        );
    }
}

#[test]
fn audit_05_handled_timeout_removes_the_expired_scope_before_recovery() {
    let mut graph = timeout_graph(60_000, false);
    graph["executionPlan"]
        .as_array_mut()
        .unwrap()
        .push(json!({"fromStep":"loop","toStep":"recover","label":"onError"}));
    graph["steps"]["recover"] =
        json!({"id":"recover","stepType":"Delay","durationMs":immediate(json!(60_000))});
    graph["executionPlan"]
        .as_array_mut()
        .unwrap()
        .push(json!({"fromStep":"recover","toStep":"finish"}));
    let (_temp, artifact) = compile("audit-timeout-recovery", graph);
    let host = audit_deadline_host(1_000_000);
    let invoke = || run_invoke_once(&artifact.wasm_path, host.clone(), b"{}".to_vec());
    assert_at(invoke(), 1_001_000);
    *host.pinned_clock_ms.lock().unwrap() = Some(1_001_000);
    assert_at(invoke(), 1_061_000);
    *host.pinned_clock_ms.lock().unwrap() = Some(1_061_001);
    assert_eq!(completed(invoke()), json!({"ok":true}));
}

#[test]
fn audit_05_child_timeout_unwind_restores_the_parent_budget() {
    let graph = json!({"entryPoint":"call","steps":{
        "call":{"id":"call","stepType":"EmbedWorkflow","childWorkflowId":"child","childVersion":"latest","maxRetries":0},
        "recover":{"id":"recover","stepType":"Delay","durationMs":immediate(json!(60_000))},
        "finish":finish("finish")
    },"executionPlan":[{"fromStep":"call","toStep":"finish"},{"fromStep":"call","toStep":"recover","label":"onError"},{"fromStep":"recover","toStep":"finish"}]});
    let (_temp, artifact) = compile_with_children(
        "audit-child-timeout-unwind",
        graph,
        vec![ChildWorkflowInput {
            step_id: "call".into(),
            workflow_id: "child".into(),
            version_requested: "latest".into(),
            version_resolved: 1,
            execution_graph: serde_json::from_value(timeout_graph(60_000, false)).unwrap(),
        }],
    );
    let host = audit_deadline_host(1_000_000);
    let invoke = || run_invoke_once(&artifact.wasm_path, host.clone(), b"{}".to_vec());
    assert_at(invoke(), 1_001_000);
    *host.pinned_clock_ms.lock().unwrap() = Some(1_001_000);
    assert_at(invoke(), 1_061_000);
    *host.pinned_clock_ms.lock().unwrap() = Some(1_061_001);
    assert_eq!(completed(invoke()), json!({"ok":true}));
}

#[test]
fn audit_05_aggregated_inner_failure_does_not_leak_its_budget() {
    let mut inner = audit_loop(
        "inner",
        "While",
        1,
        json!({"entryPoint":"error","steps":{
            "error":{"id":"error","stepType":"Error","code":"ITEM_ERROR","message":"expected"}
        }}),
    );
    inner["config"]["timeout"] = json!(1_000);
    let mut outer = audit_loop(
        "outer",
        "Split",
        2,
        json!({"entryPoint":"inner","steps":{"inner":inner,"done":finish("done")},"executionPlan":[{"fromStep":"inner","toStep":"done"}]}),
    );
    outer["config"]["dontStopOnFailed"] = json!(true);
    let graph = json!({"entryPoint":"outer","steps":{
        "outer":outer,"later":{"id":"later","stepType":"Delay","durationMs":immediate(json!(60_000))},"finish":finish("finish")
    },"executionPlan":[{"fromStep":"outer","toStep":"later"},{"fromStep":"later","toStep":"finish"}]});
    let (_temp, artifact) = compile("audit-aggregate-budget", graph);
    let host = audit_deadline_host(1_000_000);
    assert_at(
        run_invoke_once(&artifact.wasm_path, host, b"{}".to_vec()),
        1_060_000,
    );
}
