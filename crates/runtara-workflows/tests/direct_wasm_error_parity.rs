//! Direct Error parity fixtures.
//!
//! These tests compare the shared direct JSON stdlib Error helpers with the
//! current generated-code Error event and workflow-failure semantics.

use runtara_workflow_stdlib::direct_json::{DirectJsonManifest, build_source};
use runtara_workflows::ExecutionGraph;
use runtara_workflows::codegen::ast::mapping::path_to_json_pointer;
use runtara_workflows::direct_wasm::{DirectGraphManifest, build_direct_workflow_manifest};
use serde_json::{Value, json};

const ERROR_DIRECT_SIMPLE: &str = include_str!("fixtures/error_direct_simple.json");

#[test]
fn direct_error_matches_current_semantics() {
    let cases = [
        (
            "explicit metadata with context",
            ERROR_DIRECT_SIMPLE.to_string(),
            "fail",
            json!({ "requestId": "req-123" }),
        ),
        (
            "default category severity and context",
            default_error_graph_json(),
            "fail",
            json!({ "requestId": "req-456" }),
        ),
    ];

    for (name, graph_json, step_id, data) in cases {
        let direct = direct_error_event_and_failure(&graph_json, step_id, &data);
        let current = current_error_event_and_failure(&graph_json, step_id, &data);

        assert_eq!(direct, current, "case `{name}`");
    }
}

fn direct_error_event_and_failure(graph_json: &str, step_id: &str, data: &Value) -> (Value, Value) {
    let graph: ExecutionGraph = serde_json::from_str(graph_json).expect("fixture parses");
    let manifest = build_direct_workflow_manifest(&graph).expect("manifest");
    let manifest_json = manifest.to_canonical_json().expect("manifest json");
    let direct_manifest = DirectJsonManifest::parse(&manifest_json).expect("direct manifest");
    let source = source_bytes(data, &manifest.graph.variables);

    let error_id = error_id(&manifest.graph, step_id);
    let payload = direct_manifest
        .error_event(error_id, &source)
        .expect("error-event payload");
    let mut payload: Value = serde_json::from_slice(&payload).expect("payload json");
    payload
        .as_object_mut()
        .expect("payload object")
        .remove("timestamp_ms");

    let failure = direct_manifest
        .error(error_id, &source)
        .expect("error failure payload");
    let failure: Value = serde_json::from_slice(&failure).expect("failure json");

    (payload, failure)
}

fn current_error_event_and_failure(
    graph_json: &str,
    step_id: &str,
    data: &Value,
) -> (Value, Value) {
    let graph_value: Value = serde_json::from_str(graph_json).expect("graph json");
    let graph: ExecutionGraph = serde_json::from_str(graph_json).expect("fixture parses");
    let manifest = build_direct_workflow_manifest(&graph).expect("manifest");
    let source_bytes = source_bytes(data, &manifest.graph.variables);
    let source: Value = serde_json::from_slice(&source_bytes).expect("source json");
    let step = &graph_value["steps"][step_id];
    let category = step
        .get("category")
        .and_then(Value::as_str)
        .unwrap_or("permanent");
    let severity = step
        .get("severity")
        .and_then(Value::as_str)
        .unwrap_or("error");
    let code = step["code"].as_str().expect("Error code");
    let message = step["message"].as_str().expect("Error message");
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
            "category": category,
            "code": code,
            "message": message,
            "severity": severity,
            "context": context,
        }),
        json!({
            "stepId": step_id,
            "stepName": step_name,
            "category": category,
            "code": code,
            "message": message,
            "severity": severity,
            "context": context,
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

fn error_id(graph: &DirectGraphManifest, step_id: &str) -> u32 {
    graph
        .errors
        .iter()
        .find(|error| error.step_id == step_id && error.purpose == "error.config")
        .expect("error config")
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

fn default_error_graph_json() -> String {
    json!({
        "name": "Default Error",
        "steps": {
            "fail": {
                "stepType": "Error",
                "id": "fail",
                "code": "DEFAULT_FAILURE",
                "message": "Default failure"
            }
        },
        "entryPoint": "fail",
        "executionPlan": [],
        "variables": {},
        "inputSchema": {},
        "outputSchema": {}
    })
    .to_string()
}
