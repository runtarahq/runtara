// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
#![cfg(feature = "compiler")]

use runtara_dsl::{ExecutionGraph, agent_meta::AgentCatalog};
use runtara_workflow_stdlib::direct_json::DirectJsonManifest;
use runtara_workflows::{
    direct_wasm::build_direct_workflow_manifest, validation::validate_workflow,
};
use serde_json::{Value, json};

fn graph(label: Value) -> ExecutionGraph {
    serde_json::from_value(json!({
        "entryPoint":"finish", "durable":false,
        "steps":{"finish":{"id":"finish", "stepType":"Finish", "runLabel":label,
            "inputMapping":{"result":{"valueType":"immediate", "value":42}}}}
    }))
    .unwrap()
}

#[test]
fn run_label_mapping_is_separate_validated_and_optional() {
    for (label, source, expected) in [
        (
            json!({"valueType":"immediate","value":" Order/12 [done] "}),
            json!({}),
            json!("Order/12 [done]"),
        ),
        (
            json!({"valueType":"reference","value":"data.label"}),
            json!({"data":{"label":"Job-1.2 (done)"}}),
            json!("Job-1.2 (done)"),
        ),
        (
            json!({"valueType":"template","value":"Order/{{ data.id }}"}),
            json!({"data":{"id":123}}),
            json!("Order/123"),
        ),
        (
            json!({"valueType":"reference","value":"data.label"}),
            json!({"data":{}}),
            Value::Null,
        ),
        (
            json!({"valueType":"immediate","value":""}),
            json!({}),
            Value::Null,
        ),
        (
            json!({"valueType":"immediate","value":"x".repeat(251)}),
            json!({}),
            json!("x".repeat(250)),
        ),
        (
            json!({"valueType":"reference","value":"data.label"}),
            json!({"data":{"label":"x".repeat(500)}}),
            json!("x".repeat(250)),
        ),
        (
            json!({"valueType":"template","value":"{{"}),
            json!({}),
            Value::Null,
        ),
    ] {
        let manifest = build_direct_workflow_manifest(&graph(label)).unwrap();
        let runtime = DirectJsonManifest::parse(&manifest.to_canonical_json().unwrap()).unwrap();
        let mapping = manifest
            .graph
            .mappings
            .iter()
            .find(|m| m.purpose == "finish.runLabel")
            .unwrap();
        let resolved = runtime
            .apply_mapping(mapping.id, &serde_json::to_vec(&source).unwrap())
            .unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&resolved).unwrap(),
            expected
        );
        let outputs = manifest
            .graph
            .mappings
            .iter()
            .find(|m| m.purpose == "finish.inputMapping")
            .unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&runtime.apply_mapping(outputs.id, b"{}").unwrap())
                .unwrap(),
            json!({"result":42})
        );
        assert!(manifest.feature_summary.needs_runtime(false));
    }
    let manifest = build_direct_workflow_manifest(&graph(
        json!({"valueType":"reference","value":"data.label"}),
    ))
    .unwrap();
    let runtime = DirectJsonManifest::parse(&manifest.to_canonical_json().unwrap()).unwrap();
    let id = manifest
        .graph
        .mappings
        .iter()
        .find(|m| m.purpose == "finish.runLabel")
        .unwrap()
        .id;
    for value in [
        json!(123),
        json!(true),
        json!({}),
        json!([]),
        json!("bad_label"),
        json!("   "),
        json!("--- ./()[]"),
        json!("\u{200b}"),
    ] {
        let output = runtime
            .apply_mapping(
                id,
                &serde_json::to_vec(&json!({"data":{"label":value}})).unwrap(),
            )
            .unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&output).unwrap(),
            Value::Null
        );
    }
}

#[test]
fn run_label_static_validation_covers_value_scope_and_references() {
    for value in [
        json!(12),
        json!("bad_label"),
        json!("   "),
        json!("--- ./()[]"),
        json!("\u{200b}"),
    ] {
        let g = graph(json!({"valueType":"immediate","value":value}));
        assert!(
            validate_workflow(&g, &AgentCatalog::default())
                .errors
                .iter()
                .any(|e| e.code() == "E131")
        );
        assert!(build_direct_workflow_manifest(&g).is_err());
    }
    let g = graph(json!({"valueType":"reference","value":"steps.missing.outputs.label"}));
    assert!(
        validate_workflow(&g, &AgentCatalog::default())
            .errors
            .iter()
            .any(|e| e.code() == "E010")
    );
    let child = graph(json!({"valueType":"immediate","value":"child"}));
    let g: ExecutionGraph = serde_json::from_value(json!({
        "entryPoint":"loop", "steps":{
            "loop":{"id":"loop","stepType":"While","condition":{"type":"operation","op":"EQ","arguments":[{"valueType":"immediate","value":true},{"valueType":"immediate","value":true}]},"config":{"maxIterations":1},"subgraph":child},
            "finish":{"id":"finish","stepType":"Finish"}
        },"executionPlan":[{"fromStep":"loop","toStep":"finish"}]
    })).unwrap();
    assert!(
        validate_workflow(&g, &AgentCatalog::default())
            .errors
            .iter()
            .any(|e| e.code() == "E131")
    );
    assert!(build_direct_workflow_manifest(&g).is_err());
}
