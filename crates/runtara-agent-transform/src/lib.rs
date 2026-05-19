//! Transform agent — JSON manipulation — as a WebAssembly component.
//!
//! Capability metadata travels through `#[capability_input]` / `#[capability]` /
//! `#[capability_output]` annotations on the same Rust types and functions that
//! the wasm cdylib's `invoke` dispatcher calls into. The workspace binary
//! `runtara-agent-bundle-emit` reads these macro-emitted `&'static` statics on
//! the host architecture and writes `runtara_agent_transform.meta.json` next to
//! the `.wasm` — the JSON is a build artifact, never hand-edited.
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
#![allow(clippy::result_large_err)]

use runtara_agent_macro::{CapabilityInput, CapabilityOutput, capability};
use runtara_dsl::agent_meta::EnumVariants;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use strum::VariantNames;

#[cfg(target_arch = "wasm32")]
#[allow(warnings)]
mod bindings;

// -----------------------------------------------------------------------------
// Local AgentError shim — preserves the legacy `with_attr` chain shape so the
// macro-generated executor receives a JSON-string error that the host
// dispatcher can parse back into the WIT `ErrorInfo` record.
// -----------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct AgentError {
    pub code: String,
    pub message: String,
    pub category: &'static str,
    pub severity: &'static str,
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub attributes: HashMap<String, Value>,
}

impl AgentError {
    pub fn permanent(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            category: "permanent",
            severity: "error",
            attributes: HashMap::new(),
        }
    }

    pub fn with_attr(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.attributes
            .insert(key.into(), Value::String(value.into()));
        self
    }
}

impl From<AgentError> for String {
    fn from(err: AgentError) -> Self {
        serde_json::to_string(&err).unwrap_or_else(|_| format!("[{}] {}", err.code, err.message))
    }
}

// -----------------------------------------------------------------------------
// Helpers shared with input types
// -----------------------------------------------------------------------------

/// Custom deserializer that treats null as an empty Vec
fn deserialize_value_or_empty_vec<'de, D>(deserializer: D) -> Result<Vec<Value>, D::Error>
where
    D: Deserializer<'de>,
{
    let opt: Option<Vec<Value>> = Option::deserialize(deserializer)?;
    Ok(opt.unwrap_or_default())
}

fn default_ascending() -> bool {
    true
}

// -----------------------------------------------------------------------------
// Enums (with VariantNames + EnumVariants so the macro can record allowed values)
// -----------------------------------------------------------------------------

/// Condition for filtering array items
#[derive(Debug, Deserialize, Clone, PartialEq, VariantNames)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
#[strum(serialize_all = "SCREAMING_SNAKE_CASE")]
pub enum MatchCondition {
    Includes,
    Excludes,
    StartsWith,
    EndsWith,
    Contains,
}

impl EnumVariants for MatchCondition {
    fn variant_names() -> &'static [&'static str] {
        Self::VARIANTS
    }
}

// -----------------------------------------------------------------------------
// Input types
// -----------------------------------------------------------------------------

#[derive(Debug, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Extract Property Input")]
pub struct ExtractInput {
    #[field(
        display_name = "Input Array",
        description = "The array of objects to extract property values from",
        example = r#"[{"name": "Alice", "age": 30}, {"name": "Bob", "age": 25}]"#
    )]
    pub value: Vec<Value>,

    #[field(
        display_name = "Property Path",
        description = "The property path to extract from each item (JSONPath syntax)",
        example = "name"
    )]
    pub property_path: String,
}

#[derive(Debug, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Get Value Input")]
pub struct GetValueByPathInput {
    #[field(
        display_name = "Input Value",
        description = "The object to extract a property value from",
        example = r#"{"user": {"name": "Alice", "age": 30}}"#
    )]
    pub value: Option<Value>,

    #[field(
        display_name = "Property Path",
        description = "The property path to extract (JSONPath syntax)",
        example = "user.name"
    )]
    pub property_path: Option<String>,
}

#[derive(Debug, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Set Value Input")]
pub struct SetValueByPathInput {
    #[field(
        display_name = "Target Object",
        description = "The object to set a property value in",
        example = r#"{"user": {"name": "Alice"}}"#
    )]
    pub target: Option<Value>,

    #[field(
        display_name = "Property Path",
        description = "The property path to set (JSONPath syntax, creates nested objects if needed)",
        example = "user.age"
    )]
    pub property_path: Option<String>,

    #[field(
        display_name = "Value",
        description = "The value to set at the property path",
        example = "30"
    )]
    pub value: Option<Value>,
}

#[derive(Debug, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Filter Non-Values Input")]
pub struct FilterNoValueInput {
    #[field(
        display_name = "Input Array",
        description = "The array of items to filter",
        example = r#"[{"x": 1}, {"x": null}, {"x": ""}]"#
    )]
    pub value: Vec<Value>,

    #[field(
        display_name = "Property Path",
        description = "The property path to check in each item (if omitted, checks the item itself)",
        example = "x"
    )]
    #[serde(default)]
    pub property_path: Option<String>,

    #[field(
        display_name = "Filter Empty Strings",
        description = "Remove items where the value is an empty string (\"\")",
        example = "false",
        default = "false"
    )]
    #[serde(default)]
    pub filter_empty_strings: bool,

    #[field(
        display_name = "Filter Null Values",
        description = "Remove items where the value is null",
        example = "true",
        default = "false"
    )]
    #[serde(default)]
    pub filter_null_values: bool,

    #[field(
        display_name = "Filter Blank Strings",
        description = "Remove items where the value is a whitespace-only string",
        example = "false",
        default = "false"
    )]
    #[serde(default)]
    pub filter_blank_strings: bool,

    #[field(
        display_name = "Filter Zero Values",
        description = "Remove items where the value is 0 or \"0\"",
        example = "false",
        default = "false"
    )]
    #[serde(default)]
    pub filter_zero_values: bool,
}

#[derive(Debug, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Select First Input")]
pub struct SelectFirstInput {
    #[field(
        display_name = "Input Array",
        description = "The array to select the first truthy value from (skips null, empty strings, 0, false)",
        example = r#"[null, "", 0, "hello", "world"]"#
    )]
    pub value: Option<Vec<Value>>,
}

#[derive(Debug, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Coalesce Input")]
pub struct CoalesceInput {
    #[field(
        display_name = "Values",
        description = "Array of values to check; returns the first non-null/non-undefined value",
        example = r#"[null, 42, "fallback"]"#
    )]
    pub values: Vec<Value>,

    #[field(
        display_name = "Treat Empty String As Null",
        description = "If true, empty strings are treated as null and skipped",
        example = "false",
        default = "false"
    )]
    #[serde(default)]
    pub treat_empty_string_as_null: bool,

    #[field(
        display_name = "Treat Zero As Null",
        description = "If true, zero values (0, 0.0) are treated as null and skipped",
        example = "false",
        default = "false"
    )]
    #[serde(default)]
    pub treat_zero_as_null: bool,
}

#[derive(Debug, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Parse JSON Input")]
pub struct FromJsonStringInput {
    #[field(
        display_name = "JSON String",
        description = "The JSON string to parse into a value",
        example = r#""{\"name\":\"Alice\",\"age\":30}""#
    )]
    pub value: Option<String>,
}

#[derive(Debug, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Stringify JSON Input")]
pub struct ToJsonStringInput {
    #[field(
        display_name = "Input Value",
        description = "The value to serialize to a JSON string",
        example = r#"{"name": "Alice", "age": 30}"#
    )]
    pub value: Value,
}

#[derive(Debug, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Filter Array Input")]
pub struct FilterInput {
    #[field(
        display_name = "Input Array",
        description = "The array of items to filter",
        example = r#"[{"status": "active"}, {"status": "inactive"}]"#
    )]
    #[serde(default, deserialize_with = "deserialize_value_or_empty_vec")]
    pub value: Vec<Value>,

    #[field(
        display_name = "Property Path",
        description = "The property path to extract from each item (JSONPath syntax). Use \"$\" or \"\" to filter the array values directly.",
        example = "status"
    )]
    pub property_path: String,

    #[field(
        display_name = "Match Values",
        description = "The value(s) to compare against (single value or array)",
        example = r#"["active", "pending"]"#
    )]
    pub match_values: Value,

    #[field(
        display_name = "Match Condition",
        description = "Whether to include or exclude matching items",
        example = "INCLUDES",
        enum_type = "MatchCondition"
    )]
    pub match_condition: MatchCondition,
}

#[derive(Debug, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Sort Array Input")]
pub struct SortInput {
    #[field(
        display_name = "Input Array",
        description = "The array to sort",
        example = r#"[{"age": 35}, {"age": 30}, {"age": 25}]"#
    )]
    pub value: Vec<Value>,

    #[field(
        display_name = "Property Path",
        description = "The property path to sort by (if omitted, sorts the items directly)",
        example = "age"
    )]
    pub property_path: Option<String>,

    #[field(
        display_name = "Ascending Order",
        description = "Whether to sort in ascending order (true) or descending order (false)",
        example = "true",
        default = "true"
    )]
    #[serde(default = "default_ascending")]
    pub ascending: bool,
}

#[derive(Debug, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Map Fields Input")]
pub struct MapFieldsInput {
    #[field(
        display_name = "Source Data",
        description = "The source object containing data to map",
        example = r#"{"firstName": "Alice", "userAge": 30, "email": "alice@example.com"}"#
    )]
    pub source_data: Value,

    #[field(
        display_name = "Field Mappings",
        description = "Map of source field paths to target field names",
        example = r#"{"firstName": "name", "userAge": "age"}"#
    )]
    pub mappings: HashMap<String, String>,
}

#[derive(Debug, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Group By Input")]
pub struct GroupByInput {
    #[field(
        display_name = "Input Array",
        description = "The array of items to group",
        example = r#"[{"name": "Alice", "status": "active"}, {"name": "Bob", "status": "inactive"}]"#
    )]
    pub value: Value,

    #[field(
        display_name = "Group Key",
        description = "The property path to use as the grouping key (JSONPath syntax)",
        example = "status"
    )]
    pub key: String,

    #[field(
        display_name = "Return As Map",
        description = "Return grouped items as a map (key -> items) instead of an array of arrays",
        example = "true",
        default = "false"
    )]
    #[serde(default)]
    pub as_map: bool,
}

#[derive(Debug, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Append Input")]
pub struct AppendInput {
    #[field(
        display_name = "Array",
        description = "The array to append an item to (can contain objects or primitive values)",
        example = r#"[{"name": "Alice"}, {"name": "Bob"}]"#
    )]
    pub array: Vec<Value>,

    #[field(
        display_name = "Item",
        description = "The item to append to the array (can be an object or primitive value)",
        example = r#"{"name": "Charlie"}"#
    )]
    pub item: Value,
}

#[derive(Debug, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Flat Map Input")]
pub struct FlatMapInput {
    #[field(
        display_name = "Input Array",
        description = "The array of objects to flat map",
        example = r#"[{"items": [1, 2]}, {"items": [3, 4]}]"#
    )]
    pub value: Vec<Value>,

    #[field(
        display_name = "Property Path",
        description = "The property path to the nested array in each item (JSONPath syntax)",
        example = "items"
    )]
    pub property_path: String,
}

#[derive(Debug, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Array Length Input")]
pub struct ArrayLengthInput {
    #[field(
        display_name = "Value",
        description = "Array, string, or object to get the length/size of",
        example = r#"[1, 2, 3]"#
    )]
    pub value: Value,
}

#[derive(Debug, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Ensure Array Input")]
pub struct EnsureArrayInput {
    #[field(
        display_name = "Value",
        description = "Value to wrap in an array if not already an array. Arrays pass through unchanged, null becomes empty array.",
        example = r#"{"name": "Alice"}"#
    )]
    pub value: Value,
}

// -----------------------------------------------------------------------------
// Output types
// -----------------------------------------------------------------------------

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Extract Output")]
pub struct ExtractOutput {
    #[field(
        display_name = "Values",
        description = "Array of extracted property values"
    )]
    pub values: Vec<Value>,

    #[field(display_name = "Count", description = "Number of values extracted")]
    pub count: usize,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Filter Output")]
pub struct FilterOutput {
    #[field(
        display_name = "Items",
        description = "Array of items that passed the filter"
    )]
    pub items: Vec<Value>,

    #[field(
        display_name = "Count",
        description = "Number of items that passed the filter"
    )]
    pub count: usize,

    #[field(
        display_name = "Removed Count",
        description = "Number of items removed by the filter"
    )]
    pub removed_count: usize,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Sort Output")]
pub struct SortOutput {
    #[field(display_name = "Items", description = "Array of sorted items")]
    pub items: Vec<Value>,

    #[field(
        display_name = "Count",
        description = "Number of items in the sorted array"
    )]
    pub count: usize,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Group By Output")]
pub struct GroupByOutput {
    #[field(
        display_name = "Groups",
        description = "Grouped items - either a map (key -> items) or array of arrays"
    )]
    pub groups: Value,

    #[field(
        display_name = "Group Count",
        description = "Number of unique groups created"
    )]
    pub group_count: usize,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Map Fields Output")]
pub struct MapFieldsOutput {
    #[field(
        display_name = "Result",
        description = "Object with mapped field values"
    )]
    pub result: HashMap<String, Value>,

    #[field(
        display_name = "Field Count",
        description = "Number of fields successfully mapped"
    )]
    pub field_count: usize,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Append Output")]
pub struct AppendOutput {
    #[field(
        display_name = "Array",
        description = "Array with the new item appended"
    )]
    pub array: Vec<Value>,

    #[field(
        display_name = "Length",
        description = "New length of the array after appending"
    )]
    pub length: usize,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Flat Map Output")]
pub struct FlatMapOutput {
    #[field(
        display_name = "Items",
        description = "Flattened array of all nested items"
    )]
    pub items: Vec<Value>,

    #[field(
        display_name = "Count",
        description = "Total number of items in the flattened array"
    )]
    pub count: usize,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Array Length Output")]
pub struct ArrayLengthOutput {
    #[field(
        display_name = "Length",
        description = "Length of array/string, or number of keys in object"
    )]
    pub length: usize,

    #[field(
        display_name = "Is Array",
        description = "True if the value was an array"
    )]
    pub is_array: bool,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "JSON String Output")]
pub struct ToJsonStringOutput {
    #[field(display_name = "JSON", description = "The serialized JSON string")]
    pub json: String,

    #[field(
        display_name = "Length",
        description = "Length of the JSON string in characters"
    )]
    pub length: usize,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Ensure Array Output")]
pub struct EnsureArrayOutput {
    #[field(
        display_name = "Items",
        description = "The input wrapped as an array (or the original array if already an array)"
    )]
    pub items: Vec<Value>,

    #[field(
        display_name = "Count",
        description = "Number of items in the resulting array"
    )]
    pub count: usize,

    #[field(
        display_name = "Was Array",
        description = "True if the input was already an array, false if it was wrapped"
    )]
    pub was_array: bool,
}

// -----------------------------------------------------------------------------
// Capabilities — annotated for metadata; the `__executor_*` fns the macro emits
// are what the wasm Guest impl dispatches to.
// -----------------------------------------------------------------------------

/// Extracts values from an array of objects based on a property path
#[capability(
    module = "transform",
    module_display_name = "Transform",
    module_description = "Transform capabilities for data manipulation, filtering, sorting, and JSON operations",
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

            if input.filter_null_values && property_value.is_null() {
                return false;
            }

            if input.filter_empty_strings
                && let Some(s) = property_value.as_str()
                && s.is_empty()
            {
                return false;
            }

            if input.filter_blank_strings
                && let Some(s) = property_value.as_str()
                && s.trim().is_empty()
            {
                return false;
            }

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
        if item.is_null() {
            continue;
        }

        if let Some(s) = item.as_str()
            && (s.is_empty() || s.trim().is_empty())
        {
            continue;
        }

        if let Some(n) = item.as_f64()
            && n == 0.0
        {
            continue;
        }

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
        if value.is_null() {
            continue;
        }

        if input.treat_empty_string_as_null && value.as_str().is_some_and(|s| s.is_empty()) {
            continue;
        }

        if input.treat_zero_as_null && value.as_f64().is_some_and(|n| n == 0.0) {
            continue;
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
            _ => serde_json::to_string(&key_value).map_err(|e| {
                AgentError::permanent(
                    "TRANSFORM_KEY_SERIALIZE_ERROR",
                    format!("Failed to serialize group key to string: {}", e),
                )
            })?,
        };

        grouped.entry(key_str).or_default().push(item.clone());
    }

    let group_count = grouped.len();
    if input.as_map {
        let mut map = serde_json::Map::new();
        for (key, values) in grouped {
            map.insert(key, Value::Array(values));
        }
        Ok(GroupByOutput {
            groups: Value::Object(map),
            group_count,
        })
    } else {
        let arrays: Vec<Value> = grouped.into_values().map(Value::Array).collect();
        Ok(GroupByOutput {
            groups: Value::Array(arrays),
            group_count,
        })
    }
}

/// Appends an item to an array
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

/// Ensures a value is an array, wrapping non-array values in a single-element array
#[capability(
    module = "transform",
    display_name = "Ensure Array",
    description = "Ensure a value is an array. Arrays pass through unchanged, null becomes empty array, other values are wrapped in a single-element array."
)]
pub fn ensure_array(input: EnsureArrayInput) -> Result<EnsureArrayOutput, String> {
    let (items, was_array) = match input.value {
        Value::Array(arr) => (arr, true),
        Value::Null => (vec![], false),
        other => (vec![other], false),
    };

    let count = items.len();
    Ok(EnsureArrayOutput {
        items,
        count,
        was_array,
    })
}

// -----------------------------------------------------------------------------
// Helper functions (mirror runtara-agents/src/agents/transform.rs)
// -----------------------------------------------------------------------------

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

// -----------------------------------------------------------------------------
// AgentInfo assembler (host-only; the wasm binary doesn't need it)
// -----------------------------------------------------------------------------

/// Build the canonical `AgentInfo` for this agent by walking the macro-emitted
/// `&'static` statics. The workspace `runtara-agent-bundle-emit` binary calls
/// this on the host architecture and writes the JSON to disk; the wasm binary
/// itself never executes this code, so we cfg-gate it out to keep the
/// component small.
#[cfg(not(target_arch = "wasm32"))]
pub fn agent_info() -> runtara_dsl::agent_meta::AgentInfo {
    use runtara_dsl::agent_meta::{
        AgentInfo, CapabilityMeta, InputTypeMeta, OutputTypeMeta, capability_to_api,
    };
    use std::collections::HashMap;

    let caps: &[&'static CapabilityMeta] = &[
        &__CAPABILITY_META_EXTRACT,
        &__CAPABILITY_META_GET_VALUE_BY_PATH,
        &__CAPABILITY_META_SET_VALUE_BY_PATH,
        &__CAPABILITY_META_FILTER_NON_VALUES,
        &__CAPABILITY_META_SELECT_FIRST,
        &__CAPABILITY_META_COALESCE,
        &__CAPABILITY_META_FROM_JSON_STRING,
        &__CAPABILITY_META_TO_JSON_STRING,
        &__CAPABILITY_META_FILTER,
        &__CAPABILITY_META_SORT,
        &__CAPABILITY_META_MAP_FIELDS,
        &__CAPABILITY_META_GROUP_BY,
        &__CAPABILITY_META_APPEND,
        &__CAPABILITY_META_FLAT_MAP,
        &__CAPABILITY_META_ARRAY_LENGTH,
        &__CAPABILITY_META_ENSURE_ARRAY,
    ];
    let input_types: HashMap<&'static str, &'static InputTypeMeta> = [
        ("ExtractInput", &__INPUT_META_ExtractInput as &InputTypeMeta),
        ("GetValueByPathInput", &__INPUT_META_GetValueByPathInput),
        ("SetValueByPathInput", &__INPUT_META_SetValueByPathInput),
        ("FilterNoValueInput", &__INPUT_META_FilterNoValueInput),
        ("SelectFirstInput", &__INPUT_META_SelectFirstInput),
        ("CoalesceInput", &__INPUT_META_CoalesceInput),
        ("FromJsonStringInput", &__INPUT_META_FromJsonStringInput),
        ("ToJsonStringInput", &__INPUT_META_ToJsonStringInput),
        ("FilterInput", &__INPUT_META_FilterInput),
        ("SortInput", &__INPUT_META_SortInput),
        ("MapFieldsInput", &__INPUT_META_MapFieldsInput),
        ("GroupByInput", &__INPUT_META_GroupByInput),
        ("AppendInput", &__INPUT_META_AppendInput),
        ("FlatMapInput", &__INPUT_META_FlatMapInput),
        ("ArrayLengthInput", &__INPUT_META_ArrayLengthInput),
        ("EnsureArrayInput", &__INPUT_META_EnsureArrayInput),
    ]
    .into_iter()
    .collect();
    let output_types: HashMap<&'static str, &'static OutputTypeMeta> = [
        (
            "ExtractOutput",
            &__OUTPUT_META_ExtractOutput as &OutputTypeMeta,
        ),
        ("FilterOutput", &__OUTPUT_META_FilterOutput),
        ("SortOutput", &__OUTPUT_META_SortOutput),
        ("GroupByOutput", &__OUTPUT_META_GroupByOutput),
        ("MapFieldsOutput", &__OUTPUT_META_MapFieldsOutput),
        ("AppendOutput", &__OUTPUT_META_AppendOutput),
        ("FlatMapOutput", &__OUTPUT_META_FlatMapOutput),
        ("ArrayLengthOutput", &__OUTPUT_META_ArrayLengthOutput),
        ("ToJsonStringOutput", &__OUTPUT_META_ToJsonStringOutput),
        ("EnsureArrayOutput", &__OUTPUT_META_EnsureArrayOutput),
    ]
    .into_iter()
    .collect();

    let capabilities = caps
        .iter()
        .map(|cap| {
            capability_to_api(
                cap,
                input_types.get(cap.input_type).copied(),
                output_types.get(cap.output_type).copied(),
            )
        })
        .collect();

    AgentInfo {
        id: "transform".into(),
        name: "Transform".into(),
        description:
            "Transform capabilities for data manipulation, filtering, sorting, and JSON operations"
                .into(),
        has_side_effects: false,
        supports_connections: false,
        integration_ids: vec![],
        capabilities,
    }
}

// -----------------------------------------------------------------------------
// Wasm component plumbing
// -----------------------------------------------------------------------------

#[cfg(target_arch = "wasm32")]
use bindings::exports::runtara::agent::capabilities::{ConnectionInfo, ErrorInfo, Guest};

#[cfg(target_arch = "wasm32")]
struct Component;

#[cfg(target_arch = "wasm32")]
impl Guest for Component {
    fn invoke(
        capability_id: String,
        input: Vec<u8>,
        _connection: Option<ConnectionInfo>,
    ) -> Result<Vec<u8>, ErrorInfo> {
        let value: serde_json::Value = serde_json::from_slice(&input).map_err(bad_json)?;
        let executor_result = match capability_id.as_str() {
            "extract" => __executor_extract(value),
            "get-value-by-path" => __executor_get_value_by_path(value),
            "set-value-by-path" => __executor_set_value_by_path(value),
            "filter-non-values" => __executor_filter_non_values(value),
            "select-first" => __executor_select_first(value),
            "coalesce" => __executor_coalesce(value),
            "from-json-string" => __executor_from_json_string(value),
            "to-json-string" => __executor_to_json_string(value),
            "filter" => __executor_filter(value),
            "sort" => __executor_sort(value),
            "map-fields" => __executor_map_fields(value),
            "group-by" => __executor_group_by(value),
            "append" => __executor_append(value),
            "flat-map" => __executor_flat_map(value),
            "array-length" => __executor_array_length(value),
            "ensure-array" => __executor_ensure_array(value),
            other => {
                return Err(ErrorInfo {
                    code: "UNKNOWN_CAPABILITY".into(),
                    message: format!("transform agent has no capability `{other}`"),
                    category: "permanent".into(),
                    severity: "error".into(),
                    retryable: false,
                    retry_after_ms: None,
                    attributes: None,
                });
            }
        };
        executor_result
            .map_err(error_string_to_error_info)
            .and_then(|out_value| serde_json::to_vec(&out_value).map_err(bad_json))
    }
}

#[cfg(target_arch = "wasm32")]
fn bad_json(e: serde_json::Error) -> ErrorInfo {
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

/// The `#[capability]` macro packages each error as a JSON-string with
/// `{ code, message, category, severity }`. Parse it back into a typed
/// `ErrorInfo` for the WIT result.
#[cfg(target_arch = "wasm32")]
fn error_string_to_error_info(s: String) -> ErrorInfo {
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(&s) {
        ErrorInfo {
            code: value
                .get("code")
                .and_then(|v| v.as_str())
                .unwrap_or("CAPABILITY_ERROR")
                .into(),
            message: value
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or(&s)
                .into(),
            category: value
                .get("category")
                .and_then(|v| v.as_str())
                .unwrap_or("permanent")
                .into(),
            severity: value
                .get("severity")
                .and_then(|v| v.as_str())
                .unwrap_or("error")
                .into(),
            retryable: value
                .get("retryable")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
            retry_after_ms: value.get("retry_after_ms").and_then(|v| v.as_u64()),
            attributes: value.get("attributes").map(|v| v.to_string()),
        }
    } else {
        ErrorInfo {
            code: "CAPABILITY_ERROR".into(),
            message: s,
            category: "permanent".into(),
            severity: "error".into(),
            retryable: false,
            retry_after_ms: None,
            attributes: None,
        }
    }
}

#[cfg(target_arch = "wasm32")]
bindings::export!(Component with_types_in bindings);
