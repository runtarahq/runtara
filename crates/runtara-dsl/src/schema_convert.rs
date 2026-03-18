// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Convert DSL flat-map schema (`HashMap<String, SchemaField>`) to standard JSON Schema.
//!
//! This is used by:
//! - `smo-runtime` for input validation at execution time
//! - `runtara-workflows` codegen for AI Agent structured output (`response_format`)

use std::collections::HashMap;

use serde_json::Value;

use crate::{SchemaField, SchemaFieldType};

/// Convert a single `SchemaField` to a JSON Schema property object.
fn schema_field_to_json_schema(field: &SchemaField) -> Value {
    let mut prop = serde_json::Map::new();

    let type_str = match field.field_type {
        SchemaFieldType::String => "string",
        SchemaFieldType::Integer => "integer",
        SchemaFieldType::Number => "number",
        SchemaFieldType::Boolean => "boolean",
        SchemaFieldType::Array => "array",
        SchemaFieldType::Object => "object",
        SchemaFieldType::File => "object",
    };
    prop.insert("type".to_string(), Value::String(type_str.to_string()));

    if let Some(ref desc) = field.description {
        prop.insert("description".to_string(), Value::String(desc.clone()));
    }

    if let Some(ref default) = field.default {
        prop.insert("default".to_string(), default.clone());
    }

    if let Some(ref items) = field.items {
        prop.insert("items".to_string(), schema_field_to_json_schema(items));
    }

    if let Some(ref enum_values) = field.enum_values {
        prop.insert("enum".to_string(), Value::Array(enum_values.clone()));
    }

    // Forward validation constraints to JSON Schema
    if let Some(min) = field.min {
        match field.field_type {
            SchemaFieldType::String => {
                prop.insert(
                    "minLength".to_string(),
                    Value::Number(serde_json::Number::from(min as u64)),
                );
            }
            SchemaFieldType::Integer | SchemaFieldType::Number => {
                prop.insert("minimum".to_string(), serde_json::json!(min));
            }
            _ => {}
        }
    }

    if let Some(max) = field.max {
        match field.field_type {
            SchemaFieldType::String => {
                prop.insert(
                    "maxLength".to_string(),
                    Value::Number(serde_json::Number::from(max as u64)),
                );
            }
            SchemaFieldType::Integer | SchemaFieldType::Number => {
                prop.insert("maximum".to_string(), serde_json::json!(max));
            }
            _ => {}
        }
    }

    if let Some(ref pattern) = field.pattern {
        if field.field_type == SchemaFieldType::String {
            prop.insert("pattern".to_string(), Value::String(pattern.clone()));
        }
    }

    // Recurse into object properties
    if let Some(ref properties) = field.properties {
        if field.field_type == SchemaFieldType::Object {
            let nested = dsl_schema_to_json_schema(properties);
            if let Value::Object(nested_obj) = nested {
                if let Some(props) = nested_obj.get("properties") {
                    prop.insert("properties".to_string(), props.clone());
                }
                if let Some(req) = nested_obj.get("required") {
                    prop.insert("required".to_string(), req.clone());
                }
                if let Some(ap) = nested_obj.get("additionalProperties") {
                    prop.insert("additionalProperties".to_string(), ap.clone());
                }
            }
        }
    }

    Value::Object(prop)
}

/// Convert a DSL flat-map schema to standard JSON Schema.
///
/// Input (DSL format):
/// ```json
/// {
///   "sentiment": { "type": "string", "required": true, "enum": ["positive", "negative"] },
///   "score": { "type": "number", "required": true }
/// }
/// ```
///
/// Output (JSON Schema):
/// ```json
/// {
///   "type": "object",
///   "properties": {
///     "sentiment": { "type": "string", "enum": ["positive", "negative"] },
///     "score": { "type": "number" }
///   },
///   "required": ["sentiment", "score"],
///   "additionalProperties": false
/// }
/// ```
pub fn dsl_schema_to_json_schema(schema: &HashMap<String, SchemaField>) -> Value {
    let mut properties = serde_json::Map::new();
    let mut required = Vec::new();

    for (field_name, field) in schema {
        properties.insert(field_name.clone(), schema_field_to_json_schema(field));

        if field.required {
            required.push(Value::String(field_name.clone()));
        }
    }

    let mut json_schema = serde_json::Map::new();
    json_schema.insert("type".to_string(), Value::String("object".to_string()));
    json_schema.insert("properties".to_string(), Value::Object(properties));
    if !required.is_empty() {
        required.sort_by(|a, b| a.as_str().unwrap_or("").cmp(b.as_str().unwrap_or("")));
        json_schema.insert("required".to_string(), Value::Array(required));
    }
    json_schema.insert("additionalProperties".to_string(), Value::Bool(false));

    Value::Object(json_schema)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SchemaFieldType;

    fn field(ft: SchemaFieldType, required: bool) -> SchemaField {
        SchemaField {
            field_type: ft,
            description: None,
            required,
            default: None,
            example: None,
            items: None,
            enum_values: None,
            label: None,
            placeholder: None,
            order: None,
            format: None,
            min: None,
            max: None,
            pattern: None,
            properties: None,
            visible_when: None,
        }
    }

    #[test]
    fn test_basic_conversion() {
        let mut schema = HashMap::new();
        schema.insert("name".to_string(), field(SchemaFieldType::String, true));
        schema.insert("count".to_string(), field(SchemaFieldType::Integer, false));

        let result = dsl_schema_to_json_schema(&schema);
        assert_eq!(result["type"], "object");
        assert_eq!(result["properties"]["name"]["type"], "string");
        assert_eq!(result["properties"]["count"]["type"], "integer");
        assert_eq!(result["additionalProperties"], false);

        let req = result["required"].as_array().unwrap();
        assert_eq!(req.len(), 1);
        assert_eq!(req[0], "name");
    }

    #[test]
    fn test_array_with_items() {
        let mut schema = HashMap::new();
        schema.insert(
            "tags".to_string(),
            SchemaField {
                field_type: SchemaFieldType::Array,
                required: true,
                items: Some(Box::new(field(SchemaFieldType::String, false))),
                description: None,
                default: None,
                example: None,
                enum_values: None,
                label: None,
                placeholder: None,
                order: None,
                format: None,
                min: None,
                max: None,
                pattern: None,
                properties: None,
                visible_when: None,
            },
        );

        let result = dsl_schema_to_json_schema(&schema);
        assert_eq!(result["properties"]["tags"]["type"], "array");
        assert_eq!(result["properties"]["tags"]["items"]["type"], "string");
    }

    #[test]
    fn test_enum_values() {
        let mut schema = HashMap::new();
        schema.insert(
            "sentiment".to_string(),
            SchemaField {
                field_type: SchemaFieldType::String,
                required: true,
                enum_values: Some(vec![
                    Value::String("positive".to_string()),
                    Value::String("negative".to_string()),
                    Value::String("neutral".to_string()),
                ]),
                description: Some("Detected sentiment".to_string()),
                default: None,
                example: None,
                items: None,
                label: None,
                placeholder: None,
                order: None,
                format: None,
                min: None,
                max: None,
                pattern: None,
                properties: None,
                visible_when: None,
            },
        );

        let result = dsl_schema_to_json_schema(&schema);
        assert_eq!(result["properties"]["sentiment"]["type"], "string");
        assert_eq!(
            result["properties"]["sentiment"]["description"],
            "Detected sentiment"
        );
        let enums = result["properties"]["sentiment"]["enum"]
            .as_array()
            .unwrap();
        assert_eq!(enums.len(), 3);
    }

    #[test]
    fn test_no_required_fields() {
        let mut schema = HashMap::new();
        schema.insert("notes".to_string(), field(SchemaFieldType::String, false));

        let result = dsl_schema_to_json_schema(&schema);
        assert!(result.get("required").is_none());
    }

    #[test]
    fn test_empty_schema() {
        let schema = HashMap::new();
        let result = dsl_schema_to_json_schema(&schema);
        assert_eq!(result["type"], "object");
        assert_eq!(result["properties"].as_object().unwrap().len(), 0);
    }
}
