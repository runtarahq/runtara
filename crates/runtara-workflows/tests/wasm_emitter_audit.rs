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

fn compile_audit_graph(
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
        compile_audit_graph(root, retry_children(kind))
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
    let result = std::panic::catch_unwind(|| compile_audit_graph(root, retry_children(kind)))
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
        assert_retry_error(compile_audit_graph(root, vec![]).unwrap_err());
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
    assert_retry_error(compile_audit_graph(root, children).unwrap_err());
}

fn assert_manifest_identity_error(
    error: runtara_workflows::direct_wasm::DirectManifestError,
    expected: (&str, &str, &str),
) {
    use runtara_workflows::direct_wasm::DirectManifestError;
    let DirectManifestError::StepIdMismatch {
        graph_path,
        step_key,
        step_id,
    } = error
    else {
        panic!("expected a structured identity error, got {error:?}");
    };
    assert_eq!(
        (graph_path.as_str(), step_key.as_str(), step_id.as_str()),
        expected
    );
}

fn assert_bad_identity_rejected(root: Value, expected: &[(&str, &str, &str)]) {
    use runtara_workflows::{direct_wasm::DirectCompileError, validation::ValidationError};
    let root = graph(root);
    let validation = validate_workflow(&root, &AgentCatalog::default());
    let errors = validation
        .errors
        .iter()
        .filter_map(|error| match error {
            ValidationError::StepIdMismatch {
                graph_path,
                step_key,
                step_id,
            } => {
                assert_eq!(error.code(), "E130");
                assert!(error.to_string().contains("set step.id to the map key"));
                Some((graph_path.as_str(), step_key.as_str(), step_id.as_str()))
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        errors, expected,
        "each malformed declaration needs its own diagnostic"
    );
    let support = analyze_direct_wasm_support(&root);
    assert!(!support.supported);
    assert_eq!(
        support.unsupported.len(),
        expected.len(),
        "identity errors must not cause a routing cascade"
    );
    for (issue, &(path, key, id)) in support.unsupported.iter().zip(expected) {
        assert_eq!(issue.feature, "step-id-mismatch");
        assert_eq!(issue.step_id.as_deref(), Some(key));
        assert!(
            issue
                .reason
                .contains(if path.is_empty() { "<root>" } else { path })
        );
        assert!(issue.reason.contains(&format!("{key:?}")));
        assert!(issue.reason.contains(&format!("{id:?}")));
    }
    assert_manifest_identity_error(
        build_direct_workflow_manifest(&root).unwrap_err(),
        expected[0],
    );
    let DirectCompileError::Manifest(error) = compile_audit_graph(root, vec![]).unwrap_err() else {
        panic!("compilation must reject malformed IDs during manifest preflight");
    };
    assert_manifest_identity_error(error, expected[0]);
}

#[test]
fn audit_07_matching_step_keys_validate_and_compile() {
    // Equality is exact; punctuation and Unicode are not normalized or renamed.
    for id in ["finish", "a/b~c", "user.name", "读入", "é", "e\u{301}"] {
        let root = graph(leaf(id));
        assert!(
            validate_workflow(&root, &AgentCatalog::default())
                .errors
                .is_empty()
        );
        assert!(analyze_direct_wasm_support(&root).supported);
        compile_audit_graph(root, vec![]).expect("consistent graph compiles");
    }
}

#[test]
fn audit_07_root_key_id_mismatch_is_rejected() {
    assert_bad_identity_rejected(
        json!({"entryPoint": "finish", "steps": {"finish": finish("different")}}),
        &[("", "finish", "different")],
    );
}

#[test]
fn audit_07_nested_key_id_mismatch_is_rejected() {
    let body = json!({"entryPoint": "finish", "steps": {"finish": finish("different")}});
    assert_bad_identity_rejected(
        json!({"entryPoint": "loop", "steps": {
        "loop": loop_step("loop", body), "finish": finish("finish")
    }, "executionPlan": [{"fromStep": "loop", "toStep": "finish"}]}),
        &[("/steps/loop/subgraph", "finish", "different")],
    );
}

#[test]
fn audit_07_duplicate_inner_ids_are_rejected() {
    assert_bad_identity_rejected(
        json!({"entryPoint": "log", "steps": {
        "log": {"id": "finish", "stepType": "Log", "message": "audit"}, "finish": finish("finish")
    }, "executionPlan": [{"fromStep": "log", "toStep": "finish"}]}),
        &[("", "log", "finish")],
    );
}

#[test]
fn audit_07_split_and_on_wait_key_id_mismatches_are_rejected() {
    for role in ["subgraph", "onWait"] {
        let body = json!({"entryPoint":"finish", "steps":{"finish":finish("wrong")}});
        let step = if role == "subgraph" {
            json!({"id":"outer", "stepType":"Split", "config":{"value":immediate(json!([1]))}, "subgraph":body})
        } else {
            json!({"id":"outer", "stepType":"WaitForSignal", "onWait":body})
        };
        assert_bad_identity_rejected(
            json!({"entryPoint":"outer", "steps":{"outer":step}}),
            &[(&format!("/steps/outer/{role}"), "finish", "wrong")],
        );
    }
}

#[test]
fn audit_07_nested_paths_escape_json_pointer_segments() {
    let body = json!({"entryPoint":"w~/", "steps":{"w~/":{"id":"w~/", "stepType":"WaitForSignal", "onWait":{
        "entryPoint":"finish", "steps":{"finish":finish("wrong")}
    }}}});
    assert_bad_identity_rejected(
        json!({"entryPoint":"a/~", "steps":{"a/~":loop_step("a/~", body)}}),
        &[(
            "/steps/a~1~0/subgraph/steps/w~0~1/onWait",
            "finish",
            "wrong",
        )],
    );
}

#[test]
fn audit_07_unreachable_and_invalid_entry_graphs_still_report_id_mismatches() {
    for entry in ["finish", "missing"] {
        assert_bad_identity_rejected(
            json!({"entryPoint":entry, "steps":{
                "finish":finish("finish"), "orphan":{"id":"wrong", "stepType":"Log", "message":"unreachable"}
            }}),
            &[("", "orphan", "wrong")],
        );
    }
    assert_bad_identity_rejected(
        json!({"entryPoint":"missing", "steps":{"outer":loop_step("outer", json!({"entryPoint":"finish", "steps":{"finish":finish("wrong")}}))}}),
        &[("/steps/outer/subgraph", "finish", "wrong")],
    );
}

#[test]
fn audit_07_all_step_variants_check_their_declared_id() {
    let from_fixture = |source: &str| {
        let value: Value = serde_json::from_str(source).unwrap();
        value["steps"][value["entryPoint"].as_str().unwrap()].clone()
    };
    let mut cases = vec![
        finish("declared"),
        serde_json::to_value(retry_graph(None, "agent")).unwrap()["steps"]["call"].clone(),
        serde_json::to_value(retry_graph(None, "embed")).unwrap()["steps"]["call"].clone(),
        serde_json::to_value(retry_graph(None, "split")).unwrap()["steps"]["call"].clone(),
        serde_json::to_value(retry_graph(None, "ai")).unwrap()["steps"]["call"].clone(),
        loop_step("declared", leaf("done")),
        json!({"id":"declared", "stepType":"Log", "message":"audit"}),
        json!({"id":"declared", "stepType":"Error", "message":"audit", "code":"AUDIT", "category":"permanent"}),
        json!({"id":"declared", "stepType":"WaitForSignal"}),
        json!({"id":"declared", "stepType":"Delay", "durationMs":immediate(json!(1))}),
        from_fixture(include_str!("fixtures/conditional_workflow.json")),
        from_fixture(include_str!("fixtures/filter_simple.json")),
        from_fixture(include_str!("fixtures/switch_value_simple.json")),
        from_fixture(include_str!("fixtures/group_by_simple.json")),
    ];
    let types = cases
        .iter()
        .map(|step| step["stepType"].as_str().unwrap())
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(
        types.len(),
        14,
        "one fixture for every current Step variant"
    );
    for step in &mut cases {
        step["id"] = json!("declared");
        assert_bad_identity_rejected(
            json!({"entryPoint":"key", "steps":{"key":step}}),
            &[("", "key", "declared")],
        );
    }
}

#[test]
fn audit_07_ai_tool_declarations_are_checked() {
    let mut root = serde_json::to_value(retry_graph(None, "ai-tools")).unwrap();
    root["steps"]["echo"]["id"] = json!("wrong");
    assert_bad_identity_rejected(root, &[("", "echo", "wrong")]);
}

#[test]
fn audit_07_multiple_identity_errors_have_stable_order() {
    for _ in 0..8 {
        assert_bad_identity_rejected(
            json!({"entryPoint":"a", "steps":{
                "z":finish("same"), "a":finish("same"), "outer":loop_step("outer", json!({"entryPoint":"leaf", "steps":{"leaf":finish("wrong")}}))
            }}),
            &[
                ("", "a", "same"),
                ("", "z", "same"),
                ("/steps/outer/subgraph", "leaf", "wrong"),
            ],
        );
    }
}

#[test]
fn audit_07_visually_similar_ids_are_not_silently_normalized() {
    for (key, id) in [("step", " step"), ("é", "e\u{301}"), ("Step", "step")] {
        assert_bad_identity_rejected(
            json!({"entryPoint":key, "steps":{key:finish(id)}}),
            &[("", key, id)],
        );
    }
}

#[test]
fn audit_07_local_ids_can_repeat_in_separate_nested_graphs() {
    let a = loop_step("a", leaf("finish"));
    let b = json!({"id":"b", "stepType":"Split", "config":{"value":immediate(json!([1]))}, "subgraph":leaf("finish")});
    let root = graph(
        json!({"entryPoint":"a", "steps":{"a":a,"b":b,"finish":finish("finish")}, "executionPlan":[{"fromStep":"a","toStep":"b"},{"fromStep":"b","toStep":"finish"}]}),
    );
    assert!(
        validate_workflow(&root, &AgentCatalog::default())
            .errors
            .is_empty()
    );
    assert!(analyze_direct_wasm_support(&root).supported);
    compile_audit_graph(root, vec![]).unwrap();
}

#[test]
fn audit_07_preloaded_child_identity_is_validated_before_manifest_construction() {
    use runtara_workflows::{
        direct_wasm::{DirectCompileError, analyze_direct_wasm_support_with_child_workflows},
        validation::{ClosureChildGraph, ValidationError, validate_workflow_closure},
    };
    for referenced in [false, true] {
        for nested in [false, true] {
            let root = if referenced {
                retry_graph(None, "embed")
            } else {
                graph(leaf("finish"))
            };
            let bad = json!({"entryPoint":"finish", "steps":{"finish":finish("wrong")}});
            let child = graph(if nested {
                json!({"entryPoint":"outer", "steps":{"outer":loop_step("outer", bad)}})
            } else {
                bad
            });
            let relative_path = if nested { "/steps/outer/subgraph" } else { "" };
            let manifest_path = format!("/childWorkflows/0/executionGraph{relative_path}");
            let mut children = retry_children("embed");
            children[0].execution_graph = child.clone();
            let validation = validate_workflow_closure(
                "audit-identity",
                &root,
                &AgentCatalog::default(),
                &[ClosureChildGraph {
                    workflow_id: "child".into(),
                    version: 1,
                    execution_graph: child.clone(),
                }],
            );
            assert!(validation.errors().any(|(origin,error)| origin == Some(("child",1)) && matches!(error, ValidationError::StepIdMismatch { graph_path, step_key, step_id } if graph_path == relative_path && step_key == "finish" && step_id == "wrong")));
            let support = analyze_direct_wasm_support_with_child_workflows(&root, &children);
            assert!(!support.supported);
            assert_eq!(support.unsupported.len(), 1);
            assert_eq!(support.unsupported[0].feature, "step-id-mismatch");
            assert!(support.unsupported[0].reason.contains(&manifest_path));
            let inputs = [DirectManifestChildWorkflowInput {
                step_id: "call",
                workflow_id: "child",
                version_requested: "latest",
                version_resolved: 1,
                execution_graph: &child,
            }];
            assert_manifest_identity_error(
                build_direct_workflow_manifest_with_child_workflows_and_agent_catalog(
                    &root, &inputs, None,
                )
                .unwrap_err(),
                (manifest_path.as_str(), "finish", "wrong"),
            );
            let DirectCompileError::Manifest(error) =
                compile_audit_graph(root, children).unwrap_err()
            else {
                panic!("manifest identity rejection required")
            };
            assert_manifest_identity_error(error, (manifest_path.as_str(), "finish", "wrong"));
        }
    }
}

#[test]
fn audit_07_local_ids_can_repeat_in_separate_children() {
    use runtara_workflows::validation::{ClosureChildGraph, validate_workflow_closure};
    let call = |id| json!({"id":id,"stepType":"EmbedWorkflow","childWorkflowId":id,"childVersion":"latest"});
    let root = graph(
        json!({"entryPoint":"a","steps":{"a":call("a"),"b":call("b"),"finish":finish("finish")},"executionPlan":[{"fromStep":"a","toStep":"b"},{"fromStep":"b","toStep":"finish"}]}),
    );
    let children = ["a", "b"]
        .into_iter()
        .map(|id| ChildWorkflowInput {
            step_id: id.into(),
            workflow_id: id.into(),
            version_requested: "latest".into(),
            version_resolved: 1,
            execution_graph: graph(leaf("finish")),
        })
        .collect::<Vec<_>>();
    let closure = children
        .iter()
        .map(|child| ClosureChildGraph {
            workflow_id: child.workflow_id.clone(),
            version: 1,
            execution_graph: child.execution_graph.clone(),
        })
        .collect::<Vec<_>>();
    assert!(
        validate_workflow_closure("audit-identity", &root, &AgentCatalog::default(), &closure)
            .errors()
            .next()
            .is_none()
    );
    compile_audit_graph(root, children).unwrap();
}
