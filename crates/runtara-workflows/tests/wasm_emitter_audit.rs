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

fn retry_graph(retries: Option<u32>, kind: &str) -> ExecutionGraph {
    let mut step = match kind {
        "embed" => {
            json!({"id": "call", "stepType": "EmbedWorkflow", "childWorkflowId": "child", "childVersion": "latest"})
        }
        "split" => {
            json!({"id": "call", "stepType": "Split", "config": {"value": immediate(json!([1])), "sequential": true}, "subgraph": leaf("item_finish")})
        }
        "ai" | "ai-tools" => {
            json!({"id": "call", "stepType": "AiAgent", "connectionId": "test-connection", "config": {
                "systemPrompt": immediate(json!("Answer.")), "userPrompt": immediate(json!("Hello")),
                "provider": immediate(json!("openai")), "model": immediate(json!("test-model"))
            }})
        }
        "agent" => {
            json!({"id": "call", "stepType": "Agent", "agentId": "utils", "capabilityId": "return-input", "inputMapping": {"value": immediate(json!(1))}})
        }
        _ => panic!("unknown retry fixture"),
    };
    if let Some(retries) = retries {
        if matches!(kind, "split" | "ai" | "ai-tools") {
            step["config"]["maxRetries"] = json!(retries);
        } else {
            step["maxRetries"] = json!(retries);
        }
    }
    let mut root = json!({"entryPoint": "call", "steps": {"call": step, "finish": finish("finish")},
        "executionPlan": [{"fromStep": "call", "toStep": "finish"}]});
    if kind == "ai-tools" {
        root["steps"]["echo"] = json!({"id": "echo", "stepType": "Agent", "name": "Echo", "agentId": "utils", "capabilityId": "return-input", "inputMapping": {"value": immediate(json!(1))}});
        root["executionPlan"]
            .as_array_mut()
            .unwrap()
            .push(json!({"fromStep": "call", "toStep": "echo", "label": "echo"}));
    }
    graph(root)
}

fn retry_children(kind: &str) -> Vec<ChildWorkflowInput> {
    if kind == "embed" {
        vec![ChildWorkflowInput {
            step_id: "call".into(),
            workflow_id: "child".into(),
            version_requested: "latest".into(),
            version_resolved: 1,
            execution_graph: graph(leaf("child_finish")),
        }]
    } else {
        vec![]
    }
}

fn compile_retry_input(
    root: ExecutionGraph,
    children: Vec<ChildWorkflowInput>,
) -> Result<(), runtara_workflows::direct_wasm::DirectCompileError> {
    let temp = tempfile::tempdir().unwrap();
    let result = compile_direct_workflow(DirectCompilationInput {
        workflow_id: "audit-retries".into(),
        version: 1,
        source_checksum: None,
        execution_graph: root,
        child_workflows: children,
        output_dir: temp.path().into(),
        track_events: false,
        agent_catalog: None,
        agent_slug: None,
    });
    match result {
        Ok(artifact) => {
            wasmparser::Validator::new_with_features(wasmparser::WasmFeatures::all())
                .validate_all(&std::fs::read(artifact.wasm_path).unwrap())
                .expect("emitted component validates");
            Ok(())
        }
        Err(error) => {
            assert!(
                !temp
                    .path()
                    .join("audit-retries-v1-direct/workflow-logic.wasm")
                    .exists(),
                "rejected graph must not emit executable bytes"
            );
            Err(error)
        }
    }
}

fn assert_retry_boundaries(kind: &str) {
    for retries in [
        None,
        Some(0),
        Some(1),
        Some(i32::MAX as u32),
        Some(i32::MAX as u32 + 1),
        Some(u32::MAX - 2),
        Some(u32::MAX - 1),
    ] {
        let root = retry_graph(retries, kind);
        let validation = validate_workflow(
            &root,
            &AgentCatalog::from_json(include_str!("catalog/agent_catalog.json")).unwrap(),
        );
        assert!(
            validation.errors.is_empty(),
            "{kind} {retries:?}: {:?}",
            validation.errors
        );
        compile_retry_input(root, retry_children(kind))
            .expect("representable retry count compiles");
    }
}

fn assert_retry_error(error: runtara_workflows::direct_wasm::DirectCompileError) {
    let runtara_workflows::direct_wasm::DirectCompileError::Unsupported { report } = error else {
        panic!("expected structured support error, got {error:?}");
    };
    let issue = report
        .unsupported
        .iter()
        .find(|issue| issue.feature == "retry-count-overflow")
        .expect("retry bound diagnostic");
    assert_eq!(issue.step_id.as_deref(), Some("call"));
    assert!(issue.reason.contains("4294967294"));
    assert!(issue.reason.contains("4294967295"));
}

fn assert_overflow_is_rejected(kind: &str) {
    let root = retry_graph(Some(u32::MAX), kind);
    let validation = validate_workflow(
        &root,
        &AgentCatalog::from_json(include_str!("catalog/agent_catalog.json")).unwrap(),
    );
    let error = validation.errors.iter().find(|error| matches!(error, runtara_workflows::validation::ValidationError::RetryCountOverflow { step_id, max_retries } if step_id == "call" && *max_retries == u32::MAX)).expect("structured validation error");
    assert_eq!(error.code(), "E129");
    assert!(error.to_string().contains("4294967294"));
    let result = std::panic::catch_unwind(|| compile_retry_input(root, retry_children(kind)))
        .expect("invalid retry count must return an error, never panic");
    assert_retry_error(result.expect_err("must not wrap in release builds"));
}

#[test]
fn audit_06_agent_retry_boundaries_below_overflow_compile() {
    assert_retry_boundaries("agent");
}
#[test]
fn audit_06_embed_retry_boundaries_below_overflow_compile() {
    assert_retry_boundaries("embed");
}
#[test]
fn audit_06_split_retry_boundaries_below_overflow_compile() {
    assert_retry_boundaries("split");
}
#[test]
fn audit_06_ai_retry_boundaries_below_overflow_compile() {
    assert_retry_boundaries("ai");
}
#[test]
fn audit_06_ai_tool_loop_retry_boundaries_below_overflow_compile() {
    assert_retry_boundaries("ai-tools");
}
#[test]
fn audit_06_agent_retry_overflow_returns_compile_error() {
    assert_overflow_is_rejected("agent");
}
#[test]
fn audit_06_embed_retry_overflow_returns_compile_error() {
    assert_overflow_is_rejected("embed");
}
#[test]
fn audit_06_split_retry_overflow_returns_compile_error() {
    assert_overflow_is_rejected("split");
}
#[test]
fn audit_06_ai_retry_overflow_returns_compile_error() {
    assert_overflow_is_rejected("ai");
}
#[test]
fn audit_06_ai_tool_loop_retry_overflow_returns_compile_error() {
    assert_overflow_is_rejected("ai-tools");
}

#[test]
fn audit_06_nested_retry_overflow_is_rejected() {
    for wrapper in ["while", "split", "onWait"] {
        let body = serde_json::to_value(retry_graph(Some(u32::MAX), "agent")).unwrap();
        let step = match wrapper {
            "while" => loop_step("outer", body),
            "split" => {
                json!({"id": "outer", "stepType": "Split", "config": {"value": immediate(json!([1]))}, "subgraph": body})
            }
            _ => json!({"id": "outer", "stepType": "WaitForSignal", "onWait": body}),
        };
        let root = graph(
            json!({"entryPoint": "outer", "steps": {"outer": step, "done": finish("done")}, "executionPlan": [{"fromStep": "outer", "toStep": "done"}]}),
        );
        assert!(
            validate_workflow(
                &root,
                &AgentCatalog::from_json(include_str!("catalog/agent_catalog.json")).unwrap()
            )
            .errors
            .iter()
            .any(|error| error.code() == "E129"),
            "{wrapper}"
        );
        assert_retry_error(compile_retry_input(root, vec![]).unwrap_err());
    }
}

#[test]
fn audit_06_child_retry_overflow_is_rejected() {
    use runtara_workflows::validation::{ClosureChildGraph, validate_workflow_closure};
    let root = retry_graph(None, "embed");
    let child = retry_graph(Some(u32::MAX), "split");
    let validation = validate_workflow_closure(
        "audit-retries",
        &root,
        &AgentCatalog::from_json(include_str!("catalog/agent_catalog.json")).unwrap(),
        &[ClosureChildGraph {
            workflow_id: "child".into(),
            version: 1,
            execution_graph: child.clone(),
        }],
    );
    assert!(
        validation
            .errors()
            .any(|(origin, error)| origin == Some(("child", 1)) && error.code() == "E129")
    );
    let mut children = retry_children("embed");
    children[0].execution_graph = child;
    assert_retry_error(compile_retry_input(root, children).unwrap_err());
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
