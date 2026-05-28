//! Direct GroupBy parity fixtures.
//!
//! These tests compare the shared direct JSON stdlib GroupBy helper with the
//! current generated-code GroupBy semantics before the direct core starts
//! lowering GroupBy steps.

use std::collections::BTreeMap;

use runtara_workflow_stdlib::direct_json::{DirectJsonManifest, build_source};
use runtara_workflows::ExecutionGraph;
use runtara_workflows::codegen::ast::mapping::path_to_json_pointer;
use runtara_workflows::direct_wasm::{DirectGraphManifest, build_direct_workflow_manifest};
use serde_json::{Value, json};

const GROUP_BY_SIMPLE: &str = include_str!("fixtures/group_by_simple.json");
const GROUP_BY_NESTED_KEY: &str = include_str!("fixtures/group_by_nested_key.json");
const GROUP_BY_EXPECTED_KEYS: &str = include_str!("fixtures/group_by_expected_keys.json");

#[test]
fn direct_group_by_matches_current_semantics() {
    let cases = [
        (
            "simple status groups",
            GROUP_BY_SIMPLE,
            json!({
                "items": [
                    { "id": 1, "status": "active" },
                    { "id": 2, "status": "inactive" },
                    { "id": 3, "status": "active" }
                ]
            }),
            json!({
                "counts": { "active": 2, "inactive": 1 },
                "total_groups": 2
            }),
        ),
        (
            "nested role groups",
            GROUP_BY_NESTED_KEY,
            json!({
                "users": [
                    { "id": 1, "profile": { "role": "admin" } },
                    { "id": 2, "profile": { "role": "viewer" } },
                    { "id": 3, "profile": { "role": "admin" } }
                ]
            }),
            json!({
                "counts": { "admin": 2, "viewer": 1 },
                "total_groups": 2
            }),
        ),
        (
            "expected keys",
            GROUP_BY_EXPECTED_KEYS,
            json!({
                "items": [
                    { "id": 1, "action": "created" },
                    { "id": 2, "action": "updated" },
                    { "id": 3, "action": "created" }
                ]
            }),
            json!({
                "counts": {
                    "created": 2,
                    "failed": 0,
                    "linked": 0,
                    "unchanged": 0,
                    "updated": 1
                },
                "total_groups": 5
            }),
        ),
    ];

    for (name, graph_json, data, expected_subset) in cases {
        let direct_output = direct_group_by_output(graph_json, &data);
        let current_output = current_group_by_output(graph_json, &data);

        assert_eq!(direct_output, current_output, "group output case `{name}`");
        assert_eq!(
            direct_output["counts"], expected_subset["counts"],
            "counts case `{name}`"
        );
        assert_eq!(
            direct_output["total_groups"], expected_subset["total_groups"],
            "total_groups case `{name}`"
        );
    }
}

fn direct_group_by_output(graph_json: &str, data: &Value) -> Value {
    let graph: ExecutionGraph = serde_json::from_str(graph_json).expect("fixture parses");
    let manifest = build_direct_workflow_manifest(&graph).expect("manifest");
    let manifest_json = manifest.to_canonical_json().expect("manifest json");
    let direct_manifest = DirectJsonManifest::parse(&manifest_json).expect("direct manifest");
    let source = source_bytes(data, &manifest.graph.variables);

    let group_id = group_by_id(&manifest.graph, &manifest.graph.entry_point);
    let output = direct_manifest
        .group_by(group_id, &source)
        .expect("group output");
    let steps: Value = serde_json::from_slice(&output).expect("steps json");

    steps[&manifest.graph.entry_point]["outputs"].clone()
}

fn current_group_by_output(graph_json: &str, data: &Value) -> Value {
    let graph_value: Value = serde_json::from_str(graph_json).expect("graph json");
    let graph: ExecutionGraph = serde_json::from_str(graph_json).expect("fixture parses");
    let manifest = build_direct_workflow_manifest(&graph).expect("manifest");
    let source_bytes = source_bytes(data, &manifest.graph.variables);
    let source: Value = serde_json::from_slice(&source_bytes).expect("source json");

    let entry = graph_value["entryPoint"].as_str().expect("entry point");
    let config = &graph_value["steps"][entry]["config"];
    let input = apply_current_mapping_value(&config["value"], &source);
    let items = input.as_array().cloned().unwrap_or_default();
    let key_pointer = path_to_json_pointer(config["key"].as_str().expect("group key"));

    let mut groups = BTreeMap::<String, Vec<Value>>::new();
    let mut counts = BTreeMap::<String, usize>::new();
    if let Some(expected_keys) = config.get("expectedKeys").and_then(Value::as_array) {
        for key in expected_keys.iter().filter_map(Value::as_str) {
            groups.entry(key.to_string()).or_default();
            counts.entry(key.to_string()).or_insert(0);
        }
    }

    for item in items {
        let key = item.pointer(&key_pointer).cloned().unwrap_or(Value::Null);
        let key = group_key_string(&key);
        groups.entry(key.clone()).or_default().push(item);
        *counts.entry(key).or_insert(0) += 1;
    }

    json!({
        "groups": groups,
        "counts": counts,
        "total_groups": groups.len(),
    })
}

fn source_bytes(data: &Value, variables: &Value) -> Vec<u8> {
    build_source(
        data.to_string().as_bytes(),
        variables.to_string().as_bytes(),
        b"{}",
    )
    .expect("source")
}

fn group_by_id(graph: &DirectGraphManifest, step_id: &str) -> u32 {
    graph
        .group_bys
        .iter()
        .find(|group_by| group_by.step_id == step_id && group_by.purpose == "groupBy.config")
        .expect("group-by config")
        .id
}

fn apply_current_mapping_value(value: &Value, source: &Value) -> Value {
    let value_type = value["valueType"].as_str().expect("mapping valueType");
    match value_type {
        "reference" => {
            let path = value["value"].as_str().expect("reference path");
            source
                .pointer(&path_to_json_pointer(path))
                .cloned()
                .unwrap_or_else(|| value.get("default").cloned().unwrap_or(Value::Null))
        }
        "immediate" => value.get("value").cloned().unwrap_or(Value::Null),
        other => panic!("unsupported fixture mapping valueType `{other}`"),
    }
}

fn group_key_string(value: &Value) -> String {
    match value {
        Value::String(value) => value.clone(),
        Value::Number(value) => value.to_string(),
        Value::Bool(value) => value.to_string(),
        Value::Null => "_null".to_string(),
        other => serde_json::to_string(other).unwrap_or_else(|_| "_invalid".to_string()),
    }
}
