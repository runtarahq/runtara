//! Direct Conditional branch parity fixtures.
//!
//! These tests compare direct stdlib condition evaluation and branch selection
//! with the current generated-code condition semantics for the supported pure
//! direct branching shapes: `Conditional` true/false trees ending in `Finish`
//! leaves.

use runtara_workflow_stdlib::conditions::{is_truthy, to_number, values_equal};
use runtara_workflow_stdlib::direct_json::{DirectJsonManifest, build_source};
use runtara_workflows::ExecutionGraph;
use runtara_workflows::codegen::ast::mapping::path_to_json_pointer;
use runtara_workflows::direct_wasm::{DirectGraphManifest, build_direct_workflow_manifest};
use serde_json::{Map, Value, json};

const CONDITIONAL_WORKFLOW: &str = include_str!("fixtures/conditional_workflow.json");
const CONDITIONAL_LENGTH: &str = include_str!("fixtures/conditional_length_comparison.json");
const CONDITIONAL_NESTED: &str = include_str!("fixtures/conditional_nested.json");

#[test]
fn direct_conditional_branch_matches_current_semantics() {
    let cases = [
        (
            "eq true branch",
            CONDITIONAL_WORKFLOW,
            json!({ "flag": true }),
            json!({ "result": "yes" }),
        ),
        (
            "eq false branch",
            CONDITIONAL_WORKFLOW,
            json!({ "flag": false }),
            json!({ "result": "no" }),
        ),
        (
            "length true branch",
            CONDITIONAL_LENGTH,
            json!({ "description": "x".repeat(151) }),
            json!({ "result": "long" }),
        ),
        (
            "length false branch",
            CONDITIONAL_LENGTH,
            json!({ "description": "short" }),
            json!({ "result": "short" }),
        ),
        (
            "nested true true branch",
            CONDITIONAL_NESTED,
            json!({ "flag": true, "kind": "a" }),
            json!({ "result": "flag-kind-a" }),
        ),
        (
            "nested true false branch",
            CONDITIONAL_NESTED,
            json!({ "flag": true, "kind": "b" }),
            json!({ "result": "flag-kind-other" }),
        ),
        (
            "nested false branch",
            CONDITIONAL_NESTED,
            json!({ "flag": false, "kind": "a" }),
            json!({ "result": "flag-false" }),
        ),
    ];

    for (name, graph_json, data, expected_output) in cases {
        let (direct_branches, direct_output) = direct_branch_output(graph_json, &data);
        let (expected_branches, expected_current_output) = current_branch_output(graph_json, &data);

        assert_eq!(direct_branches, expected_branches, "branch case `{name}`");
        assert_eq!(
            direct_output, expected_current_output,
            "output case `{name}`"
        );
        assert_eq!(
            direct_output, expected_output,
            "fixture expectation `{name}`"
        );
    }
}

fn direct_branch_output(graph_json: &str, data: &Value) -> (Vec<bool>, Value) {
    let graph = parse_graph(graph_json);
    let manifest = build_direct_workflow_manifest(&graph).expect("manifest");
    let manifest_json = manifest.to_canonical_json().expect("manifest json");
    let direct_manifest = DirectJsonManifest::parse(&manifest_json).expect("direct manifest");
    let source = source_bytes(data, &manifest.graph.variables);

    direct_step_output(
        &manifest.graph,
        &direct_manifest,
        &source,
        &manifest.graph.entry_point,
    )
}

fn direct_step_output(
    graph: &DirectGraphManifest,
    direct_manifest: &DirectJsonManifest,
    source: &[u8],
    step_id: &str,
) -> (Vec<bool>, Value) {
    let step = graph
        .steps
        .iter()
        .find(|step| step.id == step_id)
        .expect("direct step");
    match step.step_type.as_str() {
        "Finish" => {
            let mapping_id = finish_mapping_id(graph, step_id);
            let output = direct_manifest
                .apply_mapping(mapping_id, source)
                .expect("finish mapping");
            (
                vec![],
                serde_json::from_slice(&output).expect("output json"),
            )
        }
        "Conditional" => {
            let condition_id = condition_id(graph, step_id);
            let branch = direct_manifest
                .eval_condition(condition_id, source)
                .expect("condition eval");
            let target = branch_target(graph, step_id, branch).to_string();
            let (mut branches, output) =
                direct_step_output(graph, direct_manifest, source, &target);
            branches.insert(0, branch);
            (branches, output)
        }
        other => panic!("unsupported direct parity step `{step_id}` type `{other}`"),
    }
}

fn current_branch_output(graph_json: &str, data: &Value) -> (Vec<bool>, Value) {
    let graph_value: Value = serde_json::from_str(graph_json).expect("graph json");
    let graph = parse_graph(graph_json);
    let manifest = build_direct_workflow_manifest(&graph).expect("manifest");
    let source_bytes = source_bytes(data, &manifest.graph.variables);
    let source: Value = serde_json::from_slice(&source_bytes).expect("source json");

    current_step_output(
        &graph_value,
        &manifest.graph,
        &source,
        &manifest.graph.entry_point,
    )
}

fn current_step_output(
    graph_value: &Value,
    graph: &DirectGraphManifest,
    source: &Value,
    step_id: &str,
) -> (Vec<bool>, Value) {
    let step = &graph_value["steps"][step_id];
    match step["stepType"].as_str().expect("stepType") {
        "Finish" => {
            let output = apply_current_input_mapping(&step["inputMapping"], source);
            (vec![], output.get("outputs").cloned().unwrap_or(output))
        }
        "Conditional" => {
            let branch = eval_current_condition(&step["condition"], source);
            let target = branch_target(graph, step_id, branch).to_string();
            let (mut branches, output) = current_step_output(graph_value, graph, source, &target);
            branches.insert(0, branch);
            (branches, output)
        }
        other => panic!("unsupported current parity step `{step_id}` type `{other}`"),
    }
}

fn parse_graph(graph_json: &str) -> ExecutionGraph {
    serde_json::from_str(graph_json).expect("fixture parses")
}

fn source_bytes(data: &Value, variables: &Value) -> Vec<u8> {
    build_source(
        data.to_string().as_bytes(),
        variables.to_string().as_bytes(),
        b"{}",
    )
    .expect("source")
}

fn branch_target<'a>(graph: &'a DirectGraphManifest, step_id: &str, branch: bool) -> &'a str {
    let label = if branch { "true" } else { "false" };
    graph
        .edges
        .iter()
        .find(|edge| edge.from_step == step_id && edge.label.as_deref() == Some(label))
        .map(|edge| edge.to_step.as_str())
        .expect("branch target")
}

fn condition_id(graph: &DirectGraphManifest, step_id: &str) -> u32 {
    graph
        .conditions
        .iter()
        .find(|condition| {
            condition.owner_id == step_id && condition.purpose == "conditional.condition"
        })
        .expect("conditional condition")
        .id
}

fn finish_mapping_id(graph: &DirectGraphManifest, step_id: &str) -> u32 {
    graph
        .mappings
        .iter()
        .find(|mapping| mapping.step_id == step_id && mapping.purpose == "finish.inputMapping")
        .expect("finish mapping")
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

fn apply_current_input_mapping(mapping: &Value, source: &Value) -> Value {
    let entries = mapping.as_object().expect("mapping object");
    let mut output = Map::new();
    for (key, value) in entries {
        insert_nested(&mut output, key, eval_current_mapping_value(value, source));
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

fn insert_nested(output: &mut Map<String, Value>, key: &str, value: Value) {
    let mut parts = key.split('.').peekable();
    let Some(first) = parts.next() else {
        return;
    };
    if parts.peek().is_none() {
        output.insert(first.to_string(), value);
        return;
    }

    let mut current = output
        .entry(first.to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    while let Some(part) = parts.next() {
        let is_last = parts.peek().is_none();
        if is_last {
            if let Value::Object(map) = current {
                map.insert(part.to_string(), value);
            }
            return;
        }

        if !current.is_object() {
            *current = Value::Object(Map::new());
        }
        current = current
            .as_object_mut()
            .expect("current was just forced to object")
            .entry(part.to_string())
            .or_insert_with(|| Value::Object(Map::new()));
    }
}
