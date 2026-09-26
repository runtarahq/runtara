//! Workflow start input validation.
//!
//! Validates workflow execution inputs against the DSL input schema.
//! Handles both the DSL flat-map schema format and the standard JSON Schema
//! shape emitted by the workflow editor.

use serde_json::Value;

/// Error returned when workflow start inputs are not in canonical format or do
/// not satisfy the workflow input schema.
#[derive(Debug, Clone, PartialEq)]
pub struct WorkflowInputValidationError {
    /// Human-readable validation failure message.
    pub message: String,
}

impl std::fmt::Display for WorkflowInputValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for WorkflowInputValidationError {}

/// Check if a schema is empty (no fields defined).
pub fn is_empty_schema(schema: &Value) -> bool {
    schema.as_object().map(|o| o.is_empty()).unwrap_or(true)
}

/// Validates workflow inputs match the canonical Runtara format:
/// `{"data": {...}, "variables": {...}}`
///
/// This function enforces strict input format at the API boundary.
/// Callers must provide properly structured inputs - no auto-wrapping is performed.
///
/// # Required format:
/// - Must be a JSON object
/// - Must have a "data" key (value can be any JSON type)
/// - "variables" key is optional (defaults to empty object if missing)
///
/// # Returns:
/// - `Ok(Value)` with the validated inputs (with "variables" added if missing)
/// - `Err(WorkflowInputValidationError)` if format is invalid
///
/// # Example valid inputs:
/// ```json
/// {"data": {"foo": "bar"}, "variables": {"x": 1}}
/// {"data": {"foo": "bar"}}
/// {"data": null}
/// {"data": [1, 2, 3]}
/// ```
///
/// # Example invalid inputs:
/// ```json
/// {"foo": "bar"}
/// [1, 2, 3]
/// null
/// ```
pub fn validate_workflow_inputs(inputs: Value) -> Result<Value, WorkflowInputValidationError> {
    // Must be an object
    let obj = match inputs.as_object() {
        Some(o) => o,
        None => {
            return Err(WorkflowInputValidationError {
                message: "inputs must be a JSON object with 'data' key, e.g. {\"data\": {...}, \"variables\": {...}}".to_string(),
            });
        }
    };

    // Must have "data" key
    if !obj.contains_key("data") {
        return Err(WorkflowInputValidationError {
            message: "inputs must contain 'data' key, e.g. {\"data\": {...}, \"variables\": {...}}"
                .to_string(),
        });
    }

    // Add "variables" if missing
    let mut result = inputs;
    if result.get("variables").is_none()
        && let serde_json::Value::Object(ref mut map) = result
    {
        map.insert(
            "variables".to_string(),
            serde_json::Value::Object(serde_json::Map::new()),
        );
    }

    Ok(result)
}

/// Validate canonical workflow start inputs against the workflow input schema.
///
/// The same function is used by backend execution paths and browser WASM
/// validation. It first validates the canonical `{"data", "variables"}`
/// envelope, then validates `data` against `input_schema` when the schema has
/// fields.
pub fn validate_workflow_start_inputs(
    inputs: Value,
    input_schema: &Value,
) -> Result<Value, WorkflowInputValidationError> {
    let validated_inputs = validate_workflow_inputs(inputs)?;

    if !is_empty_schema(input_schema) {
        let data_to_validate = validated_inputs
            .get("data")
            .cloned()
            .unwrap_or(serde_json::json!({}));
        validate_inputs(&data_to_validate, input_schema).map_err(|e| {
            WorkflowInputValidationError {
                message: format!("Input validation failed: {}", e),
            }
        })?;
    }

    Ok(validated_inputs)
}

/// Convert DSL flat-map schema to standard JSON Schema for validation.
///
/// DSL format:  `{"field_name": {"type": "string", "required": true, ...}}`
/// JSON Schema: `{"type": "object", "properties": {"field_name": {"type": "string"}}, "required": ["field_name"]}`
///
/// If the schema already looks like standard JSON Schema (has a "properties" key),
/// it is returned as-is.
fn dsl_schema_to_json_schema(schema: &Value) -> Value {
    let obj = match schema.as_object() {
        Some(o) => o,
        None => return schema.clone(),
    };

    // If root already has "properties", treat as standard JSON Schema
    if obj.contains_key("properties") {
        return schema.clone();
    }

    let mut properties = serde_json::Map::new();
    let mut required = Vec::new();

    for (field_name, field_def) in obj {
        let field_obj = match field_def.as_object() {
            Some(o) => o,
            None => continue,
        };

        let mut prop = serde_json::Map::new();
        if let Some(t) = field_obj.get("type") {
            // Map DSL-specific types to valid JSON Schema types.
            // "file" is a DSL type representing a FileData object (content, filename, mimeType)
            // but is not a valid JSON Schema type - map it to "object".
            let json_type = match t.as_str() {
                Some("file") => Value::String("object".to_string()),
                _ => t.clone(),
            };
            prop.insert("type".to_string(), json_type);
        }
        if let Some(desc) = field_obj.get("description") {
            prop.insert("description".to_string(), desc.clone());
        }
        if let Some(default) = field_obj.get("default") {
            prop.insert("default".to_string(), default.clone());
        }
        if let Some(items) = field_obj.get("items") {
            prop.insert("items".to_string(), items.clone());
        }

        properties.insert(field_name.clone(), Value::Object(prop));

        if field_obj
            .get("required")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        {
            required.push(Value::String(field_name.clone()));
        }
    }

    let mut json_schema = serde_json::Map::new();
    json_schema.insert("type".to_string(), Value::String("object".to_string()));
    json_schema.insert("properties".to_string(), Value::Object(properties));
    if !required.is_empty() {
        json_schema.insert("required".to_string(), Value::Array(required));
    }

    Value::Object(json_schema)
}

/// Validate inputs against a workflow input schema.
///
/// Accepts both DSL flat-map format and the standard JSON Schema object shape
/// used by the workflow editor. This intentionally covers the schema surface
/// Runtara emits for workflow start parameters instead of pulling in a full
/// remote-reference JSON Schema engine.
pub fn validate_inputs(inputs: &Value, schema: &Value) -> Result<(), String> {
    let json_schema = dsl_schema_to_json_schema(schema);
    let mut errors = Vec::new();
    validate_value(inputs, &json_schema, "", &mut errors);

    if !errors.is_empty() {
        return Err(errors.join("; "));
    }

    Ok(())
}

fn validate_value(value: &Value, schema: &Value, path: &str, errors: &mut Vec<String>) {
    let Some(schema_obj) = schema.as_object() else {
        return;
    };

    if let Some(enum_values) = schema_obj.get("enum").and_then(Value::as_array)
        && !enum_values.iter().any(|enum_value| enum_value == value)
    {
        errors.push(format!(
            "{} must be one of the allowed values",
            display_path(path)
        ));
    }

    if let Some(type_value) = schema_obj.get("type")
        && !matches_schema_type(value, type_value)
    {
        errors.push(format!(
            "{} must be {}",
            display_path(path),
            schema_type_description(type_value)
        ));
        return;
    }

    if let Some(required) = schema_obj.get("required").and_then(Value::as_array)
        && let Some(object) = value.as_object()
    {
        for field in required.iter().filter_map(Value::as_str) {
            if !object.contains_key(field) {
                errors.push(format!("{} is required", join_path(path, field)));
            }
        }
    }

    if let Some(properties) = schema_obj.get("properties").and_then(Value::as_object)
        && let Some(object) = value.as_object()
    {
        for (field, field_schema) in properties {
            if let Some(field_value) = object.get(field) {
                validate_value(field_value, field_schema, &join_path(path, field), errors);
            }
        }
    }

    if let Some(items_schema) = schema_obj.get("items")
        && let Some(items) = value.as_array()
    {
        for (index, item) in items.iter().enumerate() {
            validate_value(item, items_schema, &format!("{path}[{index}]"), errors);
        }
    }
}

fn matches_schema_type(value: &Value, type_value: &Value) -> bool {
    match type_value {
        Value::String(type_name) => matches_single_schema_type(value, type_name),
        Value::Array(types) => types
            .iter()
            .filter_map(Value::as_str)
            .any(|type_name| matches_single_schema_type(value, type_name)),
        _ => true,
    }
}

fn matches_single_schema_type(value: &Value, type_name: &str) -> bool {
    match type_name {
        "array" => value.is_array(),
        "boolean" => value.is_boolean(),
        "integer" => value.as_i64().is_some() || value.as_u64().is_some(),
        "number" => value.is_number(),
        "object" => value.is_object(),
        "string" => value.is_string(),
        "null" => value.is_null(),
        _ => true,
    }
}

fn schema_type_description(type_value: &Value) -> String {
    match type_value {
        Value::String(type_name) => format!("a {type_name}"),
        Value::Array(types) => {
            let names: Vec<&str> = types.iter().filter_map(Value::as_str).collect();
            if names.is_empty() {
                "a valid value".to_string()
            } else {
                format!("one of: {}", names.join(", "))
            }
        }
        _ => "a valid value".to_string(),
    }
}

fn join_path(parent: &str, field: &str) -> String {
    if parent.is_empty() {
        field.to_string()
    } else {
        format!("{parent}.{field}")
    }
}

fn display_path(path: &str) -> &str {
    if path.is_empty() { "input" } else { path }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // =========================================================================
    // is_empty_schema tests
    // =========================================================================

    #[test]
    fn test_empty_object_is_empty() {
        assert!(is_empty_schema(&json!({})));
    }

    #[test]
    fn test_null_is_empty() {
        assert!(is_empty_schema(&Value::Null));
    }

    #[test]
    fn test_non_empty_object_is_not_empty() {
        let schema = json!({
            "name": { "type": "string", "required": true }
        });
        assert!(!is_empty_schema(&schema));
    }

    #[test]
    fn test_validate_workflow_inputs_adds_missing_variables() {
        let input = json!({
            "data": {"foo": "bar"}
        });

        let result = validate_workflow_inputs(input).unwrap();

        assert_eq!(result["data"]["foo"], "bar");
        assert!(result["variables"].is_object());
        assert!(result["variables"].as_object().unwrap().is_empty());
    }

    #[test]
    fn test_validate_workflow_inputs_rejects_flat_object() {
        let input = json!({"foo": "bar", "count": 42});

        let err = validate_workflow_inputs(input).unwrap_err();

        assert!(err.message.contains("data"));
    }

    #[test]
    fn test_validate_workflow_start_inputs_validates_data_against_schema() {
        let schema = json!({
            "count": { "type": "integer", "required": true }
        });
        let input = json!({
            "data": { "count": 42 }
        });

        let result = validate_workflow_start_inputs(input, &schema).unwrap();

        assert_eq!(result["data"]["count"], 42);
        assert!(result["variables"].is_object());
    }

    #[test]
    fn test_validate_workflow_start_inputs_rejects_schema_mismatch() {
        let schema = json!({
            "count": { "type": "integer", "required": true }
        });
        let input = json!({
            "data": { "count": "not-a-number" },
            "variables": {}
        });

        let err = validate_workflow_start_inputs(input, &schema).unwrap_err();

        assert!(err.message.contains("Input validation failed"));
        assert!(err.message.contains("count"));
    }

    // =========================================================================
    // dsl_schema_to_json_schema tests
    // =========================================================================

    #[test]
    fn test_converts_dsl_flat_map_to_json_schema() {
        let dsl = json!({
            "tags": { "type": "array", "required": true, "items": { "type": "string" } },
            "metadata": { "type": "object", "required": true }
        });

        let result = dsl_schema_to_json_schema(&dsl);
        assert_eq!(result["type"], "object");
        assert_eq!(result["properties"]["tags"]["type"], "array");
        assert_eq!(result["properties"]["tags"]["items"]["type"], "string");
        assert_eq!(result["properties"]["metadata"]["type"], "object");

        let req = result["required"].as_array().unwrap();
        let req_strings: Vec<&str> = req.iter().map(|v| v.as_str().unwrap()).collect();
        assert!(req_strings.contains(&"tags"));
        assert!(req_strings.contains(&"metadata"));
    }

    #[test]
    fn test_passes_through_standard_json_schema() {
        let schema = json!({
            "type": "object",
            "properties": {
                "name": { "type": "string" }
            },
            "required": ["name"]
        });

        let result = dsl_schema_to_json_schema(&schema);
        assert_eq!(result, schema);
    }

    #[test]
    fn test_optional_fields_excluded_from_required() {
        let dsl = json!({
            "name": { "type": "string", "required": true },
            "notes": { "type": "string", "required": false }
        });

        let result = dsl_schema_to_json_schema(&dsl);
        let req = result["required"].as_array().unwrap();
        assert_eq!(req.len(), 1);
        assert_eq!(req[0], "name");
    }

    #[test]
    fn test_no_required_fields_omits_required_array() {
        let dsl = json!({
            "notes": { "type": "string", "required": false }
        });

        let result = dsl_schema_to_json_schema(&dsl);
        assert!(result.get("required").is_none());
    }

    #[test]
    fn test_preserves_description_and_default() {
        let dsl = json!({
            "count": {
                "type": "integer",
                "required": true,
                "description": "Item count",
                "default": 10
            }
        });

        let result = dsl_schema_to_json_schema(&dsl);
        assert_eq!(result["properties"]["count"]["description"], "Item count");
        assert_eq!(result["properties"]["count"]["default"], 10);
    }

    // =========================================================================
    // validate_inputs tests (DSL format)
    // =========================================================================

    #[test]
    fn test_validate_dsl_schema_valid_data() {
        let schema = json!({
            "tags": { "type": "array", "required": true, "items": { "type": "string" } },
            "metadata": { "type": "object", "required": true }
        });
        let inputs = json!({
            "tags": ["a", "b"],
            "metadata": { "key": "value" }
        });

        assert!(validate_inputs(&inputs, &schema).is_ok());
    }

    #[test]
    fn test_validate_dsl_schema_wrong_type() {
        let schema = json!({
            "tags": { "type": "array", "required": true },
            "metadata": { "type": "object", "required": true }
        });
        let inputs = json!({
            "tags": "not-an-array",
            "metadata": 12345
        });

        let result = validate_inputs(&inputs, &schema);
        assert!(result.is_err());
        let error = result.unwrap_err();
        assert!(error.contains("tags") || error.contains("array") || error.contains("type"));
    }

    #[test]
    fn test_validate_dsl_schema_missing_required() {
        let schema = json!({
            "name": { "type": "string", "required": true }
        });
        let inputs = json!({});

        let result = validate_inputs(&inputs, &schema);
        assert!(result.is_err());
        let error = result.unwrap_err();
        assert!(error.contains("name") || error.contains("required"));
    }

    // =========================================================================
    // validate_inputs tests (standard JSON Schema)
    // =========================================================================

    #[test]
    fn test_validate_json_schema_valid_data() {
        let schema = json!({
            "type": "object",
            "properties": {
                "name": { "type": "string" },
                "count": { "type": "integer" }
            }
        });
        let inputs = json!({
            "name": "test",
            "count": 42
        });

        assert!(validate_inputs(&inputs, &schema).is_ok());
    }

    #[test]
    fn test_validate_json_schema_missing_required() {
        let schema = json!({
            "type": "object",
            "properties": {
                "name": { "type": "string" }
            },
            "required": ["name"]
        });
        let inputs = json!({});

        let result = validate_inputs(&inputs, &schema);
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_json_schema_wrong_type() {
        let schema = json!({
            "type": "object",
            "properties": {
                "count": { "type": "integer" }
            }
        });
        let inputs = json!({
            "count": "not a number"
        });

        let result = validate_inputs(&inputs, &schema);
        assert!(result.is_err());
    }

    // =========================================================================
    // SYN-110: Required field null/empty validation tests
    // =========================================================================

    #[test]
    fn test_null_value_rejected_for_required_string() {
        let schema = json!({
            "name": { "type": "string", "required": true }
        });
        let inputs = json!({ "name": null });
        assert!(validate_inputs(&inputs, &schema).is_err());
    }

    #[test]
    fn test_empty_string_accepted_for_required_string() {
        let schema = json!({
            "name": { "type": "string", "required": true }
        });
        let inputs = json!({ "name": "" });
        assert!(validate_inputs(&inputs, &schema).is_ok());
    }

    #[test]
    fn test_zero_accepted_for_required_number() {
        let schema = json!({
            "count": { "type": "number", "required": true }
        });
        let inputs = json!({ "count": 0 });
        assert!(validate_inputs(&inputs, &schema).is_ok());
    }

    #[test]
    fn test_zero_accepted_for_required_integer() {
        let schema = json!({
            "count": { "type": "integer", "required": true }
        });
        let inputs = json!({ "count": 0 });
        assert!(validate_inputs(&inputs, &schema).is_ok());
    }

    #[test]
    fn test_null_rejected_for_required_number() {
        let schema = json!({
            "count": { "type": "number", "required": true }
        });
        let inputs = json!({ "count": null });
        assert!(validate_inputs(&inputs, &schema).is_err());
    }

    #[test]
    fn test_null_rejected_for_required_integer() {
        let schema = json!({
            "count": { "type": "integer", "required": true }
        });
        let inputs = json!({ "count": null });
        assert!(validate_inputs(&inputs, &schema).is_err());
    }

    #[test]
    fn test_empty_array_accepted_for_required_array() {
        let schema = json!({
            "tags": { "type": "array", "required": true }
        });
        let inputs = json!({ "tags": [] });
        assert!(validate_inputs(&inputs, &schema).is_ok());
    }

    #[test]
    fn test_null_array_rejected() {
        let schema = json!({
            "tags": { "type": "array", "required": true }
        });
        let inputs = json!({ "tags": null });
        assert!(validate_inputs(&inputs, &schema).is_err());
    }

    #[test]
    fn test_empty_object_accepted_for_required_object() {
        let schema = json!({
            "meta": { "type": "object", "required": true }
        });
        let inputs = json!({ "meta": {} });
        assert!(validate_inputs(&inputs, &schema).is_ok());
    }

    #[test]
    fn test_null_object_rejected() {
        let schema = json!({
            "meta": { "type": "object", "required": true }
        });
        let inputs = json!({ "meta": null });
        assert!(validate_inputs(&inputs, &schema).is_err());
    }

    #[test]
    fn test_false_accepted_for_required_boolean() {
        let schema = json!({
            "flag": { "type": "boolean", "required": true }
        });
        let inputs = json!({ "flag": false });
        assert!(validate_inputs(&inputs, &schema).is_ok());
    }

    #[test]
    fn test_null_boolean_rejected() {
        let schema = json!({
            "flag": { "type": "boolean", "required": true }
        });
        let inputs = json!({ "flag": null });
        assert!(validate_inputs(&inputs, &schema).is_err());
    }

    #[test]
    fn test_optional_field_can_be_omitted() {
        let schema = json!({
            "name": { "type": "string", "required": true },
            "notes": { "type": "string", "required": false }
        });
        let inputs = json!({ "name": "x" });
        assert!(validate_inputs(&inputs, &schema).is_ok());
    }

    // =========================================================================
    // SYN-241: File type input validation
    // =========================================================================

    #[test]
    fn test_file_type_accepts_file_data_object() {
        let schema = json!({
            "document": { "type": "file", "required": true }
        });
        let inputs = json!({
            "document": {
                "content": "aGVsbG8=",
                "filename": "test.txt",
                "mimeType": "text/plain"
            }
        });
        assert!(validate_inputs(&inputs, &schema).is_ok());
    }

    #[test]
    fn test_file_type_rejects_non_object() {
        let schema = json!({
            "document": { "type": "file", "required": true }
        });
        let inputs = json!({
            "document": "not an object"
        });
        assert!(validate_inputs(&inputs, &schema).is_err());
    }

    #[test]
    fn test_file_type_required_rejects_missing() {
        let schema = json!({
            "document": { "type": "file", "required": true }
        });
        let inputs = json!({});
        assert!(validate_inputs(&inputs, &schema).is_err());
    }

    #[test]
    fn test_mixed_required_optional_missing_required() {
        let schema = json!({
            "name": { "type": "string", "required": true },
            "notes": { "type": "string", "required": false }
        });
        let inputs = json!({ "notes": "x" });
        assert!(validate_inputs(&inputs, &schema).is_err());
    }
}
