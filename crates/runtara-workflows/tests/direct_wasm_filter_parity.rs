//! Direct Filter parity fixtures.
//!
//! These tests compare the shared direct JSON stdlib Filter helper with the
//! current generated-code Filter semantics before broader pure JSON lowering.

use runtara_workflow_stdlib::conditions::{is_truthy, to_number, values_equal};
use runtara_workflow_stdlib::direct_json::{DirectJsonManifest, build_source};
use runtara_workflows::ExecutionGraph;
use runtara_workflows::codegen::ast::mapping::path_to_json_pointer;
use runtara_workflows::direct_wasm::{DirectGraphManifest, build_direct_workflow_manifest};
use serde_json::{Value, json};

const FILTER_SIMPLE: &str = include_str!("fixtures/filter_simple.json");
const FILTER_WITH_NOT: &str = include_str!("fixtures/filter_with_not.json");
const FILTER_COMPLEX_CONDITION: &str = include_str!("fixtures/filter_complex_condition.json");

#[test]
fn direct_filter_matches_current_semantics() {
    let cases = [
        (
            "simple equality",
            FILTER_SIMPLE,
            json!({
                "items": [
                    { "id": 1, "status": "active" },
                    { "id": 2, "status": "failed" },
                    { "id": 3, "status": "active" }
                ]
            }),
            vec![1, 3],
        ),
        ("not condition", FILTER_WITH_NOT, json!({}), vec![1, 3]),
        (
            "nested boolean condition",
            FILTER_COMPLEX_CONDITION,
            json!({
                "users": [
                    { "id": 1, "status": "active", "age": 19, "role": "user" },
                    { "id": 2, "status": "active", "age": 17, "role": "user" },
                    { "id": 3, "status": "disabled", "age": 15, "role": "admin" }
                ]
            }),
            vec![1, 3],
        ),
    ];

    for (name, graph_json, data, expected_ids) in cases {
        let direct_output = direct_filter_output(graph_json, &data);
        let current_output = current_filter_output(graph_json, &data);

        assert_eq!(direct_output, current_output, "filter output case `{name}`");
        assert_eq!(
            direct_output["count"],
            json!(expected_ids.len()),
            "count case `{name}`"
        );
        let actual_ids = direct_output["items"]
            .as_array()
            .expect("items array")
            .iter()
            .map(|item| item["id"].as_i64().expect("item id") as i32)
            .collect::<Vec<_>>();
        assert_eq!(actual_ids, expected_ids, "item ids case `{name}`");
    }
}

fn direct_filter_output(graph_json: &str, data: &Value) -> Value {
    let graph: ExecutionGraph = serde_json::from_str(graph_json).expect("fixture parses");
    let manifest = build_direct_workflow_manifest(&graph).expect("manifest");
    let manifest_json = manifest.to_canonical_json().expect("manifest json");
    let direct_manifest = DirectJsonManifest::parse(&manifest_json).expect("direct manifest");
    let source = source_bytes(data, &manifest.graph.variables);

    let filter_id = filter_id(&manifest.graph, &manifest.graph.entry_point);
    let output = direct_manifest
        .filter(filter_id, &source)
        .expect("filter output");
    let steps: Value = serde_json::from_slice(&output).expect("steps json");

    steps[&manifest.graph.entry_point]["outputs"].clone()
}

fn current_filter_output(graph_json: &str, data: &Value) -> Value {
    let graph_value: Value = serde_json::from_str(graph_json).expect("graph json");
    let graph: ExecutionGraph = serde_json::from_str(graph_json).expect("fixture parses");
    let manifest = build_direct_workflow_manifest(&graph).expect("manifest");
    let source_bytes = source_bytes(data, &manifest.graph.variables);
    let source: Value = serde_json::from_slice(&source_bytes).expect("source json");

    let entry = graph_value["entryPoint"].as_str().expect("entry point");
    let config = &graph_value["steps"][entry]["config"];
    apply_current_filter(config, &source)
}

fn apply_current_filter(config: &Value, source: &Value) -> Value {
    let input = apply_current_mapping_value(&config["value"], source);
    let items = input.as_array().cloned().unwrap_or_default();
    let condition = &config["condition"];
    let mut filtered = Vec::new();

    for item in items {
        let mut source = source.clone();
        source
            .as_object_mut()
            .expect("source should be an object")
            .insert("item".to_string(), item.clone());
        if eval_current_condition(condition, &source) {
            filtered.push(item);
        }
    }

    let count = filtered.len();
    json!({
        "items": filtered,
        "count": count,
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

fn filter_id(graph: &DirectGraphManifest, step_id: &str) -> u32 {
    graph
        .filters
        .iter()
        .find(|filter| filter.step_id == step_id && filter.purpose == "filter.config")
        .expect("filter config")
        .id
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
        is_truthy(&apply_current_mapping_value(expr, source))
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
        "LENGTH" => args
            .first()
            .map(|arg| is_truthy(&eval_current_length_value(arg, source)))
            .unwrap_or(false),
        _ => panic!("unsupported fixture condition operator `{op}`"),
    }
}

fn eval_current_argument_bool(arg: &Value, source: &Value) -> bool {
    if arg.get("op").is_some() {
        eval_current_condition(arg, source)
    } else {
        is_truthy(&apply_current_mapping_value(arg, source))
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
        apply_current_mapping_value(arg, source)
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

fn apply_current_mapping_value(value: &Value, source: &Value) -> Value {
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
