//! Input Schema Validation
//!
//! Validates workflow execution inputs against the DSL input schema.
//! Handles conversion from the DSL flat-map schema format to standard
//! JSON Schema before validation with the `jsonschema` crate.

use serde_json::Value;

/// Check if a schema is empty (no fields defined).
pub fn is_empty_schema(schema: &Value) -> bool {
    schema.as_object().map(|o| o.is_empty()).unwrap_or(true)
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
            // but is not a valid JSON Schema type — map it to "object".
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

/// Validate inputs against a schema using the jsonschema crate.
/// Accepts both DSL flat-map format and standard JSON Schema.
pub fn validate_inputs(inputs: &Value, schema: &Value) -> Result<(), String> {
    let json_schema = dsl_schema_to_json_schema(schema);
    let validator =
        jsonschema::validator_for(&json_schema).map_err(|e| format!("Invalid schema: {}", e))?;

    let errors: Vec<String> = validator
        .iter_errors(inputs)
        .map(|e| {
            let path = e.instance_path.to_string();
            if path.is_empty() {
                e.to_string()
            } else {
                format!("{}: {}", path, e)
            }
        })
        .collect();

    if !errors.is_empty() {
        return Err(errors.join("; "));
    }

    Ok(())
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
