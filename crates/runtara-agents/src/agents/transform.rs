// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Transform agents for workflow execution
//!
//! This module provides data transformation operations that can be used in workflows:
//! - Property extraction and manipulation (get/set values by path)
//! - Array filtering and sorting
//! - Array flattening (flat-map)
//! - JSON parsing and serialization
//! - Field mapping between objects
//!
//! All operations accept Rust data structures directly (no CloudEvents wrapper)

use crate::types::AgentError;
use runtara_agent_macro::{CapabilityInput, CapabilityOutput, capability};
use runtara_dsl::agent_meta::EnumVariants;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use strum::VariantNames;

/// Custom deserializer that treats null as an empty Vec
fn deserialize_value_or_empty_vec<'de, D>(deserializer: D) -> Result<Vec<Value>, D::Error>
where
    D: Deserializer<'de>,
{
    let opt: Option<Vec<Value>> = Option::deserialize(deserializer)?;
    Ok(opt.unwrap_or_default())
}

// ============================================================================
// Input/Output Types
// ============================================================================

/// Input for extracting property values from an array
#[derive(Debug, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Extract Property Input")]
pub struct ExtractInput {
    /// The array of objects to extract from
    #[field(
        display_name = "Input Array",
        description = "The array of objects to extract property values from",
        example = r#"[{"name": "Alice", "age": 30}, {"name": "Bob", "age": 25}]"#
    )]
    pub value: Vec<Value>,

    /// JSONPath to the property to extract
    #[field(
        display_name = "Property Path",
        description = "The property path to extract from each item (JSONPath syntax)",
        example = "name"
    )]
    pub property_path: String,
}

/// Input for getting a value from an object by property path
#[derive(Debug, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Get Value Input")]
pub struct GetValueByPathInput {
    /// The object to extract from
    #[field(
        display_name = "Input Value",
        description = "The object to extract a property value from",
        example = r#"{"user": {"name": "Alice", "age": 30}}"#
    )]
    pub value: Option<Value>,

    /// JSONPath to the property
    #[field(
        display_name = "Property Path",
        description = "The property path to extract (JSONPath syntax)",
        example = "user.name"
    )]
    pub property_path: Option<String>,
}

/// Input for setting a value in an object at a property path
#[derive(Debug, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Set Value Input")]
pub struct SetValueByPathInput {
    /// The target object to modify
    #[field(
        display_name = "Target Object",
        description = "The object to set a property value in",
        example = r#"{"user": {"name": "Alice"}}"#
    )]
    pub target: Option<Value>,

    /// JSONPath to the property to set
    #[field(
        display_name = "Property Path",
        description = "The property path to set (JSONPath syntax, creates nested objects if needed)",
        example = "user.age"
    )]
    pub property_path: Option<String>,

    /// The value to set
    #[field(
        display_name = "Value",
        description = "The value to set at the property path",
        example = "30"
    )]
    pub value: Option<Value>,
}

/// Input for filtering out empty/null/blank values from an array
#[derive(Debug, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Filter Non-Values Input")]
pub struct FilterNoValueInput {
    /// The array to filter
    #[field(
        display_name = "Input Array",
        description = "The array of items to filter",
        example = r#"[{"x": 1}, {"x": null}, {"x": ""}]"#
    )]
    pub value: Vec<Value>,

    /// Optional property path to check
    #[field(
        display_name = "Property Path",
        description = "The property path to check in each item (if omitted, checks the item itself)",
        example = "x"
    )]
    #[serde(default)]
    pub property_path: Option<String>,

    /// Remove items with empty strings
    #[field(
        display_name = "Filter Empty Strings",
        description = "Remove items where the value is an empty string (\"\")",
        example = "false",
        default = "false"
    )]
    #[serde(default)]
    pub filter_empty_strings: bool,

    /// Remove items with null values
    #[field(
        display_name = "Filter Null Values",
        description = "Remove items where the value is null",
        example = "true",
        default = "false"
    )]
    #[serde(default)]
    pub filter_null_values: bool,

    /// Remove items with blank strings
    #[field(
        display_name = "Filter Blank Strings",
        description = "Remove items where the value is a whitespace-only string",
        example = "false",
        default = "false"
    )]
    #[serde(default)]
    pub filter_blank_strings: bool,

    /// Remove items with zero values
    #[field(
        display_name = "Filter Zero Values",
        description = "Remove items where the value is 0 or \"0\"",
        example = "false",
        default = "false"
    )]
    #[serde(default)]
    pub filter_zero_values: bool,
}

/// Input for selecting the first truthy value from an array
#[derive(Debug, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Select First Input")]
pub struct SelectFirstInput {
    /// The array to select from
    #[field(
        display_name = "Input Array",
        description = "The array to select the first truthy value from (skips null, empty strings, 0, false)",
        example = r#"[null, "", 0, "hello", "world"]"#
    )]
    pub value: Option<Vec<Value>>,
}

/// Input for coalescing values (returning first non-null)
#[derive(Debug, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Coalesce Input")]
pub struct CoalesceInput {
    /// The array of values to check in order
    #[field(
        display_name = "Values",
        description = "Array of values to check; returns the first non-null/non-undefined value",
        example = r#"[null, 42, "fallback"]"#
    )]
    pub values: Vec<Value>,

    /// Treat empty strings as null
    #[field(
        display_name = "Treat Empty String As Null",
        description = "If true, empty strings are treated as null and skipped",
        example = "false",
        default = "false"
    )]
    #[serde(default)]
    pub treat_empty_string_as_null: bool,

    /// Treat zero as null
    #[field(
        display_name = "Treat Zero As Null",
        description = "If true, zero values (0, 0.0) are treated as null and skipped",
        example = "false",
        default = "false"
    )]
    #[serde(default)]
    pub treat_zero_as_null: bool,
}

/// Input for parsing a JSON string into a value
#[derive(Debug, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Parse JSON Input")]
pub struct FromJsonStringInput {
    /// The JSON string to parse
    #[field(
        display_name = "JSON String",
        description = "The JSON string to parse into a value",
        example = r#""{\"name\":\"Alice\",\"age\":30}""#
    )]
    pub value: Option<String>,
}

/// Input for converting a value to a JSON string
#[derive(Debug, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Stringify JSON Input")]
pub struct ToJsonStringInput {
    /// The value to convert
    #[field(
        display_name = "Input Value",
        description = "The value to serialize to a JSON string",
        example = r#"{"name": "Alice", "age": 30}"#
    )]
    pub value: Value,
}

/// Condition for filtering array items
#[derive(Debug, Deserialize, Clone, PartialEq, VariantNames)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
#[strum(serialize_all = "SCREAMING_SNAKE_CASE")]
pub enum MatchCondition {
    /// Keep items where property value is in match_values
    Includes,
    /// Keep items where property value is NOT in match_values
    Excludes,
    /// Keep items where property value starts with one of match_values
    StartsWith,
    /// Keep items where property value ends with one of match_values
    EndsWith,
    /// Keep items where property value contains one of match_values
    Contains,
}

impl EnumVariants for MatchCondition {
    fn variant_names() -> &'static [&'static str] {
        Self::VARIANTS
    }
}

/// Input for filtering an array based on property values
#[derive(Debug, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Filter Array Input")]
pub struct FilterInput {
    /// The array to filter
    #[field(
        display_name = "Input Array",
        description = "The array of items to filter",
        example = r#"[{"status": "active"}, {"status": "inactive"}]"#
    )]
    #[serde(default, deserialize_with = "deserialize_value_or_empty_vec")]
    pub value: Vec<Value>,

    /// JSONPath to the property to check
    #[field(
        display_name = "Property Path",
        description = "The property path to extract from each item (JSONPath syntax). Use \"$\" or \"\" to filter the array values directly.",
        example = "status"
    )]
    pub property_path: String,

    /// Values to match against
    #[field(
        display_name = "Match Values",
        description = "The value(s) to compare against (single value or array)",
        example = r#"["active", "pending"]"#
    )]
    pub match_values: Value,

    /// How to match the values
    #[field(
        display_name = "Match Condition",
        description = "Whether to include or exclude matching items",
        example = "INCLUDES",
        enum_type = "MatchCondition"
    )]
    pub match_condition: MatchCondition,
}

/// Input for sorting an array
#[derive(Debug, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Sort Array Input")]
pub struct SortInput {
    /// The array to sort
    #[field(
        display_name = "Input Array",
        description = "The array to sort",
        example = r#"[{"age": 35}, {"age": 30}, {"age": 25}]"#
    )]
    pub value: Vec<Value>,

    /// Optional property path to sort by
    #[field(
        display_name = "Property Path",
        description = "The property path to sort by (if omitted, sorts the items directly)",
        example = "age"
    )]
    pub property_path: Option<String>,

    /// Sort in ascending order
    #[field(
        display_name = "Ascending Order",
        description = "Whether to sort in ascending order (true) or descending order (false)",
        example = "true",
        default = "true"
    )]
    #[serde(default = "default_ascending")]
    pub ascending: bool,
}

fn default_ascending() -> bool {
    true
}

/// Input for mapping fields from source to target object
#[derive(Debug, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Map Fields Input")]
pub struct MapFieldsInput {
    /// The source object to map from
    #[field(
        display_name = "Source Data",
        description = "The source object containing data to map",
        example = r#"{"firstName": "Alice", "userAge": 30, "email": "alice@example.com"}"#
    )]
    pub source_data: Value,

    /// Field mappings from source to target
    #[field(
        display_name = "Field Mappings",
        description = "Map of source field paths to target field names",
        example = r#"{"firstName": "name", "userAge": "age"}"#
    )]
    pub mappings: HashMap<String, String>,
}

/// Input for grouping array items by a key
#[derive(Debug, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Group By Input")]
pub struct GroupByInput {
    /// The array to group
    #[field(
        display_name = "Input Array",
        description = "The array of items to group",
        example = r#"[{"name": "Alice", "status": "active"}, {"name": "Bob", "status": "inactive"}]"#
    )]
    pub value: Value,

    /// The property path to group by
    #[field(
        display_name = "Group Key",
        description = "The property path to use as the grouping key (JSONPath syntax)",
        example = "status"
    )]
    pub key: String,

    /// Return as map instead of array
    #[field(
        display_name = "Return As Map",
        description = "Return grouped items as a map (key -> items) instead of an array of arrays",
        example = "true",
        default = "false"
    )]
    #[serde(default)]
    pub as_map: bool,
}

/// Input for appending an item to an array
#[derive(Debug, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Append Input")]
pub struct AppendInput {
    /// The array to append to
    #[field(
        display_name = "Array",
        description = "The array to append an item to (can contain objects or primitive values)",
        example = r#"[{"name": "Alice"}, {"name": "Bob"}]"#
    )]
    pub array: Vec<Value>,

    /// The item to append
    #[field(
        display_name = "Item",
        description = "The item to append to the array (can be an object or primitive value)",
        example = r#"{"name": "Charlie"}"#
    )]
    pub item: Value,
}

/// Input for flat mapping an array (extracting nested arrays and flattening)
#[derive(Debug, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Flat Map Input")]
pub struct FlatMapInput {
    /// The array of objects containing nested arrays
    #[field(
        display_name = "Input Array",
        description = "The array of objects to flat map",
        example = r#"[{"items": [1, 2]}, {"items": [3, 4]}]"#
    )]
    pub value: Vec<Value>,

    /// JSONPath to the nested array property to extract and flatten
    #[field(
        display_name = "Property Path",
        description = "The property path to the nested array in each item (JSONPath syntax)",
        example = "items"
    )]
    pub property_path: String,
}

// ============================================================================
// Output Types
// ============================================================================

/// Output from extract operation
#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Extract Output")]
pub struct ExtractOutput {
    /// The extracted values
    #[field(
        display_name = "Values",
        description = "Array of extracted property values"
    )]
    pub values: Vec<Value>,

    /// Number of values extracted
    #[field(display_name = "Count", description = "Number of values extracted")]
    pub count: usize,
}

/// Output from filter operations (filter, filter_non_values)
#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Filter Output")]
pub struct FilterOutput {
    /// The filtered items
    #[field(
        display_name = "Items",
        description = "Array of items that passed the filter"
    )]
    pub items: Vec<Value>,

    /// Number of items after filtering
    #[field(
        display_name = "Count",
        description = "Number of items that passed the filter"
    )]
    pub count: usize,

    /// Number of items removed by the filter
    #[field(
        display_name = "Removed Count",
        description = "Number of items removed by the filter"
    )]
    pub removed_count: usize,
}

/// Output from sort operation
#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Sort Output")]
pub struct SortOutput {
    /// The sorted items
    #[field(display_name = "Items", description = "Array of sorted items")]
    pub items: Vec<Value>,

    /// Number of items
    #[field(
        display_name = "Count",
        description = "Number of items in the sorted array"
    )]
    pub count: usize,
}

/// Output from group_by operation
#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Group By Output")]
pub struct GroupByOutput {
    /// Grouped items (as map or array based on as_map flag)
    #[field(
        display_name = "Groups",
        description = "Grouped items - either a map (key -> items) or array of arrays"
    )]
    pub groups: Value,

    /// Number of unique groups
    #[field(
        display_name = "Group Count",
        description = "Number of unique groups created"
    )]
    pub group_count: usize,
}

/// Output from map_fields operation
#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Map Fields Output")]
pub struct MapFieldsOutput {
    /// The mapped result object
    #[field(
        display_name = "Result",
        description = "Object with mapped field values"
    )]
    pub result: HashMap<String, Value>,

    /// Number of fields mapped
    #[field(
        display_name = "Field Count",
        description = "Number of fields successfully mapped"
    )]
    pub field_count: usize,
}

/// Output from append operation
#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Append Output")]
pub struct AppendOutput {
    /// The array with appended item
    #[field(
        display_name = "Array",
        description = "Array with the new item appended"
    )]
    pub array: Vec<Value>,

    /// New length of the array
    #[field(
        display_name = "Length",
        description = "New length of the array after appending"
    )]
    pub length: usize,
}

/// Output from flat_map operation
#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Flat Map Output")]
pub struct FlatMapOutput {
    /// The flattened items
    #[field(
        display_name = "Items",
        description = "Flattened array of all nested items"
    )]
    pub items: Vec<Value>,

    /// Number of items after flattening
    #[field(
        display_name = "Count",
        description = "Total number of items in the flattened array"
    )]
    pub count: usize,
}

/// Input for array-length operation
#[derive(Debug, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Array Length Input")]
pub struct ArrayLengthInput {
    /// The array or value to get the length of
    #[field(
        display_name = "Value",
        description = "Array, string, or object to get the length/size of",
        example = r#"[1, 2, 3]"#
    )]
    pub value: Value,
}

/// Output from array-length operation
#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Array Length Output")]
pub struct ArrayLengthOutput {
    /// The length/size of the value
    #[field(
        display_name = "Length",
        description = "Length of array/string, or number of keys in object"
    )]
    pub length: usize,

    /// Whether the value was an array
    #[field(
        display_name = "Is Array",
        description = "True if the value was an array"
    )]
    pub is_array: bool,
}

/// Output from to_json_string operation
#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "JSON String Output")]
pub struct ToJsonStringOutput {
    /// The JSON string
    #[field(display_name = "JSON", description = "The serialized JSON string")]
    pub json: String,

    /// Length of the JSON string
    #[field(
        display_name = "Length",
        description = "Length of the JSON string in characters"
    )]
    pub length: usize,
}

// ============================================================================
// Operations
// ============================================================================

/// Extracts values from an array of objects based on a property path
#[capability(
    module = "transform",
    display_name = "Extract Property",
    description = "Extract property values from an array of objects based on a property path"
)]
pub fn extract(input: ExtractInput) -> Result<ExtractOutput, String> {
    if input.value.is_empty() || input.property_path.is_empty() {
        let count = input.value.len();
        return Ok(ExtractOutput {
            values: input.value,
            count,
        });
    }

    let values: Vec<Value> = input
        .value
        .iter()
        .map(|item| get_property_value(item, &input.property_path))
        .collect();
    let count = values.len();
    Ok(ExtractOutput { values, count })
}

/// Gets a value from an object by property path
#[capability(
    module = "transform",
    display_name = "Get Value By Path",
    description = "Get a value from an object using a JSONPath-like property path"
)]
pub fn get_value_by_path(input: GetValueByPathInput) -> Result<Value, String> {
    let result = match (input.value, input.property_path) {
        (Some(value), Some(path)) if !path.is_empty() => get_property_value(&value, &path),
        _ => Value::Null,
    };
    Ok(result)
}

/// Sets a value in an object at the specified property path
#[capability(
    module = "transform",
    display_name = "Set Value By Path",
    description = "Set a value in an object at a specified JSONPath-like property path"
)]
pub fn set_value_by_path(input: SetValueByPathInput) -> Result<Value, String> {
    let result = match (input.target, input.property_path, input.value) {
        (Some(target), Some(path), value) if !path.is_empty() => {
            set_property_value(target, &path, value.unwrap_or(Value::Null))
        }
        (Some(target), _, _) => target,
        _ => Value::Null,
    };
    Ok(result)
}

/// Filters an array removing elements with no values based on criteria
#[capability(
    module = "transform",
    display_name = "Filter Non-Values",
    description = "Filter an array removing elements with null, empty, blank, or zero values"
)]
pub fn filter_non_values(input: FilterNoValueInput) -> Result<FilterOutput, String> {
    let original_count = input.value.len();
    if input.value.is_empty() {
        return Ok(FilterOutput {
            items: input.value,
            count: 0,
            removed_count: 0,
        });
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

            // Filter null values if specified
            if input.filter_null_values && property_value.is_null() {
                return false;
            }

            // Filter empty strings if specified
            if input.filter_empty_strings
                && let Some(s) = property_value.as_str()
                && s.is_empty()
            {
                return false;
            }

            // Filter blank strings if specified
            if input.filter_blank_strings
                && let Some(s) = property_value.as_str()
                && s.trim().is_empty()
            {
                return false;
            }

            // Filter zero values if specified
            if input.filter_zero_values {
                if let Some(n) = property_value.as_f64()
                    && n == 0.0
                {
                    return false;
                }
                if let Some(s) = property_value.as_str()
                    && s == "0"
                {
                    return false;
                }
            }

            true
        })
        .collect();
    let count = items.len();
    Ok(FilterOutput {
        items,
        count,
        removed_count: original_count - count,
    })
}

/// Returns the first truthy value from an array
#[capability(
    module = "transform",
    display_name = "Select First",
    description = "Select the first truthy value from an array (skips null, empty, zero, false)"
)]
pub fn select_first(input: SelectFirstInput) -> Result<Value, String> {
    let Some(values) = input.value else {
        return Ok(Value::Null);
    };

    if values.is_empty() {
        return Ok(Value::Null);
    }

    for item in values {
        // Skip null values
        if item.is_null() {
            continue;
        }

        // Filter empty strings
        if let Some(s) = item.as_str()
            && (s.is_empty() || s.trim().is_empty())
        {
            continue;
        }

        // Filter zero values
        if let Some(n) = item.as_f64()
            && n == 0.0
        {
            continue;
        }

        // Filter false booleans
        if let Some(b) = item.as_bool()
            && !b
        {
            continue;
        }

        return Ok(item);
    }

    Ok(Value::Null)
}

/// Returns the first non-null value from an array
#[capability(
    module = "transform",
    display_name = "Coalesce",
    description = "Return the first non-null value from an array of values"
)]
pub fn coalesce(input: CoalesceInput) -> Result<Value, String> {
    for value in input.values {
        // Skip null values
        if value.is_null() {
            continue;
        }

        // Skip empty strings if flag is set
        if input.treat_empty_string_as_null {
            if let Some(s) = value.as_str() {
                if s.is_empty() {
                    continue;
                }
            }
        }

        // Skip zero values if flag is set
        if input.treat_zero_as_null {
            if let Some(n) = value.as_f64() {
                if n == 0.0 {
                    continue;
                }
            }
        }

        return Ok(value);
    }

    Ok(Value::Null)
}

/// Parses a JSON string into a Value
#[capability(
    module = "transform",
    display_name = "From JSON String",
    description = "Parse a JSON string into a structured value",
    errors(permanent("TRANSFORM_JSON_PARSE_ERROR", "Failed to parse JSON string"),)
)]
pub fn from_json_string(input: FromJsonStringInput) -> Result<Value, AgentError> {
    match input.value {
        Some(json_str) if !json_str.is_empty() => serde_json::from_str(&json_str).map_err(|e| {
            AgentError::permanent(
                "TRANSFORM_JSON_PARSE_ERROR",
                format!("Failed to parse JSON: {}", e),
            )
            .with_attr("parse_error", e.to_string())
        }),
        _ => Ok(Value::Null),
    }
}

/// Converts a Value to a JSON string
#[capability(
    module = "transform",
    display_name = "To JSON String",
    description = "Convert a value to a JSON string",
    errors(permanent("TRANSFORM_JSON_SERIALIZE_ERROR", "Failed to serialize value to JSON"),)
)]
pub fn to_json_string(input: ToJsonStringInput) -> Result<ToJsonStringOutput, AgentError> {
    let json = serde_json::to_string(&input.value).map_err(|e| {
        AgentError::permanent(
            "TRANSFORM_JSON_SERIALIZE_ERROR",
            format!("Failed to serialize JSON: {}", e),
        )
        .with_attr("serialize_error", e.to_string())
    })?;
    let length = json.len();
    Ok(ToJsonStringOutput { json, length })
}

/// Filters an array based on property values matching filter criteria
#[capability(
    module = "transform",
    display_name = "Filter Array",
    description = "Filter an array based on property values matching or excluding specified values"
)]
pub fn filter(input: FilterInput) -> Result<FilterOutput, String> {
    let original_count = input.value.len();
    if input.value.is_empty() {
        return Ok(FilterOutput {
            items: input.value,
            count: 0,
            removed_count: 0,
        });
    }

    // Convert match_values to a list
    let match_values_list: Vec<Value> = match input.match_values {
        Value::Array(arr) => arr,
        other => vec![other],
    };

    // Check if we should filter the actual values (property_path is "$" or empty)
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
    Ok(FilterOutput {
        items: filtered,
        count,
        removed_count: original_count - count,
    })
}

/// Sorts an array based on a property path
#[capability(
    module = "transform",
    display_name = "Sort Array",
    description = "Sort an array of items, optionally by a property path"
)]
pub fn sort(input: SortInput) -> Result<SortOutput, String> {
    if input.value.is_empty() {
        return Ok(SortOutput {
            items: input.value,
            count: 0,
        });
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
    Ok(SortOutput {
        items: sorted_list,
        count,
    })
}

/// Maps fields from source object to target object based on mappings
#[capability(
    module = "transform",
    display_name = "Map Fields",
    description = "Map fields from a source object to a target object using field path mappings"
)]
pub fn map_fields(input: MapFieldsInput) -> Result<MapFieldsOutput, String> {
    let mut result = HashMap::new();

    for (source_field, target_field) in input.mappings {
        let value = get_property_value(&input.source_data, &source_field);
        if !value.is_null() {
            result.insert(target_field, value);
        }
    }

    let field_count = result.len();
    Ok(MapFieldsOutput {
        result,
        field_count,
    })
}

/// Groups an array of items by a specified key (JSONPath)
/// Returns either a Map<key, Vec<items>> or Array<Vec<items>> based on as_map flag
#[capability(
    module = "transform",
    display_name = "Group By",
    description = "Group array items by a property key, returning either a map or array of groups",
    errors(
        permanent("TRANSFORM_INVALID_INPUT", "Expected array or collection input"),
        permanent(
            "TRANSFORM_KEY_SERIALIZE_ERROR",
            "Failed to serialize group key to string"
        ),
    )
)]
pub fn group_by(input: GroupByInput) -> Result<GroupByOutput, AgentError> {
    // Validate input - only arrays are supported
    let collection = match &input.value {
        Value::Array(arr) => arr,
        Value::Null => {
            return Err(AgentError::permanent(
                "TRANSFORM_INVALID_INPUT",
                "Unsupported value. Expected array or collection.",
            )
            .with_attr("received_type", "null"));
        }
        other => {
            let type_name = match other {
                Value::Object(_) => "object",
                Value::String(_) => "string",
                Value::Number(_) => "number",
                Value::Bool(_) => "boolean",
                _ => "unknown",
            };
            return Err(AgentError::permanent(
                "TRANSFORM_INVALID_INPUT",
                "Unsupported value. Expected array or collection.",
            )
            .with_attr("received_type", type_name));
        }
    };

    // Return empty result for empty arrays
    if collection.is_empty() {
        return Ok(GroupByOutput {
            groups: if input.as_map {
                Value::Object(serde_json::Map::new())
            } else {
                Value::Array(vec![])
            },
            group_count: 0,
        });
    }

    // Prepare the JSON path (add $. prefix if not present)
    let json_path = if input.key.starts_with("$.") {
        input.key.clone()
    } else {
        format!("$.{}", input.key)
    };

    // Group items by the value at the specified JSON path
    // Use a HashMap with string keys (since JSON object keys must be strings)
    let mut grouped: HashMap<String, Vec<Value>> = HashMap::new();

    for item in collection {
        // Extract the key value using JsonPath
        let key_value = get_property_value(item, &json_path);

        // Skip items where the key value is null
        if key_value.is_null() {
            continue;
        }

        // Convert key to string for HashMap key
        let key_str = match &key_value {
            Value::String(s) => s.clone(),
            Value::Number(n) => n.to_string(),
            Value::Bool(b) => b.to_string(),
            _ => serde_json::to_string(&key_value).map_err(|e| {
                AgentError::permanent(
                    "TRANSFORM_KEY_SERIALIZE_ERROR",
                    format!("Failed to serialize group key to string: {}", e),
                )
            })?,
        };

        grouped.entry(key_str).or_default().push(item.clone());
    }

    // Return based on as_map flag
    let group_count = grouped.len();
    if input.as_map {
        // Convert HashMap to JSON Object
        let mut map = serde_json::Map::new();
        for (key, values) in grouped {
            map.insert(key, Value::Array(values));
        }
        Ok(GroupByOutput {
            groups: Value::Object(map),
            group_count,
        })
    } else {
        // Return just the values (array of arrays)
        let arrays: Vec<Value> = grouped.into_values().map(Value::Array).collect();
        Ok(GroupByOutput {
            groups: Value::Array(arrays),
            group_count,
        })
    }
}

/// Appends an item to an array
/// Returns a new array with the item appended to the end
#[capability(
    module = "transform",
    display_name = "Append",
    description = "Append an item to the end of an array"
)]
pub fn append(input: AppendInput) -> Result<AppendOutput, String> {
    let mut array = input.array;
    array.push(input.item);
    let length = array.len();
    Ok(AppendOutput { array, length })
}

/// Extracts nested arrays from each item and flattens them into a single array
#[capability(
    module = "transform",
    display_name = "Flat Map",
    description = "Extract nested arrays from each item by property path and flatten into a single array"
)]
pub fn flat_map(input: FlatMapInput) -> Result<FlatMapOutput, String> {
    if input.value.is_empty() || input.property_path.is_empty() {
        return Ok(FlatMapOutput {
            items: vec![],
            count: 0,
        });
    }

    let mut items = Vec::new();

    for item in input.value {
        let nested = get_property_value(&item, &input.property_path);
        if let Some(arr) = nested.as_array() {
            items.extend(arr.iter().cloned());
        }
    }

    let count = items.len();
    Ok(FlatMapOutput { items, count })
}

/// Gets the length/size of an array, string, or object
#[capability(
    module = "transform",
    display_name = "Array Length",
    description = "Get the length of an array, string, or number of keys in an object"
)]
pub fn array_length(input: ArrayLengthInput) -> Result<ArrayLengthOutput, String> {
    let (length, is_array) = match &input.value {
        Value::Array(arr) => (arr.len(), true),
        Value::String(s) => (s.len(), false),
        Value::Object(obj) => (obj.len(), false),
        Value::Null => (0, false),
        _ => (0, false),
    };

    Ok(ArrayLengthOutput { length, is_array })
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Gets a property value from a JSON object using a JSONPath-like syntax
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
                // Try to parse as array index
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

/// Sets a property value in a JSON object using a JSONPath-like syntax
fn set_property_value(obj: Value, property_path: &str, value: Value) -> Value {
    if property_path.is_empty() {
        return obj;
    }

    let path = property_path.strip_prefix("$.").unwrap_or(property_path);
    let parts: Vec<&str> = path.split('.').collect();

    if let Value::Object(mut map) = obj {
        if parts.len() == 1 {
            // Simple property - just set it
            map.insert(parts[0].to_string(), value);
            return Value::Object(map);
        } else {
            // Nested path - navigate or create intermediate maps
            let mut current_map = map.clone();
            set_nested_value(&mut current_map, &parts, value);
            return Value::Object(current_map);
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

/// Checks if a property value matches any of the filter values
fn matches_filter_values(property_value: &Value, filter_values: &[Value]) -> bool {
    if property_value.is_null() {
        return false;
    }

    // If property value is an array, check if any element matches
    if let Some(arr) = property_value.as_array() {
        for element in arr {
            if filter_values.contains(element) {
                return true;
            }
        }
        return false;
    }

    // Direct match
    filter_values.contains(property_value)
}

/// Matches a string value against filter values using a custom comparison function
fn matches_string_filter<F>(property_value: &Value, filter_values: &[Value], compare: F) -> bool
where
    F: Fn(&str, &str) -> bool,
{
    // Get property value as string
    let property_str = match property_value {
        Value::String(s) => s.as_str(),
        _ => return false,
    };

    // Check if any filter value matches
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

/// Compares two JSON values for sorting
fn compare_values(a: &Value, b: &Value) -> std::cmp::Ordering {
    use std::cmp::Ordering;

    match (a, b) {
        (Value::Null, Value::Null) => Ordering::Equal,
        (Value::Null, _) => Ordering::Greater, // Nulls go to the end
        (_, Value::Null) => Ordering::Less,

        // Compare numbers
        (Value::Number(n1), Value::Number(n2)) => {
            let f1 = n1.as_f64().unwrap_or(0.0);
            let f2 = n2.as_f64().unwrap_or(0.0);
            f1.partial_cmp(&f2).unwrap_or(Ordering::Equal)
        }

        // Compare strings
        (Value::String(s1), Value::String(s2)) => s1.cmp(s2),

        // Compare booleans
        (Value::Bool(b1), Value::Bool(b2)) => b1.cmp(b2),

        // Mixed types - convert to string and compare
        _ => a.to_string().cmp(&b.to_string()),
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_extract() {
        let input = ExtractInput {
            value: vec![
                json!({"name": "Alice", "age": 30}),
                json!({"name": "Bob", "age": 25}),
            ],
            property_path: "name".to_string(),
        };

        let result = extract(input).unwrap();
        assert_eq!(result.values, vec![json!("Alice"), json!("Bob")]);
        assert_eq!(result.count, 2);
    }

    #[test]
    fn test_get_value_by_path() {
        let input = GetValueByPathInput {
            value: Some(json!({"user": {"name": "Alice", "age": 30}})),
            property_path: Some("user.name".to_string()),
        };

        let result = get_value_by_path(input).unwrap();
        assert_eq!(result, json!("Alice"));
    }

    #[test]
    fn test_get_value_by_path_null() {
        let input = GetValueByPathInput {
            value: None,
            property_path: Some("user.name".to_string()),
        };

        let result = get_value_by_path(input).unwrap();
        assert_eq!(result, Value::Null);
    }

    #[test]
    fn test_set_value_by_path() {
        let input = SetValueByPathInput {
            target: Some(json!({"user": {"name": "Alice"}})),
            property_path: Some("user.age".to_string()),
            value: Some(json!(30)),
        };

        let result = set_value_by_path(input).unwrap();
        assert_eq!(result, json!({"user": {"name": "Alice", "age": 30}}));
    }

    #[test]
    fn test_filter_non_values_null() {
        let input = FilterNoValueInput {
            value: vec![json!({"x": 1}), json!({"x": null}), json!({"x": 2})],
            property_path: Some("x".to_string()),
            filter_empty_strings: false,
            filter_null_values: true,
            filter_blank_strings: false,
            filter_zero_values: false,
        };

        let result = filter_non_values(input).unwrap();
        assert_eq!(result.count, 2);
        assert_eq!(result.items.len(), 2);
    }

    #[test]
    fn test_filter_non_values_empty_strings() {
        let input = FilterNoValueInput {
            value: vec![
                json!({"x": "hello"}),
                json!({"x": ""}),
                json!({"x": "world"}),
            ],
            property_path: Some("x".to_string()),
            filter_empty_strings: true,
            filter_null_values: false,
            filter_blank_strings: false,
            filter_zero_values: false,
        };

        let result = filter_non_values(input).unwrap();
        assert_eq!(result.count, 2);
        assert_eq!(result.items.len(), 2);
    }

    #[test]
    fn test_select_first() {
        let input = SelectFirstInput {
            value: Some(vec![json!(null), json!(""), json!(0), json!("hello")]),
        };

        let result = select_first(input).unwrap();
        assert_eq!(result, json!("hello"));
    }

    #[test]
    fn test_from_json_string() {
        let input = FromJsonStringInput {
            value: Some(r#"{"name":"Alice","age":30}"#.to_string()),
        };

        let result = from_json_string(input).unwrap();
        assert_eq!(result, json!({"name": "Alice", "age": 30}));
    }

    #[test]
    fn test_to_json_string() {
        let input = ToJsonStringInput {
            value: json!({"name": "Alice", "age": 30}),
        };

        let result = to_json_string(input).unwrap();
        assert!(result.json.contains("Alice"));
        assert!(result.json.contains("30"));
        assert!(result.length > 0);
    }

    #[test]
    fn test_filter_includes() {
        let input = FilterInput {
            value: vec![
                json!({"status": "active"}),
                json!({"status": "inactive"}),
                json!({"status": "pending"}),
            ],
            property_path: "status".to_string(),
            match_values: json!(["active", "pending"]),
            match_condition: MatchCondition::Includes,
        };

        let result = filter(input).unwrap();
        assert_eq!(result.count, 2);
        assert_eq!(result.items.len(), 2);
    }

    #[test]
    fn test_filter_excludes() {
        let input = FilterInput {
            value: vec![
                json!({"status": "active"}),
                json!({"status": "inactive"}),
                json!({"status": "pending"}),
            ],
            property_path: "status".to_string(),
            match_values: json!(["inactive"]),
            match_condition: MatchCondition::Excludes,
        };

        let result = filter(input).unwrap();
        assert_eq!(result.count, 2);
        assert_eq!(result.items.len(), 2);
    }

    #[test]
    fn test_filter_primitive_array_with_dollar_sign() {
        let input = FilterInput {
            value: vec![json!("active"), json!("pending"), json!("inactive")],
            property_path: "$".to_string(),
            match_values: json!(["active"]),
            match_condition: MatchCondition::Excludes,
        };

        let result = filter(input).unwrap();
        assert_eq!(result.count, 2);
        assert_eq!(result.items, vec![json!("pending"), json!("inactive")]);
    }

    #[test]
    fn test_filter_primitive_array_with_empty_string() {
        let input = FilterInput {
            value: vec![json!("active"), json!("pending"), json!("inactive")],
            property_path: "".to_string(),
            match_values: json!(["active"]),
            match_condition: MatchCondition::Excludes,
        };

        let result = filter(input).unwrap();
        assert_eq!(result.count, 2);
        assert_eq!(result.items, vec![json!("pending"), json!("inactive")]);
    }

    #[test]
    fn test_filter_primitive_array_includes() {
        let input = FilterInput {
            value: vec![
                json!("active"),
                json!("pending"),
                json!("inactive"),
                json!("archived"),
            ],
            property_path: "$".to_string(),
            match_values: json!(["active", "pending"]),
            match_condition: MatchCondition::Includes,
        };

        let result = filter(input).unwrap();
        assert_eq!(result.count, 2);
        assert_eq!(result.items, vec![json!("active"), json!("pending")]);
    }

    #[test]
    fn test_filter_primitive_numbers() {
        let input = FilterInput {
            value: vec![json!(1), json!(2), json!(3), json!(4), json!(5)],
            property_path: "$".to_string(),
            match_values: json!([2, 4]),
            match_condition: MatchCondition::Excludes,
        };

        let result = filter(input).unwrap();
        assert_eq!(result.count, 3);
        assert_eq!(result.items, vec![json!(1), json!(3), json!(5)]);
    }

    #[test]
    fn test_sort_ascending() {
        let input = SortInput {
            value: vec![
                json!({"name": "Charlie", "age": 35}),
                json!({"name": "Alice", "age": 30}),
                json!({"name": "Bob", "age": 25}),
            ],
            property_path: Some("age".to_string()),
            ascending: true,
        };

        let result = sort(input).unwrap();
        assert_eq!(result.count, 3);
        assert_eq!(result.items[0], json!({"name": "Bob", "age": 25}));
        assert_eq!(result.items[2], json!({"name": "Charlie", "age": 35}));
    }

    #[test]
    fn test_sort_descending() {
        let input = SortInput {
            value: vec![json!(3), json!(1), json!(2)],
            property_path: None,
            ascending: false,
        };

        let result = sort(input).unwrap();
        assert_eq!(result.items, vec![json!(3), json!(2), json!(1)]);
        assert_eq!(result.count, 3);
    }

    #[test]
    fn test_map_fields() {
        let mut mappings = HashMap::new();
        mappings.insert("firstName".to_string(), "name".to_string());
        mappings.insert("userAge".to_string(), "age".to_string());

        let input = MapFieldsInput {
            source_data: json!({"firstName": "Alice", "userAge": 30, "email": "alice@example.com"}),
            mappings,
        };

        let result = map_fields(input).unwrap();
        assert_eq!(result.result.get("name"), Some(&json!("Alice")));
        assert_eq!(result.result.get("age"), Some(&json!(30)));
        assert_eq!(result.result.get("email"), None);
        assert_eq!(result.field_count, 2);
    }

    #[test]
    fn test_group_by_as_map() {
        let input = GroupByInput {
            value: json!([
                {"name": "Alice", "status": "active"},
                {"name": "Bob", "status": "inactive"},
                {"name": "Charlie", "status": "active"},
                {"name": "David", "status": "pending"}
            ]),
            key: "status".to_string(),
            as_map: true,
        };

        let result = group_by(input).unwrap();
        assert!(result.groups.is_object());
        assert_eq!(result.group_count, 3);

        let obj = result.groups.as_object().unwrap();
        assert_eq!(obj.get("active").unwrap().as_array().unwrap().len(), 2);
        assert_eq!(obj.get("inactive").unwrap().as_array().unwrap().len(), 1);
        assert_eq!(obj.get("pending").unwrap().as_array().unwrap().len(), 1);
    }

    #[test]
    fn test_group_by_as_array() {
        let input = GroupByInput {
            value: json!([
                {"name": "Alice", "status": "active"},
                {"name": "Bob", "status": "inactive"},
                {"name": "Charlie", "status": "active"}
            ]),
            key: "status".to_string(),
            as_map: false,
        };

        let result = group_by(input).unwrap();
        assert!(result.groups.is_array());
        assert_eq!(result.group_count, 2);

        let arr = result.groups.as_array().unwrap();
        assert_eq!(arr.len(), 2); // Two groups: active and inactive

        // Check that we have arrays of expected sizes (2 active, 1 inactive)
        let sizes: Vec<usize> = arr.iter().map(|v| v.as_array().unwrap().len()).collect();
        assert!(sizes.contains(&2));
        assert!(sizes.contains(&1));
    }

    #[test]
    fn test_group_by_nested_path() {
        let input = GroupByInput {
            value: json!([
                {"name": "Alice", "details": {"category": "A"}},
                {"name": "Bob", "details": {"category": "B"}},
                {"name": "Charlie", "details": {"category": "A"}}
            ]),
            key: "details.category".to_string(),
            as_map: true,
        };

        let result = group_by(input).unwrap();
        assert_eq!(result.group_count, 2);
        let obj = result.groups.as_object().unwrap();
        assert_eq!(obj.get("A").unwrap().as_array().unwrap().len(), 2);
        assert_eq!(obj.get("B").unwrap().as_array().unwrap().len(), 1);
    }

    #[test]
    fn test_group_by_skip_null_keys() {
        let input = GroupByInput {
            value: json!([
                {"name": "Alice", "status": "active"},
                {"name": "Bob"},  // No status field
                {"name": "Charlie", "status": null},  // Null status
                {"name": "David", "status": "active"}
            ]),
            key: "status".to_string(),
            as_map: true,
        };

        let result = group_by(input).unwrap();
        assert_eq!(result.group_count, 1);
        let obj = result.groups.as_object().unwrap();

        // Only items with non-null status should be grouped
        assert_eq!(obj.get("active").unwrap().as_array().unwrap().len(), 2);
        assert_eq!(obj.len(), 1); // Only "active" group exists
    }

    #[test]
    fn test_group_by_empty_array() {
        let input = GroupByInput {
            value: json!([]),
            key: "status".to_string(),
            as_map: true,
        };

        let result = group_by(input).unwrap();
        assert!(result.groups.is_object());
        assert_eq!(result.group_count, 0);
        assert_eq!(result.groups.as_object().unwrap().len(), 0);
    }

    #[test]
    fn test_group_by_numeric_keys() {
        let input = GroupByInput {
            value: json!([
                {"name": "Alice", "age": 30},
                {"name": "Bob", "age": 25},
                {"name": "Charlie", "age": 30}
            ]),
            key: "age".to_string(),
            as_map: true,
        };

        let result = group_by(input).unwrap();
        assert_eq!(result.group_count, 2);
        let obj = result.groups.as_object().unwrap();

        // Numeric keys are converted to strings
        assert_eq!(obj.get("30").unwrap().as_array().unwrap().len(), 2);
        assert_eq!(obj.get("25").unwrap().as_array().unwrap().len(), 1);
    }

    #[test]
    fn test_group_by_invalid_input() {
        let input = GroupByInput {
            value: json!({"not": "an array"}),
            key: "status".to_string(),
            as_map: true,
        };

        let result = group_by(input);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.code, "TRANSFORM_INVALID_INPUT");
        assert!(err.message.contains("Expected array"));
        assert_eq!(
            err.attributes.get("received_type"),
            Some(&"object".to_string())
        );
    }

    #[test]
    fn test_append_object_to_array() {
        let input = AppendInput {
            array: vec![
                json!({"name": "Alice", "age": 30}),
                json!({"name": "Bob", "age": 25}),
            ],
            item: json!({"name": "Charlie", "age": 35}),
        };

        let result = append(input).unwrap();
        assert_eq!(result.length, 3);
        assert_eq!(result.array[2], json!({"name": "Charlie", "age": 35}));
    }

    #[test]
    fn test_append_string_to_array() {
        let input = AppendInput {
            array: vec![json!("apple"), json!("banana")],
            item: json!("cherry"),
        };

        let result = append(input).unwrap();
        assert_eq!(result.length, 3);
        assert_eq!(result.array[2], json!("cherry"));
    }

    #[test]
    fn test_append_number_to_array() {
        let input = AppendInput {
            array: vec![json!(1), json!(2), json!(3)],
            item: json!(4),
        };

        let result = append(input).unwrap();
        assert_eq!(result.length, 4);
        assert_eq!(result.array[3], json!(4));
    }

    #[test]
    fn test_append_to_empty_array() {
        let input = AppendInput {
            array: vec![],
            item: json!({"name": "Alice"}),
        };

        let result = append(input).unwrap();
        assert_eq!(result.length, 1);
        assert_eq!(result.array[0], json!({"name": "Alice"}));
    }

    #[test]
    fn test_append_null_to_array() {
        let input = AppendInput {
            array: vec![json!(1), json!(2)],
            item: json!(null),
        };

        let result = append(input).unwrap();
        assert_eq!(result.length, 3);
        assert_eq!(result.array[2], json!(null));
    }

    #[test]
    fn test_append_array_to_array() {
        let input = AppendInput {
            array: vec![json!([1, 2]), json!([3, 4])],
            item: json!([5, 6]),
        };

        let result = append(input).unwrap();
        assert_eq!(result.length, 3);
        assert_eq!(result.array[2], json!([5, 6]));
    }

    #[test]
    fn test_append_mixed_types() {
        let input = AppendInput {
            array: vec![json!(1), json!("string"), json!({"key": "value"})],
            item: json!(true),
        };

        let result = append(input).unwrap();
        assert_eq!(result.length, 4);
        assert_eq!(result.array[0], json!(1));
        assert_eq!(result.array[1], json!("string"));
        assert_eq!(result.array[2], json!({"key": "value"}));
        assert_eq!(result.array[3], json!(true));
    }

    #[test]
    fn test_flat_map_basic() {
        let input = FlatMapInput {
            value: vec![
                json!({"items": [1, 2, 3]}),
                json!({"items": [4, 5]}),
                json!({"items": [6]}),
            ],
            property_path: "items".to_string(),
        };

        let result = flat_map(input).unwrap();
        assert_eq!(result.count, 6);
        assert_eq!(
            result.items,
            vec![json!(1), json!(2), json!(3), json!(4), json!(5), json!(6)]
        );
    }

    #[test]
    fn test_flat_map_objects() {
        let input = FlatMapInput {
            value: vec![
                json!({"records": [{"action": "created"}, {"action": "updated"}]}),
                json!({"records": [{"action": "created"}]}),
            ],
            property_path: "records".to_string(),
        };

        let result = flat_map(input).unwrap();
        assert_eq!(result.count, 3);
        assert_eq!(result.items.len(), 3);
    }

    #[test]
    fn test_flat_map_missing_property() {
        let input = FlatMapInput {
            value: vec![
                json!({"items": [1, 2]}),
                json!({"other": [3, 4]}), // missing "items"
                json!({"items": [5]}),
            ],
            property_path: "items".to_string(),
        };

        let result = flat_map(input).unwrap();
        assert_eq!(result.count, 3);
        assert_eq!(result.items, vec![json!(1), json!(2), json!(5)]);
    }

    #[test]
    fn test_flat_map_empty() {
        let input = FlatMapInput {
            value: vec![],
            property_path: "items".to_string(),
        };

        let result = flat_map(input).unwrap();
        assert_eq!(result.count, 0);
        assert!(result.items.is_empty());
    }

    #[test]
    fn test_flat_map_nested_path() {
        let input = FlatMapInput {
            value: vec![
                json!({"data": {"items": [1, 2]}}),
                json!({"data": {"items": [3]}}),
            ],
            property_path: "data.items".to_string(),
        };

        let result = flat_map(input).unwrap();
        assert_eq!(result.count, 3);
        assert_eq!(result.items, vec![json!(1), json!(2), json!(3)]);
    }

    // ==================== Coalesce Tests ====================

    #[test]
    fn test_coalesce_basic() {
        let input = CoalesceInput {
            values: vec![json!(null), json!(42)],
            treat_empty_string_as_null: false,
            treat_zero_as_null: false,
        };

        let result = coalesce(input).unwrap();
        assert_eq!(result, json!(42));
    }

    #[test]
    fn test_coalesce_multiple_fallbacks() {
        let input = CoalesceInput {
            values: vec![json!(null), json!(null), json!("fallback")],
            treat_empty_string_as_null: false,
            treat_zero_as_null: false,
        };

        let result = coalesce(input).unwrap();
        assert_eq!(result, json!("fallback"));
    }

    #[test]
    fn test_coalesce_first_value_wins() {
        let input = CoalesceInput {
            values: vec![json!("first"), json!("second"), json!("third")],
            treat_empty_string_as_null: false,
            treat_zero_as_null: false,
        };

        let result = coalesce(input).unwrap();
        assert_eq!(result, json!("first"));
    }

    #[test]
    fn test_coalesce_zero_valid_by_default() {
        let input = CoalesceInput {
            values: vec![json!(null), json!(0), json!(100)],
            treat_empty_string_as_null: false,
            treat_zero_as_null: false,
        };

        let result = coalesce(input).unwrap();
        assert_eq!(result, json!(0));
    }

    #[test]
    fn test_coalesce_treat_zero_as_null() {
        let input = CoalesceInput {
            values: vec![json!(null), json!(0), json!(100)],
            treat_empty_string_as_null: false,
            treat_zero_as_null: true,
        };

        let result = coalesce(input).unwrap();
        assert_eq!(result, json!(100));
    }

    #[test]
    fn test_coalesce_empty_string_valid_by_default() {
        let input = CoalesceInput {
            values: vec![json!(null), json!(""), json!("fallback")],
            treat_empty_string_as_null: false,
            treat_zero_as_null: false,
        };

        let result = coalesce(input).unwrap();
        assert_eq!(result, json!(""));
    }

    #[test]
    fn test_coalesce_treat_empty_string_as_null() {
        let input = CoalesceInput {
            values: vec![json!(null), json!(""), json!("fallback")],
            treat_empty_string_as_null: true,
            treat_zero_as_null: false,
        };

        let result = coalesce(input).unwrap();
        assert_eq!(result, json!("fallback"));
    }

    #[test]
    fn test_coalesce_all_null() {
        let input = CoalesceInput {
            values: vec![json!(null), json!(null)],
            treat_empty_string_as_null: false,
            treat_zero_as_null: false,
        };

        let result = coalesce(input).unwrap();
        assert_eq!(result, Value::Null);
    }

    #[test]
    fn test_coalesce_empty_array() {
        let input = CoalesceInput {
            values: vec![],
            treat_empty_string_as_null: false,
            treat_zero_as_null: false,
        };

        let result = coalesce(input).unwrap();
        assert_eq!(result, Value::Null);
    }

    #[test]
    fn test_coalesce_object_value() {
        let input = CoalesceInput {
            values: vec![json!(null), json!({"name": "Alice"})],
            treat_empty_string_as_null: false,
            treat_zero_as_null: false,
        };

        let result = coalesce(input).unwrap();
        assert_eq!(result, json!({"name": "Alice"}));
    }

    #[test]
    fn test_coalesce_array_value() {
        let input = CoalesceInput {
            values: vec![json!(null), json!([1, 2, 3])],
            treat_empty_string_as_null: false,
            treat_zero_as_null: false,
        };

        let result = coalesce(input).unwrap();
        assert_eq!(result, json!([1, 2, 3]));
    }

    #[test]
    fn test_coalesce_boolean_false_is_valid() {
        let input = CoalesceInput {
            values: vec![json!(null), json!(false), json!(true)],
            treat_empty_string_as_null: false,
            treat_zero_as_null: false,
        };

        let result = coalesce(input).unwrap();
        assert_eq!(result, json!(false));
    }

    #[test]
    fn test_coalesce_both_flags_enabled() {
        let input = CoalesceInput {
            values: vec![json!(null), json!(""), json!(0), json!("valid")],
            treat_empty_string_as_null: true,
            treat_zero_as_null: true,
        };

        let result = coalesce(input).unwrap();
        assert_eq!(result, json!("valid"));
    }

    #[test]
    fn test_coalesce_float_zero() {
        let input = CoalesceInput {
            values: vec![json!(null), json!(0.0), json!(1.5)],
            treat_empty_string_as_null: false,
            treat_zero_as_null: true,
        };

        let result = coalesce(input).unwrap();
        assert_eq!(result, json!(1.5));
    }
}
