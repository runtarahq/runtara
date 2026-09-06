// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Native reproductions for docs/wasm-emitter-audit.md.
//! Ignored cases assert the desired contract, not the observed defect.
#![cfg(feature = "compiler")]

use runtara_dsl::{ExecutionGraph, agent_meta::AgentCatalog};
use runtara_workflow_stdlib::direct_json::DirectJsonManifest;
use runtara_workflows::{
    compile::ChildWorkflowInput,
    direct_wasm::{
        DirectCompilationInput, DirectManifestChildWorkflowInput, analyze_direct_wasm_support,
        build_direct_workflow_manifest,
        build_direct_workflow_manifest_with_child_workflows_and_agent_catalog,
        compile_direct_workflow,
    },
    validation::validate_workflow,
};
use serde_json::{Value, json};

fn graph(value: Value) -> ExecutionGraph {
    serde_json::from_value(value).expect("audit fixture parses")
}

fn immediate(value: Value) -> Value {
    json!({"valueType": "immediate", "value": value})
}

fn finish(id: &str) -> Value {
    json!({"id": id, "stepType": "Finish", "inputMapping": {"ok": immediate(json!(true))}})
}

fn leaf(id: &str) -> Value {
    json!({"entryPoint": id, "steps": {id: finish(id)}})
}

fn loop_step(id: &str, body: Value) -> Value {
    json!({"id": id, "stepType": "While", "condition": {
        "type": "operation", "op": "EQ", "arguments": [immediate(json!(1)), immediate(json!(1))]
    }, "config": {"maxIterations": 1}, "subgraph": body})
}

fn wait_body(id: &str, timeout: u64) -> Value {
    json!({"entryPoint": id, "steps": {
        id: {"id": id, "stepType": "WaitForSignal", "timeoutMs": immediate(json!(timeout))},
        "done": finish("done")
    }, "executionPlan": [{"fromStep": id, "toStep": "done"}]})
}

fn wait_registry(second_id: &str, embedded: bool) -> DirectJsonManifest {
    let a = wait_body("wait", 100);
    let b = wait_body(second_id, 200);
    let step = |id: &str, body| {
        if embedded {
            json!({"id": id, "stepType": "EmbedWorkflow", "childWorkflowId": id, "childVersion": "1"})
        } else {
            loop_step(id, body)
        }
    };
    let root = graph(json!({"entryPoint": "a", "steps": {
        "a": step("a", a.clone()), "b": step("b", b.clone()), "finish": finish("finish")
    }, "executionPlan": [{"fromStep": "a", "toStep": "b"}, {"fromStep": "b", "toStep": "finish"}]}));
    let child_a = graph(a);
    let child_b = graph(b);
    let manifest = if embedded {
        let children = [
            DirectManifestChildWorkflowInput {
                step_id: "a",
                workflow_id: "a",
                version_requested: "1",
                version_resolved: 1,
                execution_graph: &child_a,
            },
            DirectManifestChildWorkflowInput {
                step_id: "b",
                workflow_id: "b",
                version_requested: "1",
                version_resolved: 1,
                execution_graph: &child_b,
            },
        ];
        build_direct_workflow_manifest_with_child_workflows_and_agent_catalog(
            &root, &children, None,
        )
    } else {
        assert!(
            validate_workflow(&root, &AgentCatalog::default())
                .errors
                .is_empty()
        );
        assert!(analyze_direct_wasm_support(&root).supported);
        build_direct_workflow_manifest(&root)
    }
    .expect("manifest builds");
    DirectJsonManifest::parse(&manifest.to_canonical_json().unwrap())
        .expect("runtime manifest parses")
}

fn assert_wait_configuration(second_id: &str, embedded: bool) {
    let runtime = wait_registry(second_id, embedded);
    let source = |scope: &str| {
        serde_json::to_vec(&json!({"variables": {
            "_scope_id": format!("sc_{scope}_0"), "_loop_indices": [0],
            "_manifest_graph_path": [[if embedded {"embedWorkflow"} else {"while.subgraph"}, scope]],
            "_cache_key_prefix": if embedded {format!("audit::{scope}")} else {String::new()}
        }}))
        .unwrap()
    };
    assert_eq!(
        runtime.wait_timeout_ms("wait", &source("a")).unwrap(),
        Some(100)
    );
    assert_eq!(
        runtime.wait_timeout_ms(second_id, &source("b")).unwrap(),
        Some(200),
        "AUDIT-04: the second scope must use its own timeout"
    );
}

#[test]
fn audit_04_distinct_loop_step_ids_keep_their_configuration() {
    assert_wait_configuration("wait_b", false);
}

#[test]
fn audit_04_duplicate_loop_step_ids_keep_their_configuration() {
    assert_wait_configuration("wait", false);
}

#[test]
fn audit_04_distinct_child_step_ids_keep_their_configuration() {
    assert_wait_configuration("wait_b", true);
}

#[test]
fn audit_04_duplicate_child_step_ids_keep_their_configuration() {
    assert_wait_configuration("wait", true);
}

fn compile_retry_graph(retries: u32, embedded: bool) -> Result<(), String> {
    let temp = tempfile::tempdir().unwrap();
    let step = if embedded {
        json!({"id": "call", "stepType": "EmbedWorkflow", "childWorkflowId": "child", "childVersion": "1", "maxRetries": retries})
    } else {
        json!({"id": "call", "stepType": "Agent", "agentId": "utils", "capabilityId": "return-input",
            "maxRetries": retries, "inputMapping": {"value": immediate(json!(1))}})
    };
    let root = graph(
        json!({"entryPoint": "call", "steps": {"call": step, "finish": finish("finish")},
        "executionPlan": [{"fromStep": "call", "toStep": "finish"}]}),
    );
    let children = if embedded {
        vec![ChildWorkflowInput {
            step_id: "call".into(),
            workflow_id: "child".into(),
            version_requested: "1".into(),
            version_resolved: 1,
            execution_graph: graph(leaf("child_finish")),
        }]
    } else {
        vec![]
    };
    compile_direct_workflow(DirectCompilationInput {
        workflow_id: "audit-retries".into(),
        version: 1,
        source_checksum: None,
        execution_graph: root,
        child_workflows: children,
        output_dir: temp.path().into(),
        track_events: false,
        agent_catalog: None,
        agent_slug: None,
    })
    .map(|_| ())
    .map_err(|err| err.to_string())
}

#[test]
fn audit_06_agent_retry_boundaries_below_overflow_compile() {
    for retries in [0, 1, u32::MAX - 1] {
        compile_retry_graph(retries, false).expect("representable retry count compiles");
    }
}

#[test]
fn audit_06_embed_retry_boundaries_below_overflow_compile() {
    for retries in [0, 1, u32::MAX - 1] {
        compile_retry_graph(retries, true).expect("representable retry count compiles");
    }
}

fn assert_overflow_is_rejected(embedded: bool) {
    let result = std::panic::catch_unwind(|| compile_retry_graph(u32::MAX, embedded));
    let result =
        result.expect("AUDIT-06: unsupported retry count must return an error, never panic");
    assert!(
        result.is_err(),
        "AUDIT-06: maxRetries + 1 must not wrap in release builds"
    );
}

#[test]
#[ignore = "AUDIT-06: u32::MAX retries overflows during Agent lowering"]
fn audit_06_agent_retry_overflow_returns_compile_error() {
    assert_overflow_is_rejected(false);
}

#[test]
#[ignore = "AUDIT-06: u32::MAX retries overflows during EmbedWorkflow lowering"]
fn audit_06_embed_retry_overflow_returns_compile_error() {
    assert_overflow_is_rejected(true);
}

#[test]
fn audit_07_matching_step_keys_validate_and_compile() {
    let root = graph(leaf("finish"));
    assert!(
        validate_workflow(&root, &AgentCatalog::default())
            .errors
            .is_empty()
    );
    let temp = tempfile::tempdir().unwrap();
    compile_direct_workflow(DirectCompilationInput {
        workflow_id: "audit-ids".into(),
        version: 1,
        source_checksum: None,
        execution_graph: root,
        child_workflows: vec![],
        output_dir: temp.path().into(),
        track_events: false,
        agent_catalog: None,
        agent_slug: None,
    })
    .expect("consistent graph compiles");
}

fn assert_bad_identity_rejected(root: Value) {
    let errors = validate_workflow(&graph(root), &AgentCatalog::default()).errors;
    assert!(
        !errors.is_empty(),
        "AUDIT-07: inconsistent map key / inner ID must fail validation"
    );
}

#[test]
#[ignore = "AUDIT-07: validation checks map keys but does not compare inner IDs"]
fn audit_07_root_key_id_mismatch_is_rejected() {
    assert_bad_identity_rejected(
        json!({"entryPoint": "finish", "steps": {"finish": finish("different")}}),
    );
}

#[test]
#[ignore = "AUDIT-07: nested graph identities are not checked either"]
fn audit_07_nested_key_id_mismatch_is_rejected() {
    let body = json!({"entryPoint": "finish", "steps": {"finish": finish("different")}});
    assert_bad_identity_rejected(json!({"entryPoint": "loop", "steps": {
        "loop": loop_step("loop", body), "finish": finish("finish")
    }, "executionPlan": [{"fromStep": "loop", "toStep": "finish"}]}));
}

#[test]
#[ignore = "AUDIT-07: different map keys can carry identical inner IDs"]
fn audit_07_duplicate_inner_ids_are_rejected() {
    assert_bad_identity_rejected(json!({"entryPoint": "log", "steps": {
        "log": {"id": "finish", "stepType": "Log", "message": "audit"}, "finish": finish("finish")
    }, "executionPlan": [{"fromStep": "log", "toStep": "finish"}]}));
}
