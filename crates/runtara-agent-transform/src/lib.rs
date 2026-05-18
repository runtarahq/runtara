//! Transform agent — JSON manipulation — as a WebAssembly component.
//!
//! Schema matches the legacy `runtara-agents/src/agents/transform.rs` agent so
//! A/B parity tests can compare results byte-for-byte.
//!
//! Capabilities (16):
//! - `extract`            — extract property values from an array of objects
//! - `get-value-by-path`  — get a value from an object by property path
//! - `set-value-by-path`  — set a value in an object at a property path
//! - `filter-non-values`  — filter out null/empty/blank/zero values from an array
//! - `select-first`       — select the first truthy value from an array
//! - `coalesce`           — return the first non-null value from an array
//! - `from-json-string`   — parse a JSON string into a value
//! - `to-json-string`     — serialize a value to a JSON string
//! - `filter`             — filter an array by property value conditions
//! - `sort`               — sort an array, optionally by property path
//! - `map-fields`         — map fields from a source to a target object
//! - `group-by`           — group array items by a property key
//! - `append`             — append an item to an array
//! - `flat-map`           — extract nested arrays and flatten into one
//! - `array-length`       — get the length/size of an array, string, or object
//! - `ensure-array`       — wrap a non-array value in an array

#![cfg(target_arch = "wasm32")]

#[allow(warnings)]
mod bindings;

use bindings::exports::runtara::agent::capabilities::{
    CapabilityInfo, ConnectionInfo, ErrorInfo, Guest, ModuleInfo,
};
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;

// =============================================================================
// Component plumbing
// =============================================================================

struct Component;

impl Guest for Component {
    fn get_module_info() -> ModuleInfo {
        ModuleInfo {
            id: "transform".into(),
            display_name: "Transform".into(),
            description:
                "JSON data manipulation: extract, filter, sort, group, map, coalesce, and more."
                    .into(),
            has_side_effects: false,
            supports_connections: false,
            integration_ids: vec![],
            secure: false,
        }
    }

    fn list_capabilities() -> Vec<CapabilityInfo> {
        vec![
            cap(
                "extract",
                "Extract Property",
                "Extract property values from an array of objects based on a property path",
                EXTRACT_INPUT_SCHEMA,
                EXTRACT_OUTPUT_SCHEMA,
            ),
            cap(
                "get-value-by-path",
                "Get Value By Path",
                "Get a value from an object using a JSONPath-like property path",
                GET_VALUE_BY_PATH_INPUT_SCHEMA,
                VALUE_OUTPUT_SCHEMA,
            ),
            cap(
                "set-value-by-path",
                "Set Value By Path",
                "Set a value in an object at a specified JSONPath-like property path",
                SET_VALUE_BY_PATH_INPUT_SCHEMA,
                VALUE_OUTPUT_SCHEMA,
            ),
            cap(
                "filter-non-values",
                "Filter Non-Values",
                "Filter an array removing elements with null, empty, blank, or zero values",
                FILTER_NON_VALUES_INPUT_SCHEMA,
                FILTER_OUTPUT_SCHEMA,
            ),
            cap(
                "select-first",
                "Select First",
                "Select the first truthy value from an array (skips null, empty, zero, false)",
                SELECT_FIRST_INPUT_SCHEMA,
                VALUE_OUTPUT_SCHEMA,
            ),
            cap(
                "coalesce",
                "Coalesce",
                "Return the first non-null value from an array of values",
                COALESCE_INPUT_SCHEMA,
                VALUE_OUTPUT_SCHEMA,
            ),
            cap(
                "from-json-string",
                "From JSON String",
                "Parse a JSON string into a structured value",
                FROM_JSON_STRING_INPUT_SCHEMA,
                VALUE_OUTPUT_SCHEMA,
            ),
            cap(
                "to-json-string",
                "To JSON String",
                "Convert a value to a JSON string",
                TO_JSON_STRING_INPUT_SCHEMA,
                TO_JSON_STRING_OUTPUT_SCHEMA,
            ),
            cap(
                "filter",
                "Filter Array",
                "Filter an array based on property values matching or excluding specified values",
                FILTER_INPUT_SCHEMA,
                FILTER_OUTPUT_SCHEMA,
            ),
            cap(
                "sort",
                "Sort Array",
                "Sort an array of items, optionally by a property path",
                SORT_INPUT_SCHEMA,
                SORT_OUTPUT_SCHEMA,
            ),
            cap(
                "map-fields",
                "Map Fields",
                "Map fields from a source object to a target object using field path mappings",
                MAP_FIELDS_INPUT_SCHEMA,
                MAP_FIELDS_OUTPUT_SCHEMA,
            ),
            cap(
                "group-by",
                "Group By",
                "Group array items by a property key, returning either a map or array of groups",
                GROUP_BY_INPUT_SCHEMA,
                GROUP_BY_OUTPUT_SCHEMA,
            ),
            cap(
                "append",
                "Append",
                "Append an item to the end of an array",
                APPEND_INPUT_SCHEMA,
                APPEND_OUTPUT_SCHEMA,
            ),
            cap(
                "flat-map",
                "Flat Map",
                "Extract nested arrays from each item by property path and flatten into a single array",
                FLAT_MAP_INPUT_SCHEMA,
                FLAT_MAP_OUTPUT_SCHEMA,
            ),
            cap(
                "array-length",
                "Array Length",
                "Get the length of an array, string, or number of keys in an object",
                ARRAY_LENGTH_INPUT_SCHEMA,
                ARRAY_LENGTH_OUTPUT_SCHEMA,
            ),
            cap(
                "ensure-array",
                "Ensure Array",
                "Ensure a value is an array. Arrays pass through unchanged, null becomes empty array, other values are wrapped in a single-element array.",
                ENSURE_ARRAY_INPUT_SCHEMA,
                ENSURE_ARRAY_OUTPUT_SCHEMA,
            ),
        ]
    }

    fn invoke(
        capability_id: String,
        input: String,
        _connection: Option<ConnectionInfo>,
    ) -> Result<String, ErrorInfo> {
        match capability_id.as_str() {
            "extract" => invoke_extract(&input),
            "get-value-by-path" => invoke_get_value_by_path(&input),
            "set-value-by-path" => invoke_set_value_by_path(&input),
            "filter-non-values" => invoke_filter_non_values(&input),
            "select-first" => invoke_select_first(&input),
            "coalesce" => invoke_coalesce(&input),
            "from-json-string" => invoke_from_json_string(&input),
            "to-json-string" => invoke_to_json_string(&input),
            "filter" => invoke_filter(&input),
            "sort" => invoke_sort(&input),
            "map-fields" => invoke_map_fields(&input),
            "group-by" => invoke_group_by(&input),
            "append" => invoke_append(&input),
            "flat-map" => invoke_flat_map(&input),
            "array-length" => invoke_array_length(&input),
            "ensure-array" => invoke_ensure_array(&input),
            other => Err(ErrorInfo {
                code: "UNKNOWN_CAPABILITY".into(),
                message: format!("transform agent has no capability `{other}`"),
                category: "permanent".into(),
                severity: "error".into(),
                retryable: false,
                retry_after_ms: None,
                attributes: None,
            }),
        }
    }
}

// =============================================================================
// Input types (mirror runtara-agents/src/agents/transform.rs)
// =============================================================================

/// Custom deserializer that treats null as an empty Vec
fn deserialize_value_or_empty_vec<'de, D>(deserializer: D) -> Result<Vec<Value>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let opt: Option<Vec<Value>> = Option::deserialize(deserializer)?;
    Ok(opt.unwrap_or_default())
}

#[derive(Deserialize)]
struct ExtractInput {
    pub value: Vec<Value>,
    pub property_path: String,
}

#[derive(Deserialize)]
struct GetValueByPathInput {
    pub value: Option<Value>,
    pub property_path: Option<String>,
}

#[derive(Deserialize)]
struct SetValueByPathInput {
    pub target: Option<Value>,
    pub property_path: Option<String>,
    pub value: Option<Value>,
}

#[derive(Deserialize)]
struct FilterNoValueInput {
    pub value: Vec<Value>,
    #[serde(default)]
    pub property_path: Option<String>,
    #[serde(default)]
    pub filter_empty_strings: bool,
    #[serde(default)]
    pub filter_null_values: bool,
    #[serde(default)]
    pub filter_blank_strings: bool,
    #[serde(default)]
    pub filter_zero_values: bool,
}

#[derive(Deserialize)]
struct SelectFirstInput {
    pub value: Option<Vec<Value>>,
}

#[derive(Deserialize)]
struct CoalesceInput {
    pub values: Vec<Value>,
    #[serde(default)]
    pub treat_empty_string_as_null: bool,
    #[serde(default)]
    pub treat_zero_as_null: bool,
}

#[derive(Deserialize)]
struct FromJsonStringInput {
    pub value: Option<String>,
}

#[derive(Deserialize)]
struct ToJsonStringInput {
    pub value: Value,
}

#[derive(Debug, Deserialize, Clone, PartialEq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum MatchCondition {
    Includes,
    Excludes,
    StartsWith,
    EndsWith,
    Contains,
}

#[derive(Deserialize)]
struct FilterInput {
    #[serde(default, deserialize_with = "deserialize_value_or_empty_vec")]
    pub value: Vec<Value>,
    pub property_path: String,
    pub match_values: Value,
    pub match_condition: MatchCondition,
}

fn default_ascending() -> bool {
    true
}

#[derive(Deserialize)]
struct SortInput {
    pub value: Vec<Value>,
    pub property_path: Option<String>,
    #[serde(default = "default_ascending")]
    pub ascending: bool,
}

#[derive(Deserialize)]
struct MapFieldsInput {
    pub source_data: Value,
    pub mappings: HashMap<String, String>,
}

#[derive(Deserialize)]
struct GroupByInput {
    pub value: Value,
    pub key: String,
    #[serde(default)]
    pub as_map: bool,
}

#[derive(Deserialize)]
struct AppendInput {
    pub array: Vec<Value>,
    pub item: Value,
}

#[derive(Deserialize)]
struct FlatMapInput {
    pub value: Vec<Value>,
    pub property_path: String,
}

#[derive(Deserialize)]
struct ArrayLengthInput {
    pub value: Value,
}

#[derive(Deserialize)]
struct EnsureArrayInput {
    pub value: Value,
}

// =============================================================================
// Capability implementations
// =============================================================================

fn invoke_extract(input_json: &str) -> Result<String, ErrorInfo> {
    let input: ExtractInput = serde_json::from_str(input_json).map_err(bad_input)?;

    if input.value.is_empty() || input.property_path.is_empty() {
        let count = input.value.len();
        return to_json(serde_json::json!({
            "values": input.value,
            "count": count
        }));
    }

    let values: Vec<Value> = input
        .value
        .iter()
        .map(|item| get_property_value(item, &input.property_path))
        .collect();
    let count = values.len();
    to_json(serde_json::json!({ "values": values, "count": count }))
}

fn invoke_get_value_by_path(input_json: &str) -> Result<String, ErrorInfo> {
    let input: GetValueByPathInput = serde_json::from_str(input_json).map_err(bad_input)?;
    let result = match (input.value, input.property_path) {
        (Some(value), Some(path)) if !path.is_empty() => get_property_value(&value, &path),
        _ => Value::Null,
    };
    to_json(result)
}

fn invoke_set_value_by_path(input_json: &str) -> Result<String, ErrorInfo> {
    let input: SetValueByPathInput = serde_json::from_str(input_json).map_err(bad_input)?;
    let result = match (input.target, input.property_path, input.value) {
        (Some(target), Some(path), value) if !path.is_empty() => {
            set_property_value(target, &path, value.unwrap_or(Value::Null))
        }
        (Some(target), _, _) => target,
        _ => Value::Null,
    };
    to_json(result)
}

fn invoke_filter_non_values(input_json: &str) -> Result<String, ErrorInfo> {
    let input: FilterNoValueInput = serde_json::from_str(input_json).map_err(bad_input)?;
    let original_count = input.value.len();
    if input.value.is_empty() {
        return to_json(serde_json::json!({
            "items": [],
            "count": 0,
            "removed_count": 0
        }));
    }

    let items: Vec<Value> = input
        .value
        .into_iter()
        .filter(|item| {
            let property_value = if let Some(ref path) = input.property_path {
                get_property_value(item, path)
            } else {
                item.clone()
            };

            if input.filter_null_values && property_value.is_null() {
                return false;
            }

            if input.filter_empty_strings {
                if let Some(s) = property_value.as_str() {
                    if s.is_empty() {
                        return false;
                    }
                }
            }

            if input.filter_blank_strings {
                if let Some(s) = property_value.as_str() {
                    if s.trim().is_empty() {
                        return false;
                    }
                }
            }

            if input.filter_zero_values {
                if let Some(n) = property_value.as_f64() {
                    if n == 0.0 {
                        return false;
                    }
                }
                if let Some(s) = property_value.as_str() {
                    if s == "0" {
                        return false;
                    }
                }
            }

            true
        })
        .collect();

    let count = items.len();
    to_json(serde_json::json!({
        "items": items,
        "count": count,
        "removed_count": original_count - count
    }))
}

fn invoke_select_first(input_json: &str) -> Result<String, ErrorInfo> {
    let input: SelectFirstInput = serde_json::from_str(input_json).map_err(bad_input)?;
    let Some(values) = input.value else {
        return to_json(Value::Null);
    };

    if values.is_empty() {
        return to_json(Value::Null);
    }

    for item in values {
        if item.is_null() {
            continue;
        }
        if let Some(s) = item.as_str() {
            if s.is_empty() || s.trim().is_empty() {
                continue;
            }
        }
        if let Some(n) = item.as_f64() {
            if n == 0.0 {
                continue;
            }
        }
        if let Some(b) = item.as_bool() {
            if !b {
                continue;
            }
        }
        return to_json(item);
    }

    to_json(Value::Null)
}

fn invoke_coalesce(input_json: &str) -> Result<String, ErrorInfo> {
    let input: CoalesceInput = serde_json::from_str(input_json).map_err(bad_input)?;
    for value in input.values {
        if value.is_null() {
            continue;
        }
        if input.treat_empty_string_as_null && value.as_str().is_some_and(|s| s.is_empty()) {
            continue;
        }
        if input.treat_zero_as_null && value.as_f64().is_some_and(|n| n == 0.0) {
            continue;
        }
        return to_json(value);
    }
    to_json(Value::Null)
}

fn invoke_from_json_string(input_json: &str) -> Result<String, ErrorInfo> {
    let input: FromJsonStringInput = serde_json::from_str(input_json).map_err(bad_input)?;
    match input.value {
        Some(json_str) if !json_str.is_empty() => {
            let parsed: Value = serde_json::from_str(&json_str).map_err(|e| ErrorInfo {
                code: "TRANSFORM_JSON_PARSE_ERROR".into(),
                message: format!("Failed to parse JSON: {}", e),
                category: "permanent".into(),
                severity: "error".into(),
                retryable: false,
                retry_after_ms: None,
                attributes: None,
            })?;
            to_json(parsed)
        }
        _ => to_json(Value::Null),
    }
}

fn invoke_to_json_string(input_json: &str) -> Result<String, ErrorInfo> {
    let input: ToJsonStringInput = serde_json::from_str(input_json).map_err(bad_input)?;
    let json = serde_json::to_string(&input.value).map_err(|e| ErrorInfo {
        code: "TRANSFORM_JSON_SERIALIZE_ERROR".into(),
        message: format!("Failed to serialize JSON: {}", e),
        category: "permanent".into(),
        severity: "error".into(),
        retryable: false,
        retry_after_ms: None,
        attributes: None,
    })?;
    let length = json.len();
    to_json(serde_json::json!({ "json": json, "length": length }))
}

fn invoke_filter(input_json: &str) -> Result<String, ErrorInfo> {
    let input: FilterInput = serde_json::from_str(input_json).map_err(bad_input)?;
    let original_count = input.value.len();
    if input.value.is_empty() {
        return to_json(serde_json::json!({
            "items": [],
            "count": 0,
            "removed_count": 0
        }));
    }

    let match_values_list: Vec<Value> = match input.match_values {
        Value::Array(arr) => arr,
        other => vec![other],
    };

    let filter_actual_values = input.property_path.is_empty() || input.property_path == "$";

    let filtered: Vec<Value> = input
        .value
        .into_iter()
        .filter(|item| {
            let property_value = if filter_actual_values {
                item.clone()
            } else {
                get_property_value(item, &input.property_path)
            };

            match input.match_condition {
                MatchCondition::Includes => {
                    matches_filter_values(&property_value, &match_values_list)
                }
                MatchCondition::Excludes => {
                    !matches_filter_values(&property_value, &match_values_list)
                }
                MatchCondition::StartsWith => {
                    matches_string_filter(&property_value, &match_values_list, |s, v| {
                        s.starts_with(v)
                    })
                }
                MatchCondition::EndsWith => {
                    matches_string_filter(&property_value, &match_values_list, |s, v| {
                        s.ends_with(v)
                    })
                }
                MatchCondition::Contains => {
                    matches_string_filter(&property_value, &match_values_list, |s, v| s.contains(v))
                }
            }
        })
        .collect();

    let count = filtered.len();
    to_json(serde_json::json!({
        "items": filtered,
        "count": count,
        "removed_count": original_count - count
    }))
}

fn invoke_sort(input_json: &str) -> Result<String, ErrorInfo> {
    let input: SortInput = serde_json::from_str(input_json).map_err(bad_input)?;
    if input.value.is_empty() {
        return to_json(serde_json::json!({ "items": [], "count": 0 }));
    }

    let mut sorted_list = input.value;
    sorted_list.sort_by(|a, b| {
        let value1 = if let Some(ref path) = input.property_path {
            get_property_value(a, path)
        } else {
            a.clone()
        };
        let value2 = if let Some(ref path) = input.property_path {
            get_property_value(b, path)
        } else {
            b.clone()
        };
        let cmp = compare_values(&value1, &value2);
        if input.ascending { cmp } else { cmp.reverse() }
    });

    let count = sorted_list.len();
    to_json(serde_json::json!({ "items": sorted_list, "count": count }))
}

fn invoke_map_fields(input_json: &str) -> Result<String, ErrorInfo> {
    let input: MapFieldsInput = serde_json::from_str(input_json).map_err(bad_input)?;
    let mut result: HashMap<String, Value> = HashMap::new();

    for (source_field, target_field) in input.mappings {
        let value = get_property_value(&input.source_data, &source_field);
        if !value.is_null() {
            result.insert(target_field, value);
        }
    }

    let field_count = result.len();
    to_json(serde_json::json!({ "result": result, "field_count": field_count }))
}

fn invoke_group_by(input_json: &str) -> Result<String, ErrorInfo> {
    let input: GroupByInput = serde_json::from_str(input_json).map_err(bad_input)?;

    let collection = match &input.value {
        Value::Array(arr) => arr,
        Value::Null => {
            return Err(ErrorInfo {
                code: "TRANSFORM_INVALID_INPUT".into(),
                message: "Unsupported value. Expected array or collection.".into(),
                category: "permanent".into(),
                severity: "error".into(),
                retryable: false,
                retry_after_ms: None,
                attributes: Some(r#"{"received_type":"null"}"#.into()),
            });
        }
        other => {
            let type_name = match other {
                Value::Object(_) => "object",
                Value::String(_) => "string",
                Value::Number(_) => "number",
                Value::Bool(_) => "boolean",
                _ => "unknown",
            };
            return Err(ErrorInfo {
                code: "TRANSFORM_INVALID_INPUT".into(),
                message: "Unsupported value. Expected array or collection.".into(),
                category: "permanent".into(),
                severity: "error".into(),
                retryable: false,
                retry_after_ms: None,
                attributes: Some(format!(r#"{{"received_type":"{type_name}"}}"#)),
            });
        }
    };

    if collection.is_empty() {
        let groups = if input.as_map {
            Value::Object(serde_json::Map::new())
        } else {
            Value::Array(vec![])
        };
        return to_json(serde_json::json!({ "groups": groups, "group_count": 0 }));
    }

    let json_path = if input.key.starts_with("$.") {
        input.key.clone()
    } else {
        format!("$.{}", input.key)
    };

    let mut grouped: HashMap<String, Vec<Value>> = HashMap::new();

    for item in collection {
        let key_value = get_property_value(item, &json_path);
        if key_value.is_null() {
            continue;
        }
        let key_str = match &key_value {
            Value::String(s) => s.clone(),
            Value::Number(n) => n.to_string(),
            Value::Bool(b) => b.to_string(),
            _ => serde_json::to_string(&key_value).map_err(|e| ErrorInfo {
                code: "TRANSFORM_KEY_SERIALIZE_ERROR".into(),
                message: format!("Failed to serialize group key to string: {}", e),
                category: "permanent".into(),
                severity: "error".into(),
                retryable: false,
                retry_after_ms: None,
                attributes: None,
            })?,
        };
        grouped.entry(key_str).or_default().push(item.clone());
    }

    let group_count = grouped.len();
    let groups = if input.as_map {
        let mut map = serde_json::Map::new();
        for (key, values) in grouped {
            map.insert(key, Value::Array(values));
        }
        Value::Object(map)
    } else {
        let arrays: Vec<Value> = grouped.into_values().map(Value::Array).collect();
        Value::Array(arrays)
    };

    to_json(serde_json::json!({ "groups": groups, "group_count": group_count }))
}

fn invoke_append(input_json: &str) -> Result<String, ErrorInfo> {
    let input: AppendInput = serde_json::from_str(input_json).map_err(bad_input)?;
    let mut array = input.array;
    array.push(input.item);
    let length = array.len();
    to_json(serde_json::json!({ "array": array, "length": length }))
}

fn invoke_flat_map(input_json: &str) -> Result<String, ErrorInfo> {
    let input: FlatMapInput = serde_json::from_str(input_json).map_err(bad_input)?;
    if input.value.is_empty() || input.property_path.is_empty() {
        return to_json(serde_json::json!({ "items": [], "count": 0 }));
    }

    let mut items = Vec::new();
    for item in input.value {
        let nested = get_property_value(&item, &input.property_path);
        if let Some(arr) = nested.as_array() {
            items.extend(arr.iter().cloned());
        }
    }

    let count = items.len();
    to_json(serde_json::json!({ "items": items, "count": count }))
}

fn invoke_array_length(input_json: &str) -> Result<String, ErrorInfo> {
    let input: ArrayLengthInput = serde_json::from_str(input_json).map_err(bad_input)?;
    let (length, is_array) = match &input.value {
        Value::Array(arr) => (arr.len(), true),
        Value::String(s) => (s.len(), false),
        Value::Object(obj) => (obj.len(), false),
        Value::Null => (0, false),
        _ => (0, false),
    };
    to_json(serde_json::json!({ "length": length, "is_array": is_array }))
}

fn invoke_ensure_array(input_json: &str) -> Result<String, ErrorInfo> {
    let input: EnsureArrayInput = serde_json::from_str(input_json).map_err(bad_input)?;
    let (items, was_array) = match input.value {
        Value::Array(arr) => (arr, true),
        Value::Null => (vec![], false),
        other => (vec![other], false),
    };
    let count = items.len();
    to_json(serde_json::json!({ "items": items, "count": count, "was_array": was_array }))
}

// =============================================================================
// Helper functions (mirror runtara-agents/src/agents/transform.rs)
// =============================================================================

fn get_property_value(obj: &Value, property_path: &str) -> Value {
    if property_path.is_empty() {
        return obj.clone();
    }

    let path = property_path.strip_prefix("$.").unwrap_or(property_path);
    let parts: Vec<&str> = path.split('.').collect();

    let mut current = obj;
    for part in parts {
        current = match current {
            Value::Object(map) => map.get(part).unwrap_or(&Value::Null),
            Value::Array(arr) => {
                if let Ok(index) = part.parse::<usize>() {
                    arr.get(index).unwrap_or(&Value::Null)
                } else {
                    &Value::Null
                }
            }
            _ => &Value::Null,
        };

        if current.is_null() {
            return Value::Null;
        }
    }

    current.clone()
}

fn set_property_value(obj: Value, property_path: &str, value: Value) -> Value {
    if property_path.is_empty() {
        return obj;
    }

    let path = property_path.strip_prefix("$.").unwrap_or(property_path);
    let parts: Vec<&str> = path.split('.').collect();

    if let Value::Object(mut map) = obj {
        if parts.len() == 1 {
            map.insert(parts[0].to_string(), value);
            return Value::Object(map);
        } else {
            set_nested_value(&mut map, &parts, value);
            return Value::Object(map);
        }
    }

    obj
}

fn set_nested_value(map: &mut serde_json::Map<String, Value>, parts: &[&str], value: Value) {
    if parts.is_empty() {
        return;
    }

    if parts.len() == 1 {
        map.insert(parts[0].to_string(), value);
        return;
    }

    let key = parts[0];
    let rest = &parts[1..];

    let next = map
        .entry(key.to_string())
        .or_insert_with(|| Value::Object(serde_json::Map::new()));

    if let Value::Object(nested_map) = next {
        set_nested_value(nested_map, rest, value);
    }
}

fn matches_filter_values(property_value: &Value, filter_values: &[Value]) -> bool {
    if property_value.is_null() {
        return false;
    }
    if let Some(arr) = property_value.as_array() {
        for element in arr {
            if filter_values.contains(element) {
                return true;
            }
        }
        return false;
    }
    filter_values.contains(property_value)
}

fn matches_string_filter<F>(property_value: &Value, filter_values: &[Value], compare: F) -> bool
where
    F: Fn(&str, &str) -> bool,
{
    let property_str = match property_value {
        Value::String(s) => s.as_str(),
        _ => return false,
    };

    for filter_value in filter_values {
        let filter_str = match filter_value {
            Value::String(s) => s.as_str(),
            _ => continue,
        };
        if compare(property_str, filter_str) {
            return true;
        }
    }
    false
}

fn compare_values(a: &Value, b: &Value) -> std::cmp::Ordering {
    use std::cmp::Ordering;

    match (a, b) {
        (Value::Null, Value::Null) => Ordering::Equal,
        (Value::Null, _) => Ordering::Greater,
        (_, Value::Null) => Ordering::Less,
        (Value::Number(n1), Value::Number(n2)) => {
            let f1 = n1.as_f64().unwrap_or(0.0);
            let f2 = n2.as_f64().unwrap_or(0.0);
            f1.partial_cmp(&f2).unwrap_or(Ordering::Equal)
        }
        (Value::String(s1), Value::String(s2)) => s1.cmp(s2),
        (Value::Bool(b1), Value::Bool(b2)) => b1.cmp(b2),
        _ => a.to_string().cmp(&b.to_string()),
    }
}

// =============================================================================
// Serialization helpers
// =============================================================================

fn to_json<T: serde::Serialize>(v: T) -> Result<String, ErrorInfo> {
    serde_json::to_string(&v).map_err(|e| ErrorInfo {
        code: "SERIALIZATION_ERROR".into(),
        message: e.to_string(),
        category: "permanent".into(),
        severity: "error".into(),
        retryable: false,
        retry_after_ms: None,
        attributes: None,
    })
}

fn bad_input(e: serde_json::Error) -> ErrorInfo {
    ErrorInfo {
        code: "INPUT_DESERIALIZATION_ERROR".into(),
        message: e.to_string(),
        category: "permanent".into(),
        severity: "error".into(),
        retryable: false,
        retry_after_ms: None,
        attributes: None,
    }
}

fn cap(
    id: &str,
    display_name: &str,
    description: &str,
    input_schema: &str,
    output_schema: &str,
) -> CapabilityInfo {
    CapabilityInfo {
        id: id.into(),
        function_name: id.into(),
        display_name: Some(display_name.into()),
        description: Some(description.into()),
        has_side_effects: false,
        is_idempotent: true,
        rate_limited: false,
        tags: vec!["transform".into()],
        input_schema: input_schema.into(),
        output_schema: output_schema.into(),
        known_errors: vec![],
        compensation_hint: None,
    }
}

// =============================================================================
// JSON Schemas published via list-capabilities()
// =============================================================================

const EXTRACT_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["value", "property_path"],
    "properties": {
        "value": {
            "type": "array",
            "description": "The array of objects to extract property values from"
        },
        "property_path": {
            "type": "string",
            "description": "The property path to extract from each item (JSONPath syntax)"
        }
    }
}"#;

const EXTRACT_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "values": { "type": "array", "description": "Array of extracted property values" },
        "count":  { "type": "integer", "description": "Number of values extracted" }
    }
}"#;

const GET_VALUE_BY_PATH_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "value":         { "description": "The object to extract a property value from" },
        "property_path": { "type": "string", "description": "The property path to extract (JSONPath syntax)" }
    }
}"#;

const SET_VALUE_BY_PATH_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "target":        { "description": "The object to set a property value in" },
        "property_path": { "type": "string", "description": "The property path to set (JSONPath syntax)" },
        "value":         { "description": "The value to set at the property path" }
    }
}"#;

const VALUE_OUTPUT_SCHEMA: &str = r#"{ "description": "A JSON value (any type)" }"#;

const FILTER_NON_VALUES_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["value"],
    "properties": {
        "value":               { "type": "array", "description": "The array of items to filter" },
        "property_path":       { "type": "string", "description": "The property path to check in each item" },
        "filter_empty_strings":{ "type": "boolean", "default": false },
        "filter_null_values":  { "type": "boolean", "default": false },
        "filter_blank_strings":{ "type": "boolean", "default": false },
        "filter_zero_values":  { "type": "boolean", "default": false }
    }
}"#;

const FILTER_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "items":        { "type": "array" },
        "count":        { "type": "integer" },
        "removed_count":{ "type": "integer" }
    }
}"#;

const SELECT_FIRST_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "value": { "type": "array", "description": "The array to select the first truthy value from" }
    }
}"#;

const COALESCE_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["values"],
    "properties": {
        "values":                  { "type": "array", "description": "Array of values to check" },
        "treat_empty_string_as_null": { "type": "boolean", "default": false },
        "treat_zero_as_null":         { "type": "boolean", "default": false }
    }
}"#;

const FROM_JSON_STRING_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "value": { "type": "string", "description": "The JSON string to parse" }
    }
}"#;

const TO_JSON_STRING_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["value"],
    "properties": {
        "value": { "description": "The value to serialize to a JSON string" }
    }
}"#;

const TO_JSON_STRING_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "json":   { "type": "string", "description": "The serialized JSON string" },
        "length": { "type": "integer" }
    }
}"#;

const FILTER_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["property_path", "match_values", "match_condition"],
    "properties": {
        "value":           { "type": "array", "description": "The array of items to filter" },
        "property_path":   { "type": "string", "description": "Property path to check; use \"$\" or \"\" to filter values directly" },
        "match_values":    { "description": "Value(s) to compare against (single value or array)" },
        "match_condition": {
            "type": "string",
            "enum": ["INCLUDES", "EXCLUDES", "STARTS_WITH", "ENDS_WITH", "CONTAINS"]
        }
    }
}"#;

const SORT_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["value"],
    "properties": {
        "value":         { "type": "array" },
        "property_path": { "type": "string", "description": "Property path to sort by (optional)" },
        "ascending":     { "type": "boolean", "default": true }
    }
}"#;

const SORT_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "items": { "type": "array" },
        "count": { "type": "integer" }
    }
}"#;

const MAP_FIELDS_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["source_data", "mappings"],
    "properties": {
        "source_data": { "type": "object", "description": "The source object containing data to map" },
        "mappings":    { "type": "object", "description": "Map of source field paths to target field names", "additionalProperties": { "type": "string" } }
    }
}"#;

const MAP_FIELDS_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "result":      { "type": "object" },
        "field_count": { "type": "integer" }
    }
}"#;

const GROUP_BY_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["value", "key"],
    "properties": {
        "value":  { "type": "array", "description": "The array of items to group" },
        "key":    { "type": "string", "description": "Property path to group by (JSONPath syntax)" },
        "as_map": { "type": "boolean", "default": false, "description": "Return as map (key -> items) instead of array of arrays" }
    }
}"#;

const GROUP_BY_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "groups":      { "description": "Grouped items — map or array depending on as_map" },
        "group_count": { "type": "integer" }
    }
}"#;

const APPEND_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["array", "item"],
    "properties": {
        "array": { "type": "array", "description": "The array to append to" },
        "item":  { "description": "The item to append" }
    }
}"#;

const APPEND_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "array":  { "type": "array" },
        "length": { "type": "integer" }
    }
}"#;

const FLAT_MAP_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["value", "property_path"],
    "properties": {
        "value":         { "type": "array", "description": "The array of objects to flat map" },
        "property_path": { "type": "string", "description": "Property path to the nested array in each item" }
    }
}"#;

const FLAT_MAP_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "items": { "type": "array" },
        "count": { "type": "integer" }
    }
}"#;

const ARRAY_LENGTH_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["value"],
    "properties": {
        "value": { "description": "Array, string, or object to get the length/size of" }
    }
}"#;

const ARRAY_LENGTH_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "length":   { "type": "integer" },
        "is_array": { "type": "boolean" }
    }
}"#;

const ENSURE_ARRAY_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["value"],
    "properties": {
        "value": { "description": "Value to wrap in an array if not already an array" }
    }
}"#;

const ENSURE_ARRAY_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "items":     { "type": "array" },
        "count":     { "type": "integer" },
        "was_array": { "type": "boolean" }
    }
}"#;

bindings::export!(Component with_types_in bindings);
