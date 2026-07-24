// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Structural validation of a JSON value against the JSON Schema subset that
//! [`crate::schema_convert::dsl_schema_to_json_schema`] emits.
//!
//! This is deliberately **not** a general-purpose JSON Schema validator. It
//! checks the structural contract that downstream field mappings depend on —
//! `type`, `required`, nested `properties`, `enum`, and array `items` — and
//! ignores every other keyword.
//!
//! In particular `additionalProperties`, `minLength`/`maxLength`,
//! `minimum`/`maximum` and `pattern` are **not** enforced. The LLM providers'
//! structured-output modes don't enforce them either (they're advisory hints in
//! the schema we send), so treating them as validation failures would reject
//! responses that are otherwise conformant.

use serde_json::Value;

/// Validate `value` against the structural constraints in `schema`.
///
/// Returns every violation found, each as a human-readable message prefixed
/// with the dotted path of the offending location (`$` for the root).
pub fn validate_against_json_schema(value: &Value, schema: &Value) -> Result<(), Vec<String>> {
    let mut errors = Vec::new();
    validate_value(value, schema, "", &mut errors);
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

fn validate_value(value: &Value, schema: &Value, path: &str, errors: &mut Vec<String>) {
    let Some(schema) = schema.as_object() else {
        return;
    };

    if let Some(expected) = schema.get("type").and_then(Value::as_str)
        && !type_matches(value, expected)
    {
        errors.push(format!(
            "{}: expected {expected}, got {}",
            label(path),
            type_name(value)
        ));
        // A type mismatch makes the nested checks meaningless — one clear error
        // beats a cascade of consequential ones.
        return;
    }

    if let Some(allowed) = schema.get("enum").and_then(Value::as_array)
        && !allowed.iter().any(|candidate| candidate == value)
    {
        errors.push(format!(
            "{}: {} is not one of the allowed values",
            label(path),
            render(value)
        ));
    }

    match value {
        Value::Object(map) => {
            let required: Vec<&str> = schema
                .get("required")
                .and_then(Value::as_array)
                .map(|names| names.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();
            for name in &required {
                match map.get(*name) {
                    None => errors.push(format!(
                        "{}: required property `{name}` is missing",
                        label(path)
                    )),
                    Some(Value::Null) => errors.push(format!(
                        "{}: required property `{name}` is null",
                        label(path)
                    )),
                    Some(_) => {}
                }
            }

            if let Some(properties) = schema.get("properties").and_then(Value::as_object) {
                for (name, property_schema) in properties {
                    let Some(child) = map.get(name) else {
                        continue;
                    };
                    // Optional properties may be omitted or explicitly null; a
                    // null on a *required* property is already reported above.
                    if child.is_null() {
                        continue;
                    }
                    validate_value(child, property_schema, &child_path(path, name), errors);
                }
            }
        }
        Value::Array(items) => {
            if let Some(item_schema) = schema.get("items") {
                for (index, item) in items.iter().enumerate() {
                    validate_value(item, item_schema, &format!("{path}[{index}]"), errors);
                }
            }
        }
        _ => {}
    }
}

fn type_matches(value: &Value, expected: &str) -> bool {
    match expected {
        "object" => value.is_object(),
        "array" => value.is_array(),
        "string" => value.is_string(),
        "boolean" => value.is_boolean(),
        // An integral f64 (`3.0`) counts as an integer — providers routinely
        // serialize whole numbers that way.
        "integer" => {
            value.is_i64() || value.is_u64() || value.as_f64().is_some_and(|n| n.fract() == 0.0)
        }
        "number" => value.is_number(),
        // Unknown type keywords aren't enforced.
        _ => true,
    }
}

fn type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(number) => {
            if number.is_f64() {
                "number"
            } else {
                "integer"
            }
        }
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn label(path: &str) -> &str {
    if path.is_empty() { "$" } else { path }
}

fn child_path(path: &str, name: &str) -> String {
    if path.is_empty() {
        name.to_string()
    } else {
        format!("{path}.{name}")
    }
}

/// Render a value for an error message, bounded so a large blob can't dominate
/// the failure text.
fn render(value: &Value) -> String {
    const MAX: usize = 60;
    let rendered = serde_json::to_string(value).unwrap_or_else(|_| "<unrenderable>".to_string());
    if rendered.chars().count() <= MAX {
        return rendered;
    }
    let mut truncated: String = rendered.chars().take(MAX).collect();
    truncated.push('\u{2026}');
    truncated
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema_convert::dsl_schema_to_json_schema;
    use crate::{SchemaField, SchemaFieldType};
    use serde_json::json;
    use std::collections::HashMap;

    fn field(field_type: SchemaFieldType, required: bool) -> SchemaField {
        SchemaField {
            field_type,
            description: None,
            required,
            default: None,
            example: None,
            items: None,
            enum_values: None,
            integration: None,
            label: None,
            placeholder: None,
            order: None,
            format: None,
            min: None,
            max: None,
            pattern: None,
            properties: None,
            visible_when: None,
            nullable: None,
        }
    }

    /// The schema an AiAgent step with this DSL config actually sends:
    /// `sentiment` (required enum string), `confidence` (required number),
    /// `reasoning` (optional string).
    fn sentiment_schema() -> Value {
        let mut schema = HashMap::new();
        let mut sentiment = field(SchemaFieldType::String, true);
        sentiment.enum_values = Some(vec![json!("positive"), json!("negative")]);
        schema.insert("sentiment".to_string(), sentiment);
        schema.insert(
            "confidence".to_string(),
            field(SchemaFieldType::Number, true),
        );
        schema.insert(
            "reasoning".to_string(),
            field(SchemaFieldType::String, false),
        );
        dsl_schema_to_json_schema(&schema)
    }

    #[test]
    fn conforming_object_passes() {
        let value = json!({"sentiment": "positive", "confidence": 0.9, "reasoning": "clear"});
        assert!(validate_against_json_schema(&value, &sentiment_schema()).is_ok());
    }

    #[test]
    fn missing_required_property_is_reported() {
        let value = json!({"confidence": 0.9});
        let errors = validate_against_json_schema(&value, &sentiment_schema())
            .expect_err("missing required property");
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(errors[0].contains("`sentiment`"), "{errors:?}");
        assert!(errors[0].contains("missing"), "{errors:?}");
    }

    #[test]
    fn required_null_property_is_reported_once() {
        let value = json!({"sentiment": null, "confidence": 0.9});
        let errors = validate_against_json_schema(&value, &sentiment_schema())
            .expect_err("null required property");
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(errors[0].contains("is null"), "{errors:?}");
    }

    #[test]
    fn wrong_scalar_type_is_reported() {
        let value = json!({"sentiment": 3, "confidence": 0.9});
        let errors =
            validate_against_json_schema(&value, &sentiment_schema()).expect_err("wrong type");
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert_eq!(errors[0], "sentiment: expected string, got integer");
    }

    #[test]
    fn enum_violation_is_reported() {
        let value = json!({"sentiment": "ecstatic", "confidence": 0.9});
        let errors =
            validate_against_json_schema(&value, &sentiment_schema()).expect_err("bad enum");
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(errors[0].starts_with("sentiment:"), "{errors:?}");
        assert!(errors[0].contains("allowed values"), "{errors:?}");
    }

    #[test]
    fn optional_property_may_be_absent_or_null() {
        let schema = sentiment_schema();
        let absent = json!({"sentiment": "positive", "confidence": 0.9});
        assert!(validate_against_json_schema(&absent, &schema).is_ok());
        let null = json!({"sentiment": "positive", "confidence": 0.9, "reasoning": null});
        assert!(validate_against_json_schema(&null, &schema).is_ok());
    }

    #[test]
    fn integral_float_satisfies_integer() {
        let mut schema = HashMap::new();
        schema.insert("count".to_string(), field(SchemaFieldType::Integer, true));
        let schema = dsl_schema_to_json_schema(&schema);

        assert!(validate_against_json_schema(&json!({"count": 3.0}), &schema).is_ok());
        let errors = validate_against_json_schema(&json!({"count": 3.5}), &schema)
            .expect_err("fractional number is not an integer");
        assert_eq!(errors[0], "count: expected integer, got number");
    }

    #[test]
    fn nested_object_errors_carry_a_dotted_path() {
        let mut inner = HashMap::new();
        inner.insert("city".to_string(), field(SchemaFieldType::String, true));
        let mut address = field(SchemaFieldType::Object, true);
        address.properties = Some(inner);
        let mut schema = HashMap::new();
        schema.insert("address".to_string(), address);
        let schema = dsl_schema_to_json_schema(&schema);

        let errors = validate_against_json_schema(&json!({"address": {"city": 12}}), &schema)
            .expect_err("nested type mismatch");
        assert_eq!(errors[0], "address.city: expected string, got integer");
    }

    #[test]
    fn array_item_errors_carry_an_index() {
        let mut schema = HashMap::new();
        let mut tags = field(SchemaFieldType::Array, true);
        tags.items = Some(Box::new(field(SchemaFieldType::String, false)));
        schema.insert("tags".to_string(), tags);
        let schema = dsl_schema_to_json_schema(&schema);

        let errors = validate_against_json_schema(&json!({"tags": ["a", 2]}), &schema)
            .expect_err("bad array item");
        assert_eq!(errors[0], "tags[1]: expected string, got integer");
    }

    #[test]
    fn non_object_root_is_reported_against_the_root_path() {
        let errors = validate_against_json_schema(&json!("just a string"), &sentiment_schema())
            .expect_err("root type mismatch");
        assert_eq!(errors, vec!["$: expected object, got string"]);
    }

    #[test]
    fn undeclared_properties_are_accepted() {
        // `dsl_schema_to_json_schema` emits `additionalProperties: false`, which
        // this validator deliberately does not enforce — extra keys don't break
        // the downstream mappings this guards.
        let value = json!({"sentiment": "positive", "confidence": 0.9, "extra": true});
        assert!(validate_against_json_schema(&value, &sentiment_schema()).is_ok());
    }

    #[test]
    fn bounds_and_pattern_are_not_enforced() {
        // Same rationale: the providers treat these as advisory, so enforcing
        // them here would fail otherwise-conformant responses.
        let mut name = field(SchemaFieldType::String, true);
        name.min = Some(5.0);
        name.pattern = Some("^[A-Z]+$".to_string());
        let mut schema = HashMap::new();
        schema.insert("name".to_string(), name);
        let schema = dsl_schema_to_json_schema(&schema);

        assert!(validate_against_json_schema(&json!({"name": "ab"}), &schema).is_ok());
    }

    #[test]
    fn every_violation_is_collected() {
        let value = json!({"sentiment": "ecstatic", "reasoning": 7});
        let errors =
            validate_against_json_schema(&value, &sentiment_schema()).expect_err("three problems");
        assert_eq!(errors.len(), 3, "{errors:?}");
        assert!(
            errors.iter().any(|e| e.contains("`confidence`")),
            "{errors:?}"
        );
        assert!(
            errors
                .iter()
                .any(|e| e.starts_with("sentiment:") && e.contains("allowed values")),
            "{errors:?}"
        );
        assert!(
            errors
                .iter()
                .any(|e| e == "reasoning: expected string, got integer"),
            "{errors:?}"
        );
    }
}
