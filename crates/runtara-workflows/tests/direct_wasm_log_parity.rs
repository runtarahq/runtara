//! Direct Log parity fixtures.
//!
//! These tests compare the shared direct JSON stdlib Log helpers with the
//! current generated-code Log event and step-output semantics.

use runtara_workflow_stdlib::direct_json::{DirectJsonManifest, build_source};
use runtara_workflows::ExecutionGraph;
use runtara_workflows::codegen::ast::mapping::path_to_json_pointer;
use runtara_workflows::direct_wasm::{DirectGraphManifest, build_direct_workflow_manifest};
use serde_json::{Value, json};

const LOG_ALL_LEVELS: &str = include_str!("fixtures/log_all_levels.json");

#[test]
fn direct_log_matches_current_semantics() {
    let data = json!({ "message": "hello", "count": 2 });

    for step_id in ["log_debug", "log_info", "log_warn", "log_error"] {
        let direct = direct_log_event_and_output(LOG_ALL_LEVELS, step_id, &data);
        let current = current_log_event_and_output(LOG_ALL_LEVELS, step_id, &data);

        assert_eq!(direct, current, "Log step `{step_id}`");
    }
}

fn direct_log_event_and_output(graph_json: &str, step_id: &str, data: &Value) -> (Value, Value) {
    let graph: ExecutionGraph = serde_json::from_str(graph_json).expect("fixture parses");
    let manifest = build_direct_workflow_manifest(&graph).expect("manifest");
    let manifest_json = manifest.to_canonical_json().expect("manifest json");
    let direct_manifest = DirectJsonManifest::parse(&manifest_json).expect("direct manifest");
    let source = source_bytes(data, &manifest.graph.variables);

    let log_id = log_id(&manifest.graph, step_id);
    let payload = direct_manifest
        .log_event(log_id, &source)
        .expect("log-event payload");
    let mut payload: Value = serde_json::from_slice(&payload).expect("payload json");
    payload
        .as_object_mut()
        .expect("payload object")
        .remove("timestamp_ms");

    let steps = direct_manifest.log(log_id, &source).expect("log output");
    let steps: Value = serde_json::from_slice(&steps).expect("steps json");

    (payload, steps[step_id]["outputs"].clone())
}

fn current_log_event_and_output(graph_json: &str, step_id: &str, data: &Value) -> (Value, Value) {
    let graph_value: Value = serde_json::from_str(graph_json).expect("graph json");
    let graph: ExecutionGraph = serde_json::from_str(graph_json).expect("fixture parses");
    let manifest = build_direct_workflow_manifest(&graph).expect("manifest");
    let source_bytes = source_bytes(data, &manifest.graph.variables);
    let source: Value = serde_json::from_slice(&source_bytes).expect("source json");
    let step = &graph_value["steps"][step_id];
    let level = step["level"].as_str().unwrap_or("info");
    let message = step["message"].as_str().expect("Log message");
    let step_name = step
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("Unnamed");
    let context = step
        .get("context")
        .and_then(Value::as_object)
        .filter(|context| !context.is_empty())
        .map(|context| apply_current_mapping(&Value::Object(context.clone()), &source))
        .unwrap_or_else(|| json!({}));

    (
        json!({
            "step_id": step_id,
            "step_name": step_name,
            "level": level,
            "message": message,
            "context": context,
        }),
        json!({
            "level": level,
            "message": message,
        }),
    )
}

fn source_bytes(data: &Value, variables: &Value) -> Vec<u8> {
    build_source(
        data.to_string().as_bytes(),
        variables.to_string().as_bytes(),
        b"{}",
    )
    .expect("source")
}

fn log_id(graph: &DirectGraphManifest, step_id: &str) -> u32 {
    graph
        .logs
        .iter()
        .find(|log| log.step_id == step_id && log.purpose == "log.config")
        .expect("log config")
        .id
}

fn apply_current_mapping(mapping: &Value, source: &Value) -> Value {
    let mut output = serde_json::Map::new();
    for (key, value) in mapping.as_object().expect("mapping object") {
        output.insert(key.clone(), eval_current_mapping_value(value, source));
    }
    Value::Object(output)
}

fn eval_current_mapping_value(value: &Value, source: &Value) -> Value {
    let value_type = value["valueType"].as_str().expect("mapping valueType");
    match value_type {
        "reference" => {
            let path = value["value"].as_str().expect("reference path");
            let pointer = path_to_json_pointer(path);
            match source.pointer(&pointer).cloned() {
                Some(Value::Null) | None => value.get("default").cloned().unwrap_or(Value::Null),
                Some(value) => value,
            }
        }
        "immediate" => value.get("value").cloned().unwrap_or(Value::Null),
        other => panic!("unsupported fixture mapping valueType `{other}`"),
    }
}
