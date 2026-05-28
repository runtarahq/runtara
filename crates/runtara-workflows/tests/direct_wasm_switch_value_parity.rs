//! Direct Switch parity fixtures.
//!
//! These tests compare the shared direct JSON stdlib Switch helpers with the
//! current generated-code Switch semantics for value and routing switches.

use runtara_workflow_stdlib::conditions::{is_truthy, to_number, values_equal};
use runtara_workflow_stdlib::direct_json::{DirectJsonManifest, build_source};
use runtara_workflow_stdlib::switch_helpers::process_switch_output;
use runtara_workflows::ExecutionGraph;
use runtara_workflows::codegen::ast::mapping::path_to_json_pointer;
use runtara_workflows::direct_wasm::{DirectGraphManifest, build_direct_workflow_manifest};
use serde_json::{Value, json};

const SWITCH_VALUE_SIMPLE: &str = include_str!("fixtures/switch_value_simple.json");
const SWITCH_VALUE_RANGE: &str = include_str!("fixtures/switch_value_range.json");
const SWITCH_ROUTING_SIMPLE: &str = include_str!("fixtures/switch_routing_simple.json");

#[test]
fn direct_value_switch_matches_current_semantics() {
    let cases = [
        (
            "first matching case",
            SWITCH_VALUE_SIMPLE,
            json!({ "status": "active" }),
            json!({ "bucket": "ready", "echo": "active" }),
        ),
        (
            "array equality shorthand",
            SWITCH_VALUE_SIMPLE,
            json!({ "status": "queued" }),
            json!({ "bucket": "pending" }),
        ),
        (
            "default output",
            SWITCH_VALUE_SIMPLE,
            json!({ "status": "done" }),
            json!({ "bucket": "other" }),
        ),
        (
            "between case",
            SWITCH_VALUE_RANGE,
            json!({ "score": 90 }),
            json!({ "grade": "high" }),
        ),
        (
            "range case",
            SWITCH_VALUE_RANGE,
            json!({ "score": 65 }),
            json!({ "grade": "mid" }),
        ),
        (
            "range default",
            SWITCH_VALUE_RANGE,
            json!({ "score": 20 }),
            json!({ "grade": "low" }),
        ),
    ];

    for (name, graph_json, data, expected_output) in cases {
        let direct_output = direct_value_switch_output(graph_json, &data);
        let current_output = current_value_switch_output(graph_json, &data);

        assert_eq!(direct_output, current_output, "switch output case `{name}`");
        assert_eq!(
            direct_output, expected_output,
            "fixture expectation `{name}`"
        );
    }
}

#[test]
fn direct_routing_switch_matches_current_semantics() {
    let cases = [
        (
            "first matching route",
            json!({ "status": "active" }),
            "active",
            json!({ "bucket": "ready", "echo": "active" }),
        ),
        (
            "array equality route",
            json!({ "status": "queued" }),
            "pending",
            json!({ "bucket": "pending" }),
        ),
        (
            "default route",
            json!({ "status": "done" }),
            "default",
            json!({ "bucket": "other" }),
        ),
    ];

    for (name, data, expected_route, expected_output) in cases {
        let direct_result = direct_routing_switch_route_and_output(SWITCH_ROUTING_SIMPLE, &data);
        let current_result = current_routing_switch_route_and_output(SWITCH_ROUTING_SIMPLE, &data);

        assert_eq!(
            direct_result, current_result,
            "routing switch case `{name}`"
        );
        assert_eq!(
            direct_result,
            (expected_route.to_string(), expected_output),
            "fixture expectation `{name}`"
        );
    }
}

fn direct_value_switch_output(graph_json: &str, data: &Value) -> Value {
    let graph: ExecutionGraph = serde_json::from_str(graph_json).expect("fixture parses");
    let manifest = build_direct_workflow_manifest(&graph).expect("manifest");
    let manifest_json = manifest.to_canonical_json().expect("manifest json");
    let direct_manifest = DirectJsonManifest::parse(&manifest_json).expect("direct manifest");
    let source = source_bytes(data, &manifest.graph.variables);

    let switch_id = switch_id(&manifest.graph, &manifest.graph.entry_point);
    let output = direct_manifest
        .value_switch(switch_id, &source)
        .expect("value-switch output");
    let steps: Value = serde_json::from_slice(&output).expect("steps json");

    steps[&manifest.graph.entry_point]["outputs"].clone()
}

fn direct_routing_switch_route_and_output(graph_json: &str, data: &Value) -> (String, Value) {
    let graph: ExecutionGraph = serde_json::from_str(graph_json).expect("fixture parses");
    let manifest = build_direct_workflow_manifest(&graph).expect("manifest");
    let manifest_json = manifest.to_canonical_json().expect("manifest json");
    let direct_manifest = DirectJsonManifest::parse(&manifest_json).expect("direct manifest");
    let source = source_bytes(data, &manifest.graph.variables);

    let switch_id = switch_id(&manifest.graph, &manifest.graph.entry_point);
    let route = direct_manifest
        .process_switch(switch_id, &source)
        .expect("process-switch route");
    let output = direct_manifest
        .value_switch(switch_id, &source)
        .expect("value-switch output");
    let steps: Value = serde_json::from_slice(&output).expect("steps json");

    assert_eq!(
        steps[&manifest.graph.entry_point]["route"],
        json!(route.clone())
    );
    (route, steps[&manifest.graph.entry_point]["outputs"].clone())
}

fn current_value_switch_output(graph_json: &str, data: &Value) -> Value {
    let graph_value: Value = serde_json::from_str(graph_json).expect("graph json");
    let graph: ExecutionGraph = serde_json::from_str(graph_json).expect("fixture parses");
    let manifest = build_direct_workflow_manifest(&graph).expect("manifest");
    let source_bytes = source_bytes(data, &manifest.graph.variables);
    let source: Value = serde_json::from_slice(&source_bytes).expect("source json");

    let entry = graph_value["entryPoint"].as_str().expect("entry point");
    let config = &graph_value["steps"][entry]["config"];
    apply_current_value_switch(config, &source)
}

fn current_routing_switch_route_and_output(graph_json: &str, data: &Value) -> (String, Value) {
    let graph_value: Value = serde_json::from_str(graph_json).expect("graph json");
    let graph: ExecutionGraph = serde_json::from_str(graph_json).expect("fixture parses");
    let manifest = build_direct_workflow_manifest(&graph).expect("manifest");
    let source_bytes = source_bytes(data, &manifest.graph.variables);
    let source: Value = serde_json::from_slice(&source_bytes).expect("source json");

    let entry = graph_value["entryPoint"].as_str().expect("entry point");
    let config = &graph_value["steps"][entry]["config"];
    apply_current_routing_switch(config, &source)
}

fn apply_current_value_switch(config: &Value, source: &Value) -> Value {
    let Some(switch_value) = config.get("value") else {
        let default = config
            .get("default")
            .cloned()
            .unwrap_or_else(|| Value::Object(serde_json::Map::new()));
        return process_switch_output(&default, source);
    };

    if let Some(cases) = config.get("cases").and_then(Value::as_array) {
        for case in cases {
            let condition = switch_case_condition(switch_value, case);
            if eval_current_condition(&condition, source) {
                return process_switch_output(&case["output"], source);
            }
        }
    }

    let default = config
        .get("default")
        .cloned()
        .unwrap_or_else(|| Value::Object(serde_json::Map::new()));
    process_switch_output(&default, source)
}

fn apply_current_routing_switch(config: &Value, source: &Value) -> (String, Value) {
    let Some(switch_value) = config.get("value") else {
        let default = config
            .get("default")
            .cloned()
            .unwrap_or_else(|| Value::Object(serde_json::Map::new()));
        return (
            "default".to_string(),
            process_switch_output(&default, source),
        );
    };

    if let Some(cases) = config.get("cases").and_then(Value::as_array) {
        for case in cases {
            let condition = switch_case_condition(switch_value, case);
            if eval_current_condition(&condition, source) {
                let route = case
                    .get("route")
                    .and_then(Value::as_str)
                    .unwrap_or("default")
                    .to_string();
                return (route, process_switch_output(&case["output"], source));
            }
        }
    }

    let default = config
        .get("default")
        .cloned()
        .unwrap_or_else(|| Value::Object(serde_json::Map::new()));
    (
        "default".to_string(),
        process_switch_output(&default, source),
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

fn switch_id(graph: &DirectGraphManifest, step_id: &str) -> u32 {
    graph
        .switches
        .iter()
        .find(|switch| switch.step_id == step_id && switch.purpose == "switch.config")
        .expect("switch config")
        .id
}

fn switch_case_condition(switch_value: &Value, case: &Value) -> Value {
    let match_type = case["matchType"].as_str().expect("matchType");
    let match_value = case.get("match").cloned().unwrap_or(Value::Null);
    let right = json!({
        "valueType": "immediate",
        "value": match_value,
    });

    match match_type {
        "EQ" if case.get("match").is_some_and(Value::is_array) => {
            binary_condition("IN", switch_value.clone(), right)
        }
        "EQ" | "NE" | "GT" | "GTE" | "LT" | "LTE" | "STARTS_WITH" | "ENDS_WITH" | "CONTAINS"
        | "IN" | "NOT_IN" => binary_condition(match_type, switch_value.clone(), right),
        "IS_DEFINED" | "IS_EMPTY" | "IS_NOT_EMPTY" => {
            unary_condition(match_type, switch_value.clone())
        }
        "BETWEEN" => build_between_condition(switch_value, &match_value),
        "RANGE" => build_range_condition(switch_value, &match_value),
        other => panic!("unsupported fixture Switch matchType `{other}`"),
    }
}

fn binary_condition(op: &str, left: Value, right: Value) -> Value {
    json!({
        "type": "operation",
        "op": op,
        "arguments": [left, right],
    })
}

fn unary_condition(op: &str, value: Value) -> Value {
    json!({
        "type": "operation",
        "op": op,
        "arguments": [value],
    })
}

fn value_condition(value: bool) -> Value {
    json!({
        "type": "value",
        "valueType": "immediate",
        "value": value,
    })
}

fn build_between_condition(switch_value: &Value, match_value: &Value) -> Value {
    let Some(bounds) = match_value.as_array().filter(|bounds| bounds.len() >= 2) else {
        return value_condition(false);
    };

    json!({
        "type": "operation",
        "op": "AND",
        "arguments": [
            binary_condition(
                "GTE",
                switch_value.clone(),
                json!({ "valueType": "immediate", "value": bounds[0].clone() }),
            ),
            binary_condition(
                "LTE",
                switch_value.clone(),
                json!({ "valueType": "immediate", "value": bounds[1].clone() }),
            ),
        ],
    })
}

fn build_range_condition(switch_value: &Value, match_value: &Value) -> Value {
    let Some(bounds) = match_value.as_object() else {
        return value_condition(true);
    };

    let mut conditions = Vec::new();
    for (key, op) in [("gte", "GTE"), ("gt", "GT"), ("lte", "LTE"), ("lt", "LT")] {
        if let Some(value) = bounds.get(key) {
            conditions.push(binary_condition(
                op,
                switch_value.clone(),
                json!({ "valueType": "immediate", "value": value.clone() }),
            ));
        }
    }

    match conditions.len() {
        0 => value_condition(true),
        1 => conditions.remove(0),
        _ => json!({
            "type": "operation",
            "op": "AND",
            "arguments": conditions,
        }),
    }
}

fn eval_current_condition(expr: &Value, source: &Value) -> bool {
    if expr.get("op").is_some()
        || expr
            .get("type")
            .and_then(Value::as_str)
            .is_some_and(|kind| kind == "operation")
    {
        eval_current_operation(expr, source)
    } else {
        is_truthy(&eval_current_mapping_value(expr, source))
    }
}

fn eval_current_operation(expr: &Value, source: &Value) -> bool {
    let op = expr["op"].as_str().expect("condition op");
    let args = expr["arguments"].as_array().expect("condition args");

    match op {
        "AND" => args
            .iter()
            .all(|arg| eval_current_argument_bool(arg, source)),
        "OR" => args
            .iter()
            .any(|arg| eval_current_argument_bool(arg, source)),
        "NOT" => args
            .first()
            .map(|arg| !eval_current_argument_bool(arg, source))
            .unwrap_or(true),
        "GT" | "GTE" | "LT" | "LTE" => {
            if args.len() < 2 {
                return false;
            }
            let left = eval_current_argument_value(&args[0], source);
            let right = eval_current_argument_value(&args[1], source);
            let (Some(left), Some(right)) = (to_number(&left), to_number(&right)) else {
                return false;
            };
            match op {
                "GT" => left > right,
                "GTE" => left >= right,
                "LT" => left < right,
                "LTE" => left <= right,
                _ => false,
            }
        }
        "EQ" | "NE" => {
            if args.len() < 2 {
                return false;
            }
            let equal = values_equal(
                &eval_current_argument_value(&args[0], source),
                &eval_current_argument_value(&args[1], source),
            );
            if op == "NE" { !equal } else { equal }
        }
        "STARTS_WITH" | "ENDS_WITH" => {
            if args.len() < 2 {
                return false;
            }
            let left = eval_current_argument_value(&args[0], source);
            let right = eval_current_argument_value(&args[1], source);
            let (Some(left), Some(right)) = (left.as_str(), right.as_str()) else {
                return false;
            };
            if op == "STARTS_WITH" {
                left.starts_with(right)
            } else {
                left.ends_with(right)
            }
        }
        "CONTAINS" | "IN" | "NOT_IN" => {
            if args.len() < 2 {
                return false;
            }
            let left = eval_current_argument_value(&args[0], source);
            let right = eval_current_argument_value(&args[1], source);
            let matched = match op {
                "CONTAINS" => left
                    .as_array()
                    .is_some_and(|items| items.iter().any(|item| values_equal(item, &right))),
                "IN" | "NOT_IN" => right
                    .as_array()
                    .is_some_and(|items| items.iter().any(|item| values_equal(&left, item))),
                _ => false,
            };
            if op == "NOT_IN" { !matched } else { matched }
        }
        "LENGTH" => args
            .first()
            .map(|arg| is_truthy(&eval_current_length_value(arg, source)))
            .unwrap_or(false),
        "IS_DEFINED" => args
            .first()
            .map(|arg| !eval_current_argument_value(arg, source).is_null())
            .unwrap_or(false),
        "IS_EMPTY" => args
            .first()
            .map(|arg| is_empty_value(&eval_current_argument_value(arg, source)))
            .unwrap_or(true),
        "IS_NOT_EMPTY" => args
            .first()
            .map(|arg| !is_empty_value(&eval_current_argument_value(arg, source)))
            .unwrap_or(false),
        _ => panic!("unsupported fixture condition operator `{op}`"),
    }
}

fn eval_current_argument_bool(arg: &Value, source: &Value) -> bool {
    if arg.get("op").is_some() {
        eval_current_condition(arg, source)
    } else {
        is_truthy(&eval_current_mapping_value(arg, source))
    }
}

fn eval_current_argument_value(arg: &Value, source: &Value) -> Value {
    if arg.get("op").and_then(Value::as_str) == Some("LENGTH") {
        let args = arg["arguments"].as_array().expect("length args");
        return args
            .first()
            .map(|arg| eval_current_length_value(arg, source))
            .unwrap_or_else(|| Value::Number(0.into()));
    }
    if arg.get("op").is_some() {
        Value::Bool(eval_current_condition(arg, source))
    } else {
        eval_current_mapping_value(arg, source)
    }
}

fn eval_current_length_value(arg: &Value, source: &Value) -> Value {
    let value = eval_current_argument_value(arg, source);
    let len = match &value {
        Value::String(value) => value.len() as i64,
        Value::Array(value) => value.len() as i64,
        Value::Object(value) => value.len() as i64,
        Value::Null => 0,
        _ => 1,
    };
    Value::Number(len.into())
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

fn is_empty_value(value: &Value) -> bool {
    match value {
        Value::Array(value) => value.is_empty(),
        Value::String(value) => value.is_empty(),
        Value::Object(value) => value.is_empty(),
        Value::Null => true,
        _ => false,
    }
}
