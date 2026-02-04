// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Runtime validation for child scenario inputs.
//!
//! This module provides types and functions for validating inputs
//! to child scenarios at runtime, catching issues that compile-time
//! validation cannot detect (null values for required fields, dynamic references).

use serde_json::Value;
use std::fmt;

/// Information about a required field in a child scenario's input schema.
#[derive(Debug, Clone)]
pub struct RequiredField {
    /// Field name
    pub name: &'static str,
    /// Field type (for error messages)
    pub field_type: &'static str,
    /// Optional description
    pub description: Option<&'static str>,
}

/// Embedded input schema for a child scenario.
#[derive(Debug, Clone)]
pub struct ChildInputSchema {
    /// List of required fields
    pub required_fields: &'static [RequiredField],
}

/// Reason why a required input is missing.
#[derive(Debug, Clone)]
pub enum MissingReason {
    /// Field was not provided at all
    NotProvided,
    /// Field was provided but value was null
    WasNull,
}

impl fmt::Display for MissingReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MissingReason::NotProvided => write!(f, "not provided"),
            MissingReason::WasNull => write!(f, "was null"),
        }
    }
}

/// A missing required input field.
#[derive(Debug, Clone)]
pub struct MissingInput {
    /// Field name
    pub name: String,
    /// Field type
    pub field_type: String,
    /// Optional description
    pub description: Option<String>,
    /// Why the field is considered missing
    pub reason: MissingReason,
}

/// Error when child scenario inputs are invalid.
#[derive(Debug, Clone)]
pub struct ChildInputValidationError {
    /// Parent step ID
    pub parent_step: String,
    /// Child scenario ID
    pub child_scenario: String,
    /// Missing or invalid fields
    pub missing_inputs: Vec<MissingInput>,
}

impl fmt::Display for ChildInputValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(
            f,
            "StartScenario step '{}' is missing required inputs for child scenario '{}':",
            self.parent_step, self.child_scenario
        )?;
        for input in &self.missing_inputs {
            write!(f, "  - {} ({})", input.name, input.field_type)?;
            if let Some(ref desc) = input.description {
                write!(f, ": {}", desc)?;
            }
            writeln!(f, " [{}]", input.reason)?;
        }
        Ok(())
    }
}

impl std::error::Error for ChildInputValidationError {}

/// Validate inputs against a child scenario's schema.
///
/// Returns Ok(()) if all required fields are present and non-null.
/// Returns Err with details about missing/null fields otherwise.
pub fn validate_child_inputs(
    parent_step: &str,
    child_scenario: &str,
    inputs: &Value,
    schema: &ChildInputSchema,
) -> Result<(), ChildInputValidationError> {
    let mut missing_inputs = Vec::new();

    let input_obj = inputs.as_object();

    for field in schema.required_fields {
        let value = input_obj.and_then(|obj| obj.get(field.name));

        match value {
            None => {
                missing_inputs.push(MissingInput {
                    name: field.name.to_string(),
                    field_type: field.field_type.to_string(),
                    description: field.description.map(String::from),
                    reason: MissingReason::NotProvided,
                });
            }
            Some(Value::Null) => {
                missing_inputs.push(MissingInput {
                    name: field.name.to_string(),
                    field_type: field.field_type.to_string(),
                    description: field.description.map(String::from),
                    reason: MissingReason::WasNull,
                });
            }
            Some(_) => {
                // Field is present and not null
            }
        }
    }

    if missing_inputs.is_empty() {
        Ok(())
    } else {
        Err(ChildInputValidationError {
            parent_step: parent_step.to_string(),
            child_scenario: child_scenario.to_string(),
            missing_inputs,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_validate_all_fields_present() {
        static SCHEMA: ChildInputSchema = ChildInputSchema {
            required_fields: &[
                RequiredField {
                    name: "id",
                    field_type: "String",
                    description: None,
                },
                RequiredField {
                    name: "name",
                    field_type: "String",
                    description: Some("User name"),
                },
            ],
        };

        let inputs = json!({
            "id": "123",
            "name": "Test"
        });

        let result = validate_child_inputs("step-1", "child-1", &inputs, &SCHEMA);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_missing_field() {
        static SCHEMA: ChildInputSchema = ChildInputSchema {
            required_fields: &[RequiredField {
                name: "id",
                field_type: "String",
                description: None,
            }],
        };

        let inputs = json!({});

        let result = validate_child_inputs("step-1", "child-1", &inputs, &SCHEMA);
        assert!(result.is_err());

        let err = result.unwrap_err();
        assert_eq!(err.missing_inputs.len(), 1);
        assert_eq!(err.missing_inputs[0].name, "id");
        assert!(matches!(
            err.missing_inputs[0].reason,
            MissingReason::NotProvided
        ));
    }

    #[test]
    fn test_validate_null_field() {
        static SCHEMA: ChildInputSchema = ChildInputSchema {
            required_fields: &[RequiredField {
                name: "id",
                field_type: "String",
                description: None,
            }],
        };

        let inputs = json!({
            "id": null
        });

        let result = validate_child_inputs("step-1", "child-1", &inputs, &SCHEMA);
        assert!(result.is_err());

        let err = result.unwrap_err();
        assert_eq!(err.missing_inputs.len(), 1);
        assert!(matches!(
            err.missing_inputs[0].reason,
            MissingReason::WasNull
        ));
    }
}
