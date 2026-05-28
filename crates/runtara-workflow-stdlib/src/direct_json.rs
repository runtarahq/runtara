// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! JSON semantics used by direct-emitted workflow components.
//!
//! This module is the pure Rust implementation behind the
//! `runtara:workflow-stdlib/json` WIT contract. The component wrapper can keep
//! a parsed [`DirectJsonManifest`] after `init-manifest` and delegate the WIT
//! functions here.

use std::collections::BTreeMap;

use serde::Deserialize;
use serde_json::{Map, Value};

use crate::template::render_template;

/// Parsed direct-workflow manifest data needed by JSON stdlib calls.
#[derive(Debug, Clone)]
pub struct DirectJsonManifest {
    mappings: BTreeMap<u32, DirectJsonMapping>,
}

impl DirectJsonManifest {
    /// Parse direct manifest JSON emitted by `runtara-workflows`.
    pub fn parse(bytes: &[u8]) -> Result<Self, String> {
        let manifest: ManifestWire = serde_json::from_slice(bytes)
            .map_err(|err| format!("failed to parse direct manifest: {err}"))?;
        let mut mappings = BTreeMap::new();
        collect_graph_mappings(&manifest.graph, &mut mappings)?;
        Ok(Self { mappings })
    }

    /// Apply a manifest mapping to a source JSON envelope.
    pub fn apply_mapping(&self, mapping_id: u32, source: &[u8]) -> Result<Vec<u8>, String> {
        let source: Value = serde_json::from_slice(source)
            .map_err(|err| format!("failed to parse mapping source: {err}"))?;
        let mapping = self
            .mappings
            .get(&mapping_id)
            .ok_or_else(|| format!("unknown direct mapping id {mapping_id}"))?;
        let mut output = apply_input_mapping(&mapping.value, &source)?;
        if mapping.purpose == "finish.inputMapping" {
            output = output.get("outputs").cloned().unwrap_or(output);
        }
        serde_json::to_vec(&output)
            .map_err(|err| format!("failed to serialize mapping output: {err}"))
    }
}

/// Build the source envelope consumed by direct mapping/condition helpers.
pub fn build_source(data: &[u8], variables: &[u8], steps: &[u8]) -> Result<Vec<u8>, String> {
    let data: Value =
        serde_json::from_slice(data).map_err(|err| format!("failed to parse data: {err}"))?;
    let variables: Value = serde_json::from_slice(variables)
        .map_err(|err| format!("failed to parse variables: {err}"))?;
    let steps: Value =
        serde_json::from_slice(steps).map_err(|err| format!("failed to parse steps: {err}"))?;

    let mut source = Map::new();
    source.insert("data".to_string(), data.clone());
    source.insert("variables".to_string(), variables.clone());
    source.insert("steps".to_string(), steps);

    let mut workflow_inputs = Map::new();
    workflow_inputs.insert("data".to_string(), data);
    workflow_inputs.insert("variables".to_string(), variables.clone());
    source.insert(
        "workflow".to_string(),
        serde_json::json!({ "inputs": Value::Object(workflow_inputs) }),
    );

    if let Some(loop_ctx) = variables.as_object().and_then(|vars| vars.get("_loop")) {
        source.insert("loop".to_string(), loop_ctx.clone());
    }
    if let Some(item) = variables.as_object().and_then(|vars| vars.get("_item")) {
        source.insert("item".to_string(), item.clone());
    }

    serde_json::to_vec(&Value::Object(source))
        .map_err(|err| format!("failed to serialize source: {err}"))
}

fn collect_graph_mappings(
    graph: &GraphWire,
    mappings: &mut BTreeMap<u32, DirectJsonMapping>,
) -> Result<(), String> {
    for mapping in &graph.mappings {
        if mappings
            .insert(
                mapping.id,
                DirectJsonMapping {
                    purpose: mapping.purpose.clone(),
                    value: mapping.value.clone(),
                },
            )
            .is_some()
        {
            return Err(format!("duplicate direct mapping id {}", mapping.id));
        }
    }
    for step in &graph.steps {
        for nested in &step.nested_graphs {
            collect_graph_mappings(&nested.graph, mappings)?;
        }
    }
    Ok(())
}

fn apply_input_mapping(mapping: &Value, source: &Value) -> Result<Value, String> {
    let Value::Object(entries) = mapping else {
        return Err("input mapping must be a JSON object".to_string());
    };

    let mut output = Map::new();
    for (key, value) in entries {
        let value = apply_mapping_value(value, source)?;
        insert_nested(&mut output, key, value);
    }
    Ok(Value::Object(output))
}

fn apply_mapping_value(value: &Value, source: &Value) -> Result<Value, String> {
    let Value::Object(map) = value else {
        return Err("mapping value must be an object".to_string());
    };
    let value_type = map
        .get("valueType")
        .and_then(Value::as_str)
        .ok_or_else(|| "mapping value missing valueType".to_string())?;

    match value_type {
        "reference" => apply_reference(map, source),
        "immediate" => Ok(map.get("value").cloned().unwrap_or(Value::Null)),
        "composite" => apply_composite(map.get("value").unwrap_or(&Value::Null), source),
        "template" => {
            let template = map
                .get("value")
                .and_then(Value::as_str)
                .ok_or_else(|| "template mapping value must be a string".to_string())?;
            render_template(template, source).map(Value::String)
        }
        other => Err(format!("unsupported mapping valueType '{other}'")),
    }
}

fn apply_reference(map: &Map<String, Value>, source: &Value) -> Result<Value, String> {
    let path = map
        .get("value")
        .and_then(Value::as_str)
        .ok_or_else(|| "reference mapping value must be a string path".to_string())?;
    let default = map.get("default").cloned();
    let value = match lookup_source_path(source, path) {
        Some(Value::Null) | None => default.unwrap_or(Value::Null),
        Some(value) => value,
    };
    Ok(apply_type_hint(
        value,
        map.get("type").and_then(Value::as_str),
    ))
}

fn apply_composite(value: &Value, source: &Value) -> Result<Value, String> {
    match value {
        Value::Object(map) => {
            let mut output = Map::new();
            for (key, child) in map {
                output.insert(key.clone(), apply_mapping_value(child, source)?);
            }
            Ok(Value::Object(output))
        }
        Value::Array(items) => items
            .iter()
            .map(|item| apply_mapping_value(item, source))
            .collect::<Result<Vec<_>, _>>()
            .map(Value::Array),
        _ => Err("composite mapping value must be an object or array".to_string()),
    }
}

fn apply_type_hint(value: Value, type_hint: Option<&str>) -> Value {
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

fn lookup_source_path(source: &Value, path: &str) -> Option<Value> {
    let pointer = path_to_json_pointer(path);
    source.pointer(&pointer).cloned()
}

fn path_to_json_pointer(path: &str) -> String {
    let normalized = path
        .replace("['", ".")
        .replace("']", "")
        .replace("[\"", ".")
        .replace("\"]", "");

    let mut dotted = String::new();
    let mut chars = normalized.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '[' {
            let mut index = String::new();
            while let Some(&next_ch) = chars.peek() {
                if next_ch == ']' {
                    chars.next();
                    break;
                }
                index.push(chars.next().expect("peeked character exists"));
            }
            if index.chars().all(|c| c.is_ascii_digit()) {
                dotted.push('.');
                dotted.push_str(&index);
            } else {
                dotted.push('[');
                dotted.push_str(&index);
                dotted.push(']');
            }
        } else {
            dotted.push(ch);
        }
    }

    let mut out = String::with_capacity(dotted.len() + 4);
    for segment in dotted.split('.') {
        out.push('/');
        for ch in segment.chars() {
            match ch {
                '~' => out.push_str("~0"),
                '/' => out.push_str("~1"),
                _ => out.push(ch),
            }
        }
    }
    out
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ManifestWire {
    graph: GraphWire,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GraphWire {
    #[serde(default)]
    mappings: Vec<MappingWire>,
    #[serde(default)]
    steps: Vec<StepWire>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StepWire {
    #[serde(default)]
    nested_graphs: Vec<NestedGraphWire>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct NestedGraphWire {
    graph: GraphWire,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MappingWire {
    id: u32,
    purpose: String,
    value: Value,
}

#[derive(Debug, Clone)]
struct DirectJsonMapping {
    purpose: String,
    value: Value,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn manifest(mapping_value: Value) -> Vec<u8> {
        serde_json::to_vec(&json!({
            "graph": {
                "mappings": [{
                    "id": 0,
                    "stepId": "finish",
                    "stepType": "Finish",
                    "purpose": "finish.inputMapping",
                    "value": mapping_value
                }],
                "steps": []
            }
        }))
        .expect("manifest json")
    }

    #[test]
    fn build_source_matches_generated_workflow_shape() {
        let source = build_source(
            br#"{"input":"hello"}"#,
            br#"{"tenant":"t1","_item":{"id":7}}"#,
            br#"{"previous":{"outputs":{"ok":true}}}"#,
        )
        .expect("source");
        let source: Value = serde_json::from_slice(&source).expect("source json");

        assert_eq!(source["data"]["input"], "hello");
        assert_eq!(source["variables"]["tenant"], "t1");
        assert_eq!(source["steps"]["previous"]["outputs"]["ok"], true);
        assert_eq!(source["workflow"]["inputs"]["data"]["input"], "hello");
        assert_eq!(source["workflow"]["inputs"]["variables"]["tenant"], "t1");
        assert_eq!(source["item"]["id"], 7);
    }

    #[test]
    fn apply_finish_mapping_resolves_simple_passthrough() {
        let manifest = DirectJsonManifest::parse(&manifest(json!({
            "result": { "valueType": "reference", "value": "data.input" }
        })))
        .expect("manifest");
        let source = build_source(br#"{"input":"hello"}"#, b"{}", b"{}").expect("source");

        let output = manifest.apply_mapping(0, &source).expect("mapping output");
        let output: Value = serde_json::from_slice(&output).expect("output json");

        assert_eq!(output, json!({ "result": "hello" }));
    }

    #[test]
    fn finish_mapping_unwraps_outputs_field_after_dotted_insert() {
        let manifest = DirectJsonManifest::parse(&manifest(json!({
            "outputs.value": { "valueType": "immediate", "value": 7 }
        })))
        .expect("manifest");
        let source = build_source(b"{}", b"{}", b"{}").expect("source");

        let output = manifest.apply_mapping(0, &source).expect("mapping output");
        let output: Value = serde_json::from_slice(&output).expect("output json");

        assert_eq!(output, json!({ "value": 7 }));
    }

    #[test]
    fn apply_mapping_handles_defaults_templates_and_composites() {
        let manifest = DirectJsonManifest::parse(&manifest(json!({
            "fallback": {
                "valueType": "reference",
                "value": "data.missing",
                "type": "string",
                "default": 42
            },
            "nullFallback": {
                "valueType": "reference",
                "value": "data.nullish",
                "default": "defaulted"
            },
            "message": {
                "valueType": "template",
                "value": "hello {{ data.name }}"
            },
            "nested": {
                "valueType": "composite",
                "value": {
                    "first": { "valueType": "reference", "value": "steps.prev.outputs.first" },
                    "items": {
                        "valueType": "composite",
                        "value": [
                            { "valueType": "reference", "value": "workflow.inputs.data.name" },
                            { "valueType": "immediate", "value": true }
                        ]
                    }
                }
            }
        })))
        .expect("manifest");
        let source = build_source(
            br#"{"name":"Ada","nullish":null}"#,
            b"{}",
            br#"{"prev":{"outputs":{"first":"alpha"}}}"#,
        )
        .expect("source");

        let output = manifest.apply_mapping(0, &source).expect("mapping output");
        let output: Value = serde_json::from_slice(&output).expect("output json");

        assert_eq!(
            output,
            json!({
                "fallback": "42",
                "nullFallback": "defaulted",
                "message": "hello Ada",
                "nested": {
                    "first": "alpha",
                    "items": ["Ada", true]
                }
            })
        );
    }
}
