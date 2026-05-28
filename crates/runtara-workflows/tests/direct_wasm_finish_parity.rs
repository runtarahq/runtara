//! Direct Finish mapping parity fixtures.
//!
//! These tests keep the direct JSON stdlib aligned with the current
//! Rust-generated Finish mapping contract before direct mode expands past the
//! single-entry Finish shape.

use runtara_workflow_stdlib::direct_json::{DirectJsonManifest, build_source};
use runtara_workflows::ExecutionGraph;
use runtara_workflows::codegen::ast::mapping::path_to_json_pointer;
use runtara_workflows::direct_wasm::build_direct_workflow_manifest;
use serde_json::{Map, Value, json};

#[derive(Debug)]
struct FinishParityCase {
    name: &'static str,
    mapping: Value,
    data: Value,
    variables: Value,
    steps: Value,
}

#[test]
fn direct_finish_mapping_matches_rust_codegen_semantics() {
    for case in finish_parity_cases() {
        let direct = direct_finish_output(&case);
        let expected = rust_codegen_finish_output(&case.mapping, &case.source());

        assert_eq!(direct, expected, "case `{}`", case.name);
    }
}

fn finish_parity_cases() -> Vec<FinishParityCase> {
    vec![
        FinishParityCase {
            name: "simple data passthrough",
            mapping: json!({
                "result": { "valueType": "reference", "value": "data.input" }
            }),
            data: json!({ "input": "hello" }),
            variables: json!({}),
            steps: json!({}),
        },
        FinishParityCase {
            name: "finish outputs unwrap after dotted insert",
            mapping: json!({
                "outputs.value": { "valueType": "immediate", "value": 7 },
                "outputs.label": {
                    "valueType": "template",
                    "value": "hello {{ data.name }}"
                }
            }),
            data: json!({ "name": "Ada" }),
            variables: json!({}),
            steps: json!({}),
        },
        FinishParityCase {
            name: "defaults apply to missing and null references",
            mapping: json!({
                "missing": {
                    "valueType": "reference",
                    "value": "data.missing",
                    "type": "string",
                    "default": 42
                },
                "nullDefault": {
                    "valueType": "reference",
                    "value": "data.nullish",
                    "default": "fallback"
                }
            }),
            data: json!({ "nullish": null }),
            variables: json!({}),
            steps: json!({}),
        },
        FinishParityCase {
            name: "composite values resolve workflow variables and steps",
            mapping: json!({
                "bundle": {
                    "valueType": "composite",
                    "value": {
                        "tenant": {
                            "valueType": "reference",
                            "value": "variables.tenant"
                        },
                        "previous": {
                            "valueType": "reference",
                            "value": "steps.prev.outputs.value"
                        },
                        "items": {
                            "valueType": "composite",
                            "value": [
                                {
                                    "valueType": "reference",
                                    "value": "workflow.inputs.data.input"
                                },
                                { "valueType": "immediate", "value": true }
                            ]
                        }
                    }
                }
            }),
            data: json!({ "input": "payload" }),
            variables: json!({ "tenant": "tenant-a" }),
            steps: json!({ "prev": { "outputs": { "value": "from-step" } } }),
        },
    ]
}

impl FinishParityCase {
    fn source(&self) -> Value {
        let bytes = build_source(
            self.data.to_string().as_bytes(),
            self.variables.to_string().as_bytes(),
            self.steps.to_string().as_bytes(),
        )
        .expect("source builds");
        serde_json::from_slice(&bytes).expect("source json")
    }
}

fn direct_finish_output(case: &FinishParityCase) -> Value {
    let graph = graph_with_finish_mapping(&case.mapping);
    let manifest = build_direct_workflow_manifest(&graph).expect("direct manifest");
    let mapping_id = manifest
        .graph
        .mappings
        .iter()
        .find(|mapping| mapping.purpose == "finish.inputMapping")
        .expect("finish mapping")
        .id;
    let manifest_json = manifest.to_canonical_json().expect("manifest json");
    let direct_manifest = DirectJsonManifest::parse(&manifest_json).expect("direct json manifest");
    let source = build_source(
        case.data.to_string().as_bytes(),
        case.variables.to_string().as_bytes(),
        case.steps.to_string().as_bytes(),
    )
    .expect("source builds");
    let output = direct_manifest
        .apply_mapping(mapping_id, &source)
        .expect("direct mapping output");

    serde_json::from_slice(&output).expect("direct output json")
}

fn graph_with_finish_mapping(mapping: &Value) -> ExecutionGraph {
    serde_json::from_value(json!({
        "name": "direct finish parity",
        "steps": {
            "finish": {
                "stepType": "Finish",
                "id": "finish",
                "inputMapping": mapping
            }
        },
        "entryPoint": "finish",
        "executionPlan": [],
        "variables": {},
        "inputSchema": {},
        "outputSchema": {}
    }))
    .expect("finish parity graph parses")
}

fn rust_codegen_finish_output(mapping: &Value, source: &Value) -> Value {
    let output = apply_rust_codegen_input_mapping(mapping, source);
    output.get("outputs").cloned().unwrap_or(output)
}

fn apply_rust_codegen_input_mapping(mapping: &Value, source: &Value) -> Value {
    let entries = mapping.as_object().expect("mapping object");
    let mut output = Map::new();
    for (key, value) in entries {
        let value = apply_rust_codegen_mapping_value(value, source);
        insert_nested(&mut output, key, value);
    }
    Value::Object(output)
}

fn apply_rust_codegen_mapping_value(value: &Value, source: &Value) -> Value {
    let map = value.as_object().expect("mapping value object");
    match map
        .get("valueType")
        .and_then(Value::as_str)
        .expect("mapping valueType")
    {
        "reference" => apply_rust_codegen_reference(map, source),
        "immediate" => map.get("value").cloned().unwrap_or(Value::Null),
        "composite" => {
            apply_rust_codegen_composite(map.get("value").unwrap_or(&Value::Null), source)
        }
        "template" => {
            let template = map
                .get("value")
                .and_then(Value::as_str)
                .expect("template string");
            runtara_workflow_stdlib::template::render_template(template, source)
                .map(Value::String)
                .unwrap_or_else(|err| Value::String(format!("Template error: {err}")))
        }
        other => panic!("unsupported mapping valueType `{other}`"),
    }
}

fn apply_rust_codegen_reference(map: &Map<String, Value>, source: &Value) -> Value {
    let path = map
        .get("value")
        .and_then(Value::as_str)
        .expect("reference path");
    let pointer = path_to_json_pointer(path);
    let looked_up = source.pointer(&pointer).cloned();
    let value = match looked_up {
        Some(Value::Null) | None => map.get("default").cloned().unwrap_or(Value::Null),
        Some(value) => value,
    };
    apply_rust_codegen_type_hint(value, map.get("type").and_then(Value::as_str))
}

fn apply_rust_codegen_composite(value: &Value, source: &Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(key, value)| (key.clone(), apply_rust_codegen_mapping_value(value, source)))
                .collect(),
        ),
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|item| apply_rust_codegen_mapping_value(item, source))
                .collect(),
        ),
        _ => panic!("composite mapping value must be an object or array"),
    }
}

fn apply_rust_codegen_type_hint(value: Value, type_hint: Option<&str>) -> Value {
    match type_hint {
        Some("string") => match value {
            Value::String(_) | Value::Null => value,
            Value::Number(number) => Value::String(number.to_string()),
            Value::Bool(boolean) => Value::String(boolean.to_string()),
            other => Value::String(other.to_string()),
        },
        Some("integer") => value
            .as_i64()
            .or_else(|| value.as_f64().map(|value| value as i64))
            .or_else(|| value.as_str().and_then(|value| value.parse::<i64>().ok()))
            .or_else(|| value.as_bool().map(|value| if value { 1 } else { 0 }))
            .map(|value| Value::Number(value.into()))
            .unwrap_or_else(|| {
                if value.is_null() {
                    Value::Null
                } else {
                    Value::Number(0.into())
                }
            }),
        Some("number") => value
            .as_f64()
            .or_else(|| value.as_str().and_then(|value| value.parse::<f64>().ok()))
            .and_then(serde_json::Number::from_f64)
            .map(Value::Number)
            .unwrap_or_else(|| {
                if value.is_null() {
                    Value::Null
                } else {
                    Value::Number(0.into())
                }
            }),
        Some("boolean") => match value {
            Value::Bool(_) | Value::Null => value,
            Value::String(value) => Value::Bool(value == "true" || value == "1"),
            Value::Number(value) => Value::Bool(value.as_i64().is_some_and(|value| value != 0)),
            Value::Array(value) => Value::Bool(!value.is_empty()),
            Value::Object(value) => Value::Bool(!value.is_empty()),
        },
        Some("json" | "file") | None => value,
        Some(_) => value,
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
