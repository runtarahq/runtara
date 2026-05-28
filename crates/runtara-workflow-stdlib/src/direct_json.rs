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

use crate::conditions::{is_truthy, to_number, values_equal};
use crate::switch_helpers::process_switch_output;
use crate::template::render_template;

/// Parsed direct-workflow manifest data needed by JSON stdlib calls.
#[derive(Debug, Clone)]
pub struct DirectJsonManifest {
    mappings: BTreeMap<u32, DirectJsonMapping>,
    conditions: BTreeMap<u32, Value>,
    filters: BTreeMap<u32, DirectJsonFilter>,
    switches: BTreeMap<u32, DirectJsonSwitch>,
    group_bys: BTreeMap<u32, DirectJsonGroupBy>,
}

impl DirectJsonManifest {
    /// Parse direct manifest JSON emitted by `runtara-workflows`.
    pub fn parse(bytes: &[u8]) -> Result<Self, String> {
        let manifest: ManifestWire = serde_json::from_slice(bytes)
            .map_err(|err| format!("failed to parse direct manifest: {err}"))?;
        let mut mappings = BTreeMap::new();
        let mut conditions = BTreeMap::new();
        let mut filters = BTreeMap::new();
        let mut switches = BTreeMap::new();
        let mut group_bys = BTreeMap::new();
        collect_graph_manifest(
            &manifest.graph,
            &mut mappings,
            &mut conditions,
            &mut filters,
            &mut switches,
            &mut group_bys,
        )?;
        Ok(Self {
            mappings,
            conditions,
            filters,
            switches,
            group_bys,
        })
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

    /// Evaluate a manifest condition against a source JSON envelope.
    pub fn eval_condition(&self, condition_id: u32, source: &[u8]) -> Result<bool, String> {
        let source: Value = serde_json::from_slice(source)
            .map_err(|err| format!("failed to parse condition source: {err}"))?;
        let condition = self
            .conditions
            .get(&condition_id)
            .ok_or_else(|| format!("unknown direct condition id {condition_id}"))?;
        eval_condition_expression(condition, &source)
    }

    /// Execute a manifest Filter config and return an updated steps context.
    pub fn filter(&self, filter_id: u32, source: &[u8]) -> Result<Vec<u8>, String> {
        let source: Value = serde_json::from_slice(source)
            .map_err(|err| format!("failed to parse filter source: {err}"))?;
        let filter = self
            .filters
            .get(&filter_id)
            .ok_or_else(|| format!("unknown direct Filter id {filter_id}"))?;
        let output = apply_filter(&filter.value, &source)?;
        let steps = insert_step_output(
            &source,
            &filter.step_id,
            filter.name.as_deref(),
            "Filter",
            output,
        );
        serde_json::to_vec(&Value::Object(steps))
            .map_err(|err| format!("failed to serialize filter steps context: {err}"))
    }

    /// Execute a manifest value Switch config and return an updated steps context.
    pub fn value_switch(&self, switch_id: u32, source: &[u8]) -> Result<Vec<u8>, String> {
        let source: Value = serde_json::from_slice(source)
            .map_err(|err| format!("failed to parse value-switch source: {err}"))?;
        let switch = self
            .switches
            .get(&switch_id)
            .ok_or_else(|| format!("unknown direct Switch id {switch_id}"))?;
        let output = apply_value_switch(&switch.value, &source)?;
        let steps = insert_step_output(
            &source,
            &switch.step_id,
            switch.name.as_deref(),
            "Switch",
            output,
        );
        serde_json::to_vec(&Value::Object(steps))
            .map_err(|err| format!("failed to serialize value-switch steps context: {err}"))
    }

    /// Execute a manifest GroupBy config and return an updated steps context.
    pub fn group_by(&self, group_id: u32, source: &[u8]) -> Result<Vec<u8>, String> {
        let source: Value = serde_json::from_slice(source)
            .map_err(|err| format!("failed to parse group-by source: {err}"))?;
        let group_by = self
            .group_bys
            .get(&group_id)
            .ok_or_else(|| format!("unknown direct GroupBy id {group_id}"))?;
        let output = apply_group_by(&group_by.value, &source)?;
        let steps = insert_step_output(
            &source,
            &group_by.step_id,
            group_by.name.as_deref(),
            "GroupBy",
            output,
        );
        serde_json::to_vec(&Value::Object(steps))
            .map_err(|err| format!("failed to serialize group-by steps context: {err}"))
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

fn collect_graph_manifest(
    graph: &GraphWire,
    mappings: &mut BTreeMap<u32, DirectJsonMapping>,
    conditions: &mut BTreeMap<u32, Value>,
    filters: &mut BTreeMap<u32, DirectJsonFilter>,
    switches: &mut BTreeMap<u32, DirectJsonSwitch>,
    group_bys: &mut BTreeMap<u32, DirectJsonGroupBy>,
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
    for condition in &graph.conditions {
        if conditions
            .insert(condition.id, condition.value.clone())
            .is_some()
        {
            return Err(format!("duplicate direct condition id {}", condition.id));
        }
    }
    for filter in &graph.filters {
        if filters
            .insert(
                filter.id,
                DirectJsonFilter {
                    step_id: filter.step_id.clone(),
                    name: filter.name.clone(),
                    value: filter.value.clone(),
                },
            )
            .is_some()
        {
            return Err(format!("duplicate direct Filter id {}", filter.id));
        }
    }
    for switch in &graph.switches {
        if switches
            .insert(
                switch.id,
                DirectJsonSwitch {
                    step_id: switch.step_id.clone(),
                    name: switch.name.clone(),
                    value: switch.value.clone(),
                },
            )
            .is_some()
        {
            return Err(format!("duplicate direct Switch id {}", switch.id));
        }
    }
    for group_by in &graph.group_bys {
        if group_bys
            .insert(
                group_by.id,
                DirectJsonGroupBy {
                    step_id: group_by.step_id.clone(),
                    name: group_by.name.clone(),
                    value: group_by.value.clone(),
                },
            )
            .is_some()
        {
            return Err(format!("duplicate direct GroupBy id {}", group_by.id));
        }
    }
    for step in &graph.steps {
        for nested in &step.nested_graphs {
            collect_graph_manifest(
                &nested.graph,
                mappings,
                conditions,
                filters,
                switches,
                group_bys,
            )?;
        }
    }
    Ok(())
}

fn apply_filter(config: &Value, source: &Value) -> Result<Value, String> {
    let input = config
        .get("value")
        .ok_or_else(|| "Filter config missing value".to_string())
        .and_then(|value| apply_mapping_value(value, source))?;
    let items = input.as_array().cloned().unwrap_or_default();
    let condition = config
        .get("condition")
        .ok_or_else(|| "Filter config missing condition".to_string())?;
    let mut source = source.clone();
    if !source.is_object() {
        return Err("filter source must be a JSON object".to_string());
    }

    let mut filtered = Vec::new();
    for item in items {
        source
            .as_object_mut()
            .expect("filter source was checked as object")
            .insert("item".to_string(), item.clone());
        if eval_condition_expression(condition, &source)? {
            filtered.push(item);
        }
    }

    Ok(serde_json::json!({
        "items": filtered,
        "count": filtered.len(),
    }))
}

fn apply_value_switch(config: &Value, source: &Value) -> Result<Value, String> {
    let Some(switch_value) = config.get("value") else {
        let default = config
            .get("default")
            .cloned()
            .unwrap_or_else(|| Value::Object(Map::new()));
        return Ok(process_switch_output(&default, source));
    };

    if let Some(cases) = config.get("cases").and_then(Value::as_array) {
        for case in cases {
            let condition = switch_case_condition(switch_value, case)?;
            if eval_condition_expression(&condition, source)? {
                let output = case
                    .get("output")
                    .ok_or_else(|| "Switch case missing output".to_string())?;
                return Ok(process_switch_output(output, source));
            }
        }
    }

    let default = config
        .get("default")
        .cloned()
        .unwrap_or_else(|| Value::Object(Map::new()));
    Ok(process_switch_output(&default, source))
}

fn switch_case_condition(switch_value: &Value, case: &Value) -> Result<Value, String> {
    let match_type = case
        .get("matchType")
        .and_then(Value::as_str)
        .ok_or_else(|| "Switch case missing matchType".to_string())?;
    let match_value = case.get("match").cloned().unwrap_or(Value::Null);
    let right = serde_json::json!({
        "valueType": "immediate",
        "value": match_value,
    });

    match match_type {
        "EQ" if case.get("match").is_some_and(Value::is_array) => {
            Ok(binary_condition("IN", switch_value.clone(), right))
        }
        "EQ" | "NE" | "GT" | "GTE" | "LT" | "LTE" | "STARTS_WITH" | "ENDS_WITH" | "CONTAINS"
        | "IN" | "NOT_IN" => Ok(binary_condition(match_type, switch_value.clone(), right)),
        "IS_DEFINED" | "IS_EMPTY" | "IS_NOT_EMPTY" => {
            Ok(unary_condition(match_type, switch_value.clone()))
        }
        "BETWEEN" => Ok(build_between_condition(switch_value, &match_value)),
        "RANGE" => Ok(build_range_condition(switch_value, &match_value)),
        other => Err(format!("unsupported Switch matchType '{other}'")),
    }
}

fn binary_condition(op: &str, left: Value, right: Value) -> Value {
    serde_json::json!({
        "type": "operation",
        "op": op,
        "arguments": [left, right],
    })
}

fn unary_condition(op: &str, value: Value) -> Value {
    serde_json::json!({
        "type": "operation",
        "op": op,
        "arguments": [value],
    })
}

fn value_condition(value: bool) -> Value {
    serde_json::json!({
        "type": "value",
        "valueType": "immediate",
        "value": value,
    })
}

fn build_between_condition(switch_value: &Value, match_value: &Value) -> Value {
    let Some(bounds) = match_value.as_array().filter(|bounds| bounds.len() >= 2) else {
        return value_condition(false);
    };

    serde_json::json!({
        "type": "operation",
        "op": "AND",
        "arguments": [
            binary_condition(
                "GTE",
                switch_value.clone(),
                serde_json::json!({ "valueType": "immediate", "value": bounds[0].clone() }),
            ),
            binary_condition(
                "LTE",
                switch_value.clone(),
                serde_json::json!({ "valueType": "immediate", "value": bounds[1].clone() }),
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
                serde_json::json!({ "valueType": "immediate", "value": value.clone() }),
            ));
        }
    }

    match conditions.len() {
        0 => value_condition(true),
        1 => conditions.remove(0),
        _ => serde_json::json!({
            "type": "operation",
            "op": "AND",
            "arguments": conditions,
        }),
    }
}

fn apply_group_by(config: &Value, source: &Value) -> Result<Value, String> {
    let input = config
        .get("value")
        .ok_or_else(|| "GroupBy config missing value".to_string())
        .and_then(|value| apply_mapping_value(value, source))?;
    let items = input.as_array().cloned().unwrap_or_default();
    let key = config
        .get("key")
        .and_then(Value::as_str)
        .ok_or_else(|| "GroupBy config missing key".to_string())?;
    let pointer = path_to_json_pointer(key);

    let mut groups = BTreeMap::<String, Vec<Value>>::new();
    let mut counts = BTreeMap::<String, usize>::new();
    if let Some(expected_keys) = config.get("expectedKeys").and_then(Value::as_array) {
        for key in expected_keys.iter().filter_map(Value::as_str) {
            groups.entry(key.to_string()).or_default();
            counts.entry(key.to_string()).or_insert(0);
        }
    }

    for item in items {
        let key = item.pointer(&pointer).cloned().unwrap_or(Value::Null);
        let key = group_key_string(&key);
        groups.entry(key.clone()).or_default().push(item);
        *counts.entry(key).or_insert(0) += 1;
    }

    Ok(serde_json::json!({
        "groups": groups,
        "counts": counts,
        "total_groups": groups.len(),
    }))
}

fn insert_step_output(
    source: &Value,
    step_id: &str,
    step_name: Option<&str>,
    step_type: &str,
    output: Value,
) -> Map<String, Value> {
    let mut steps = source
        .get("steps")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    steps.insert(
        step_id.to_string(),
        serde_json::json!({
            "stepId": step_id,
            "stepName": step_name.unwrap_or("Unnamed"),
            "stepType": step_type,
            "outputs": output,
        }),
    );
    steps
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

fn eval_condition_expression(expr: &Value, source: &Value) -> Result<bool, String> {
    if is_condition_operation(expr) {
        eval_condition_operation(expr, source)
    } else {
        eval_condition_value(expr, source).map(|value| is_truthy(&value))
    }
}

fn is_condition_operation(expr: &Value) -> bool {
    expr.get("op").is_some()
        || expr
            .get("type")
            .and_then(Value::as_str)
            .is_some_and(|value| value == "operation")
}

fn eval_condition_operation(expr: &Value, source: &Value) -> Result<bool, String> {
    let op = expr
        .get("op")
        .and_then(Value::as_str)
        .ok_or_else(|| "condition operation missing op".to_string())?;
    let args = expr
        .get("arguments")
        .and_then(Value::as_array)
        .ok_or_else(|| "condition operation missing arguments".to_string())?;

    match op {
        "AND" => args.iter().try_fold(true, |acc, arg| {
            if !acc {
                Ok(false)
            } else {
                eval_condition_argument_as_bool(arg, source)
            }
        }),
        "OR" => args.iter().try_fold(false, |acc, arg| {
            if acc {
                Ok(true)
            } else {
                eval_condition_argument_as_bool(arg, source)
            }
        }),
        "NOT" => args
            .first()
            .map(|arg| eval_condition_argument_as_bool(arg, source).map(|value| !value))
            .unwrap_or(Ok(true)),
        "GT" | "GTE" | "LT" | "LTE" => eval_comparison(op, args, source),
        "EQ" | "NE" => eval_equality(op, args, source),
        "STARTS_WITH" | "ENDS_WITH" => eval_string_match(op, args, source),
        "CONTAINS" | "IN" | "NOT_IN" => eval_array_match(op, args, source),
        "LENGTH" => eval_length_as_value(args, source).map(|value| {
            value
                .as_i64()
                .or_else(|| value.as_u64().map(|value| value as i64))
                .unwrap_or(0)
                > 0
        }),
        "IS_DEFINED" => args
            .first()
            .map(|arg| eval_condition_argument_as_value(arg, source).map(|value| !value.is_null()))
            .unwrap_or(Ok(false)),
        "IS_EMPTY" => args
            .first()
            .map(|arg| {
                eval_condition_argument_as_value(arg, source).map(|value| match value {
                    Value::Array(value) => value.is_empty(),
                    Value::String(value) => value.is_empty(),
                    Value::Object(value) => value.is_empty(),
                    Value::Null => true,
                    _ => false,
                })
            })
            .unwrap_or(Ok(true)),
        "IS_NOT_EMPTY" => args
            .first()
            .map(|arg| {
                eval_condition_argument_as_value(arg, source).map(|value| match value {
                    Value::Array(value) => !value.is_empty(),
                    Value::String(value) => !value.is_empty(),
                    Value::Object(value) => !value.is_empty(),
                    Value::Null => false,
                    _ => true,
                })
            })
            .unwrap_or(Ok(false)),
        "SIMILARITY_GTE" | "MATCH" | "COSINE_DISTANCE_LTE" | "L2_DISTANCE_LTE" => Ok(false),
        other => Err(format!("unsupported condition operator '{other}'")),
    }
}

fn eval_condition_argument_as_bool(arg: &Value, source: &Value) -> Result<bool, String> {
    if is_condition_operation(arg) {
        eval_condition_expression(arg, source)
    } else {
        eval_condition_value(arg, source).map(|value| is_truthy(&value))
    }
}

fn eval_condition_argument_as_value(arg: &Value, source: &Value) -> Result<Value, String> {
    if is_condition_operation(arg) {
        if arg.get("op").and_then(Value::as_str) == Some("LENGTH") {
            let args = arg
                .get("arguments")
                .and_then(Value::as_array)
                .ok_or_else(|| "LENGTH condition missing arguments".to_string())?;
            eval_length_as_value(args, source)
        } else {
            eval_condition_expression(arg, source).map(Value::Bool)
        }
    } else {
        eval_condition_value(arg, source)
    }
}

fn eval_condition_value(value: &Value, source: &Value) -> Result<Value, String> {
    if value.get("type").and_then(Value::as_str) == Some("value") {
        if value.get("valueType").is_some() {
            return apply_mapping_value(value, source);
        }
        if let Some(inner) = value.get("value") {
            return apply_mapping_value(inner, source);
        }
    }
    apply_mapping_value(value, source)
}

fn eval_comparison(op: &str, args: &[Value], source: &Value) -> Result<bool, String> {
    if args.len() < 2 {
        return Ok(false);
    }
    let left = eval_condition_argument_as_value(&args[0], source)?;
    let right = eval_condition_argument_as_value(&args[1], source)?;
    let Some(left) = to_number(&left) else {
        return Ok(false);
    };
    let Some(right) = to_number(&right) else {
        return Ok(false);
    };
    Ok(match op {
        "GT" => left > right,
        "GTE" => left >= right,
        "LT" => left < right,
        "LTE" => left <= right,
        _ => false,
    })
}

fn eval_equality(op: &str, args: &[Value], source: &Value) -> Result<bool, String> {
    if args.len() < 2 {
        return Ok(false);
    }
    let left = eval_condition_argument_as_value(&args[0], source)?;
    let right = eval_condition_argument_as_value(&args[1], source)?;
    let equal = values_equal(&left, &right);
    Ok(if op == "NE" { !equal } else { equal })
}

fn eval_string_match(op: &str, args: &[Value], source: &Value) -> Result<bool, String> {
    if args.len() < 2 {
        return Ok(false);
    }
    let left = eval_condition_argument_as_value(&args[0], source)?;
    let right = eval_condition_argument_as_value(&args[1], source)?;
    let Some(left) = left.as_str() else {
        return Ok(false);
    };
    let Some(right) = right.as_str() else {
        return Ok(false);
    };
    Ok(if op == "STARTS_WITH" {
        left.starts_with(right)
    } else {
        left.ends_with(right)
    })
}

fn eval_array_match(op: &str, args: &[Value], source: &Value) -> Result<bool, String> {
    if args.len() < 2 {
        return Ok(false);
    }
    let left = eval_condition_argument_as_value(&args[0], source)?;
    let right = eval_condition_argument_as_value(&args[1], source)?;
    let matched = match op {
        "CONTAINS" => left
            .as_array()
            .is_some_and(|items| items.iter().any(|item| values_equal(item, &right))),
        "IN" | "NOT_IN" => right
            .as_array()
            .is_some_and(|items| items.iter().any(|item| values_equal(&left, item))),
        _ => false,
    };
    Ok(if op == "NOT_IN" { !matched } else { matched })
}

fn eval_length_as_value(args: &[Value], source: &Value) -> Result<Value, String> {
    let Some(arg) = args.first() else {
        return Ok(Value::Number(0.into()));
    };
    let value = eval_condition_argument_as_value(arg, source)?;
    let len = match &value {
        Value::String(value) => value.len() as i64,
        Value::Array(value) => value.len() as i64,
        Value::Object(value) => value.len() as i64,
        Value::Null => 0,
        _ => 1,
    };
    Ok(Value::Number(len.into()))
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
    conditions: Vec<ConditionWire>,
    #[serde(default)]
    filters: Vec<FilterWire>,
    #[serde(default)]
    switches: Vec<SwitchWire>,
    #[serde(default)]
    group_bys: Vec<GroupByWire>,
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

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConditionWire {
    id: u32,
    value: Value,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FilterWire {
    id: u32,
    step_id: String,
    #[serde(default)]
    name: Option<String>,
    value: Value,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SwitchWire {
    id: u32,
    step_id: String,
    #[serde(default)]
    name: Option<String>,
    value: Value,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GroupByWire {
    id: u32,
    step_id: String,
    #[serde(default)]
    name: Option<String>,
    value: Value,
}

#[derive(Debug, Clone)]
struct DirectJsonMapping {
    purpose: String,
    value: Value,
}

#[derive(Debug, Clone)]
struct DirectJsonFilter {
    step_id: String,
    name: Option<String>,
    value: Value,
}

#[derive(Debug, Clone)]
struct DirectJsonSwitch {
    step_id: String,
    name: Option<String>,
    value: Value,
}

#[derive(Debug, Clone)]
struct DirectJsonGroupBy {
    step_id: String,
    name: Option<String>,
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

    fn condition_manifest(condition_value: Value) -> Vec<u8> {
        serde_json::to_vec(&json!({
            "graph": {
                "conditions": [{
                    "id": 0,
                    "ownerId": "check",
                    "ownerType": "Conditional",
                    "purpose": "conditional.condition",
                    "value": condition_value
                }],
                "steps": []
            }
        }))
        .expect("manifest json")
    }

    fn filter_manifest(config: Value) -> Vec<u8> {
        serde_json::to_vec(&json!({
            "graph": {
                "filters": [{
                    "id": 0,
                    "stepId": "filter",
                    "name": "Filter Active Items",
                    "stepType": "Filter",
                    "purpose": "filter.config",
                    "value": config
                }],
                "steps": []
            }
        }))
        .expect("manifest json")
    }

    fn switch_manifest(config: Value) -> Vec<u8> {
        serde_json::to_vec(&json!({
            "graph": {
                "switches": [{
                    "id": 0,
                    "stepId": "switch",
                    "name": "Classify Status",
                    "stepType": "Switch",
                    "purpose": "switch.config",
                    "value": config
                }],
                "steps": []
            }
        }))
        .expect("manifest json")
    }

    fn group_by_manifest(config: Value) -> Vec<u8> {
        serde_json::to_vec(&json!({
            "graph": {
                "groupBys": [{
                    "id": 0,
                    "stepId": "group",
                    "name": "Group by Status",
                    "stepType": "GroupBy",
                    "purpose": "groupBy.config",
                    "value": config
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

    #[test]
    fn eval_condition_handles_equality_against_source() {
        let manifest = DirectJsonManifest::parse(&condition_manifest(json!({
            "type": "operation",
            "op": "EQ",
            "arguments": [
                { "valueType": "reference", "value": "data.flag" },
                { "valueType": "immediate", "value": true }
            ]
        })))
        .expect("manifest");
        let source = build_source(br#"{"flag":true}"#, b"{}", b"{}").expect("source");

        assert!(manifest.eval_condition(0, &source).expect("condition"));
    }

    #[test]
    fn eval_condition_handles_length_comparison() {
        let manifest = DirectJsonManifest::parse(&condition_manifest(json!({
            "type": "operation",
            "op": "GT",
            "arguments": [
                {
                    "type": "operation",
                    "op": "LENGTH",
                    "arguments": [
                        { "valueType": "reference", "value": "data.description" }
                    ]
                },
                { "valueType": "immediate", "value": 3 }
            ]
        })))
        .expect("manifest");
        let short = build_source(br#"{"description":"hey"}"#, b"{}", b"{}").expect("source");
        let long = build_source(br#"{"description":"hello"}"#, b"{}", b"{}").expect("source");

        assert!(!manifest.eval_condition(0, &short).expect("short"));
        assert!(manifest.eval_condition(0, &long).expect("long"));
    }

    #[test]
    fn eval_condition_handles_truthy_value_expression() {
        let manifest = DirectJsonManifest::parse(&condition_manifest(json!({
            "type": "value",
            "valueType": "reference",
            "value": "data.present"
        })))
        .expect("manifest");
        let source = build_source(br#"{"present":"yes"}"#, b"{}", b"{}").expect("source");

        assert!(manifest.eval_condition(0, &source).expect("condition"));
    }

    #[test]
    fn filter_keeps_items_matching_condition() {
        let manifest = DirectJsonManifest::parse(&filter_manifest(json!({
            "value": { "valueType": "reference", "value": "data.items" },
            "condition": {
                "type": "operation",
                "op": "EQ",
                "arguments": [
                    { "valueType": "reference", "value": "item.status" },
                    { "valueType": "immediate", "value": "active" }
                ]
            }
        })))
        .expect("manifest");
        let source = build_source(
            br#"{"items":[{"id":1,"status":"active"},{"id":2,"status":"failed"},{"id":3,"status":"active"}]}"#,
            b"{}",
            b"{}",
        )
        .expect("source");

        let steps = manifest.filter(0, &source).expect("steps context");
        let steps: Value = serde_json::from_slice(&steps).expect("steps json");
        let output = &steps["filter"]["outputs"];

        assert_eq!(output["count"], json!(2));
        assert_eq!(output["items"][0]["id"], json!(1));
        assert_eq!(output["items"][1]["id"], json!(3));
        assert_eq!(steps["filter"]["stepName"], json!("Filter Active Items"));
        assert_eq!(steps["filter"]["stepType"], json!("Filter"));
    }

    #[test]
    fn filter_supports_nested_boolean_conditions() {
        let manifest = DirectJsonManifest::parse(&filter_manifest(json!({
            "value": { "valueType": "reference", "value": "data.users" },
            "condition": {
                "type": "operation",
                "op": "OR",
                "arguments": [
                    {
                        "type": "operation",
                        "op": "AND",
                        "arguments": [
                            {
                                "type": "operation",
                                "op": "EQ",
                                "arguments": [
                                    { "valueType": "reference", "value": "item.status" },
                                    { "valueType": "immediate", "value": "active" }
                                ]
                            },
                            {
                                "type": "operation",
                                "op": "GT",
                                "arguments": [
                                    { "valueType": "reference", "value": "item.age" },
                                    { "valueType": "immediate", "value": 18 }
                                ]
                            }
                        ]
                    },
                    {
                        "type": "operation",
                        "op": "EQ",
                        "arguments": [
                            { "valueType": "reference", "value": "item.role" },
                            { "valueType": "immediate", "value": "admin" }
                        ]
                    }
                ]
            }
        })))
        .expect("manifest");
        let source = build_source(
            br#"{"users":[{"id":1,"status":"active","age":19,"role":"user"},{"id":2,"status":"active","age":17,"role":"user"},{"id":3,"status":"disabled","age":15,"role":"admin"}]}"#,
            b"{}",
            b"{}",
        )
        .expect("source");

        let steps = manifest.filter(0, &source).expect("steps context");
        let steps: Value = serde_json::from_slice(&steps).expect("steps json");
        let output = &steps["filter"]["outputs"];

        assert_eq!(output["count"], json!(2));
        assert_eq!(output["items"][0]["id"], json!(1));
        assert_eq!(output["items"][1]["id"], json!(3));
    }

    #[test]
    fn filter_treats_non_array_input_as_empty_array() {
        let manifest = DirectJsonManifest::parse(&filter_manifest(json!({
            "value": { "valueType": "reference", "value": "data.items" },
            "condition": {
                "type": "operation",
                "op": "EQ",
                "arguments": [
                    { "valueType": "reference", "value": "item.status" },
                    { "valueType": "immediate", "value": "active" }
                ]
            }
        })))
        .expect("manifest");
        let source =
            build_source(br#"{"items":{"status":"active"}}"#, b"{}", b"{}").expect("source");

        let steps = manifest.filter(0, &source).expect("steps context");
        let steps: Value = serde_json::from_slice(&steps).expect("steps json");
        let output = &steps["filter"]["outputs"];

        assert_eq!(output["count"], json!(0));
        assert_eq!(output["items"], json!([]));
    }

    #[test]
    fn value_switch_selects_first_matching_case() {
        let manifest = DirectJsonManifest::parse(&switch_manifest(json!({
            "value": { "valueType": "reference", "value": "data.status" },
            "cases": [
                {
                    "matchType": "EQ",
                    "match": "active",
                    "output": {
                        "bucket": { "valueType": "immediate", "value": "ready" },
                        "echo": { "valueType": "reference", "value": "data.status" }
                    }
                },
                {
                    "matchType": "EQ",
                    "match": ["active", "retry"],
                    "output": { "bucket": "array-match" }
                }
            ],
            "default": { "bucket": "other" }
        })))
        .expect("manifest");
        let source = build_source(br#"{"status":"active"}"#, b"{}", b"{}").expect("source");

        let steps = manifest.value_switch(0, &source).expect("steps context");
        let steps: Value = serde_json::from_slice(&steps).expect("steps json");
        let output = &steps["switch"]["outputs"];

        assert_eq!(output, &json!({ "bucket": "ready", "echo": "active" }));
        assert_eq!(steps["switch"]["stepName"], json!("Classify Status"));
        assert_eq!(steps["switch"]["stepType"], json!("Switch"));
    }

    #[test]
    fn value_switch_supports_array_match_and_default() {
        let manifest = DirectJsonManifest::parse(&switch_manifest(json!({
            "value": { "valueType": "reference", "value": "data.status" },
            "cases": [
                {
                    "matchType": "EQ",
                    "match": ["queued", "retry"],
                    "output": { "bucket": "pending" }
                }
            ],
            "default": { "bucket": "other" }
        })))
        .expect("manifest");
        let queued = build_source(br#"{"status":"queued"}"#, b"{}", b"{}").expect("source");
        let unknown = build_source(br#"{"status":"done"}"#, b"{}", b"{}").expect("source");

        let queued_steps = manifest.value_switch(0, &queued).expect("queued steps");
        let queued_steps: Value = serde_json::from_slice(&queued_steps).expect("queued json");
        assert_eq!(
            queued_steps["switch"]["outputs"],
            json!({ "bucket": "pending" })
        );

        let unknown_steps = manifest.value_switch(0, &unknown).expect("unknown steps");
        let unknown_steps: Value = serde_json::from_slice(&unknown_steps).expect("unknown json");
        assert_eq!(
            unknown_steps["switch"]["outputs"],
            json!({ "bucket": "other" })
        );
    }

    #[test]
    fn value_switch_supports_between_and_range_cases() {
        let manifest = DirectJsonManifest::parse(&switch_manifest(json!({
            "value": { "valueType": "reference", "value": "data.score" },
            "cases": [
                {
                    "matchType": "BETWEEN",
                    "match": [80, 100],
                    "output": { "grade": "high" }
                },
                {
                    "matchType": "RANGE",
                    "match": { "gte": 50, "lt": 80 },
                    "output": { "grade": "mid" }
                }
            ],
            "default": { "grade": "low" }
        })))
        .expect("manifest");

        for (input, expected) in [
            (br#"{"score":90}"#.as_slice(), json!({ "grade": "high" })),
            (br#"{"score":65}"#.as_slice(), json!({ "grade": "mid" })),
            (br#"{"score":20}"#.as_slice(), json!({ "grade": "low" })),
        ] {
            let source = build_source(input, b"{}", b"{}").expect("source");
            let steps = manifest.value_switch(0, &source).expect("steps context");
            let steps: Value = serde_json::from_slice(&steps).expect("steps json");
            assert_eq!(steps["switch"]["outputs"], expected);
        }
    }

    #[test]
    fn group_by_groups_items_by_simple_key() {
        let manifest = DirectJsonManifest::parse(&group_by_manifest(json!({
            "value": { "valueType": "reference", "value": "data.items" },
            "key": "status"
        })))
        .expect("manifest");
        let source = build_source(
            br#"{"items":[{"id":1,"status":"active"},{"id":2,"status":"inactive"},{"id":3,"status":"active"}]}"#,
            b"{}",
            b"{}",
        )
        .expect("source");

        let steps = manifest.group_by(0, &source).expect("steps context");
        let steps: Value = serde_json::from_slice(&steps).expect("steps json");
        let output = &steps["group"]["outputs"];

        assert_eq!(output["counts"], json!({ "active": 2, "inactive": 1 }));
        assert_eq!(output["total_groups"], json!(2));
        assert_eq!(output["groups"]["active"][0]["id"], json!(1));
        assert_eq!(output["groups"]["active"][1]["id"], json!(3));
        assert_eq!(steps["group"]["stepName"], json!("Group by Status"));
        assert_eq!(steps["group"]["stepType"], json!("GroupBy"));
    }

    #[test]
    fn group_by_handles_nested_keys_null_and_expected_keys() {
        let manifest = DirectJsonManifest::parse(&group_by_manifest(json!({
            "value": { "valueType": "reference", "value": "data.users" },
            "key": "profile.role",
            "expectedKeys": ["admin", "viewer", "missing"]
        })))
        .expect("manifest");
        let source = build_source(
            br#"{"users":[{"id":1,"profile":{"role":"admin"}},{"id":2,"profile":{"role":"viewer"}},{"id":3,"profile":{}}]}"#,
            b"{}",
            b"{}",
        )
        .expect("source");

        let steps = manifest.group_by(0, &source).expect("steps context");
        let steps: Value = serde_json::from_slice(&steps).expect("steps json");
        let output = &steps["group"]["outputs"];

        assert_eq!(
            output["counts"],
            json!({ "_null": 1, "admin": 1, "missing": 0, "viewer": 1 })
        );
        assert_eq!(output["groups"]["missing"], json!([]));
        assert_eq!(output["groups"]["_null"][0]["id"], json!(3));
        assert_eq!(output["total_groups"], json!(4));
    }

    #[test]
    fn group_by_treats_non_array_input_as_empty_array() {
        let manifest = DirectJsonManifest::parse(&group_by_manifest(json!({
            "value": { "valueType": "reference", "value": "data.items" },
            "key": "status",
            "expectedKeys": ["active"]
        })))
        .expect("manifest");
        let source =
            build_source(br#"{"items":{"status":"active"}}"#, b"{}", b"{}").expect("source");

        let steps = manifest.group_by(0, &source).expect("steps context");
        let steps: Value = serde_json::from_slice(&steps).expect("steps json");
        let output = &steps["group"]["outputs"];

        assert_eq!(output["counts"], json!({ "active": 0 }));
        assert_eq!(output["groups"], json!({ "active": [] }));
        assert_eq!(output["total_groups"], json!(1));
    }
}
