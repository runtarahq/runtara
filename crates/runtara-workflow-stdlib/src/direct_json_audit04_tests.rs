// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Graph-local configuration and metadata must follow the defining graph.
use super::*;
use serde_json::json;

fn source(path: Value) -> Value {
    json!({"data":{},"steps":{},"variables":{"_manifest_graph_path":path,
        "_durable_key_version":2,"_loop_path":[],"_workflow_id":"wf"}})
}
fn bytes(value: &Value) -> Vec<u8> {
    serde_json::to_vec(value).unwrap()
}
fn imm(value: Value) -> Value {
    json!({"valueType":"immediate","value":value})
}
fn waits(owner: &str) -> DirectJsonManifest {
    let wait = |n| {
        json!({"id":"same","stepType":"WaitForSignal","name":format!("wait-{n}"),"body":{
            "timeoutMs":imm(json!(n)),"pollIntervalMs":n,"responseSchema":{"decision":{"enum":[n]}},"action":{"key":format!("action-{n}")}
        }})
    };
    DirectJsonManifest::parse(&bytes(&json!({"graph":{"steps":[wait(100),
        {"id":owner,"stepType":"While","nestedGraphs":[{"role":"while.subgraph","graph":{"steps":[wait(200)]}}]}
    ]}}))).unwrap()
}

#[test]
fn audit_04_scoped_wait_lookup_has_no_unqualified_fallback() {
    let manifest = waits("outer/::[]λ");
    let nested = bytes(&source(json!([["while.subgraph", "outer/::[]λ"]])));
    assert_eq!(
        manifest.wait_timeout_ms("same", &nested).unwrap(),
        Some(200)
    );
    assert_eq!(
        manifest
            .wait_poll_interval_ms_scoped("same", &nested)
            .unwrap(),
        200
    );
    let event: Value =
        serde_json::from_slice(&manifest.wait_event("same", "signal", &nested).unwrap()).unwrap();
    assert_eq!(event["step_name"], "wait-200");
    assert_eq!(event["action_key"], "action-200");
    assert_eq!(event["response_schema"]["decision"]["enum"], json!([200]));
    for path in [json!([["while.subgraph", "missing"]]), json!(42)] {
        let err = manifest
            .wait_timeout_ms("same", &bytes(&source(path)))
            .unwrap_err();
        assert!(err.contains("graph"), "{err}");
    }
    assert_eq!(
        manifest
            .wait_timeout_ms("same", br#"{"variables":{}}"#)
            .unwrap(),
        Some(100),
        "legacy artifacts retain their original route"
    );
    assert_eq!(manifest.wait_poll_interval_ms("same").unwrap(), 100);
}

#[test]
fn audit_04_loop_scope_cannot_be_replaced_by_authored_variables() {
    let parent = source(json!([["waitForSignal.onWait", "notify"]]));
    let config = json!({"variables":{"_manifest_graph_path":imm(json!([]))}});
    let split = DirectJsonSplit {
        step_id: "same".into(),
        name: None,
        value: config.clone(),
        input_schema: json!({}),
        output_schema: json!({}),
    };
    let while_step = DirectJsonWhile {
        step_id: "same".into(),
        name: None,
        value: config,
        condition: json!(true),
    };
    let split_vars = split_iteration_variables(&split, &parent, Value::Null, 3).unwrap();
    let while_vars = while_iteration_variables(
        &while_step,
        &parent,
        &DirectJsonWhileState {
            index: 3,
            outputs: Value::Null,
        },
    )
    .unwrap();
    assert_eq!(
        split_vars["_manifest_graph_path"],
        json!([
            ["waitForSignal.onWait", "notify"],
            ["split.subgraph", "same"]
        ])
    );
    assert_eq!(
        while_vars["_manifest_graph_path"],
        json!([
            ["waitForSignal.onWait", "notify"],
            ["while.subgraph", "same"]
        ])
    );
    let start = build_source(
        br#"{"data":{},"variables":{"_manifest_graph_path":[["while.subgraph","forged"]]}}"#,
        &bytes(&parent["variables"]),
        b"{}",
    )
    .unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&start).unwrap()["variables"]["_manifest_graph_path"],
        parent["variables"]["_manifest_graph_path"]
    );
}

#[test]
fn audit_04_debug_finish_mapping_is_bound_to_the_defining_graph() {
    let graph = |id, n| {
        json!({"steps":[{"id":"same","stepType":"Finish","name":format!("finish-{n}"),"body":{}}],
        "mappings":[{"id":id,"stepId":"same","purpose":"finish.inputMapping","value":{"marker":imm(json!(n))}}]})
    };
    let mut root = graph(0, 1);
    root["steps"].as_array_mut().unwrap().push(json!({"id":"scope","stepType":"While","nestedGraphs":[{"role":"while.subgraph","graph":graph(1,2)}]}));
    let manifest = DirectJsonManifest::parse(&bytes(&json!({"graph":root}))).unwrap();
    for (path, n) in [(json!([]), 1), (json!([["while.subgraph", "scope"]]), 2)] {
        let input = bytes(&source(path));
        let start: Value =
            serde_json::from_slice(&manifest.step_debug_start("same", &input).unwrap()).unwrap();
        assert_eq!(start["input_mapping"]["marker"], imm(json!(n)));
        let end: Value =
            serde_json::from_slice(&manifest.step_debug_end("same", &input, 0, 0).unwrap())
                .unwrap();
        assert_eq!(end["outputs"]["outputs"], json!({"marker":n}));
        let breakpoint: Value =
            serde_json::from_slice(&manifest.breakpoint_event("same", &input).unwrap()).unwrap();
        assert_eq!(breakpoint["inputs"], json!({"marker":n}));
    }
}

#[test]
fn audit_04_ai_debug_selects_main_agent_among_memory_and_tool_records() {
    let mapping = |id, purpose, marker| json!({"id":id,"stepId":"ai","purpose":purpose,"value":{"marker":imm(json!(marker))}});
    let agent = |id, purpose, mapping| json!({"id":id,"stepId":"ai","purpose":purpose,"agentId":"ai-tools","capabilityId":"chat-turn","inputMappingId":mapping});
    let manifest=DirectJsonManifest::parse(&bytes(&json!({"graph":{
        "steps":[{"id":"ai","stepType":"AiAgent","body":{}}],
        "mappings":[mapping(0,"agent.inputMapping","main"),mapping(1,"memory.conversation","memory")],
        "agents":[agent(0,"agent.config",0),agent(1,"memory.load",1),agent(2,"mcp.tool",1)]
    }}))).unwrap();
    let event: Value = serde_json::from_slice(
        &manifest
            .step_debug_start("ai", &bytes(&source(json!([]))))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(event["inputs"], json!({"marker":"main"}));
    assert_eq!(event["input_mapping"]["marker"], imm(json!("main")));
}

#[test]
fn audit_04_overlapping_debug_timers_do_not_overwrite_other_scopes() {
    let manifest = waits("outer");
    let root = bytes(&source(json!([])));
    let nested = bytes(&source(json!([["while.subgraph", "outer"]])));
    manifest
        .wait_debug_start("same", "root-signal", Some(100), &root)
        .unwrap();
    manifest
        .wait_debug_start("same", "nested-signal", Some(200), &nested)
        .unwrap();
    assert_eq!(manifest.debug_start_ms.borrow().len(), 2);
    manifest
        .step_debug_error("same", &root, b"root-error")
        .unwrap();
    assert_eq!(
        manifest.debug_start_ms.borrow().len(),
        1,
        "root completion leaves the nested timer intact"
    );
    manifest
        .step_debug_error("same", &nested, b"nested-error")
        .unwrap();
    assert!(manifest.debug_start_ms.borrow().is_empty());
}

#[test]
fn audit_04_large_interned_graph_paths_keep_the_same_definition() {
    reset_value_store();
    let owner = "long".repeat(20_000);
    let manifest = waits(&owner);
    let plain = source(json!([["while.subgraph", owner]]));
    let interned = build_source(b"{}", &bytes(&plain["variables"]), b"{}").unwrap();
    assert!(wfref_id(&serde_json::from_slice::<Value>(&interned).unwrap()["variables"]["_manifest_graph_path"]).is_some());
    assert_eq!(
        manifest.wait_timeout_ms("same", &interned).unwrap(),
        Some(200)
    );
    assert_eq!(
        manifest
            .wait_poll_interval_ms_scoped("same", &interned)
            .unwrap(),
        200
    );
}
