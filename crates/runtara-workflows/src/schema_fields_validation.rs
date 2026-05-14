// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Validation for editable workflow schema field rows.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Editable schema field row accepted by workflow schema editors.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EditableSchemaField {
    /// Field name before conversion to map-based schema JSON.
    #[serde(default)]
    pub name: String,
    /// Field type selected in the editor.
    #[serde(default, rename = "type")]
    pub field_type: Option<String>,
}

/// A validation issue for editable schema field rows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SchemaFieldValidationIssue {
    /// Stable validation code.
    pub code: String,
    /// Human-readable validation message.
    pub message: String,
    /// Normalized field name associated with the issue.
    pub field_name: Option<String>,
    /// Zero-based editor row indices affected by this issue.
    pub row_indices: Vec<usize>,
}

/// Validate editable schema field rows before they are collapsed into a JSON object.
pub fn validate_schema_fields(
    schema_label: &str,
    fields: &[EditableSchemaField],
) -> Vec<SchemaFieldValidationIssue> {
    let mut row_indices_by_name: BTreeMap<String, Vec<usize>> = BTreeMap::new();

    for (index, field) in fields.iter().enumerate() {
        let normalized_name = field.name.trim();
        if normalized_name.is_empty() {
            continue;
        }

        row_indices_by_name
            .entry(normalized_name.to_string())
            .or_default()
            .push(index);
    }

    row_indices_by_name
        .into_iter()
        .filter_map(|(field_name, row_indices)| {
            if row_indices.len() < 2 {
                return None;
            }

            Some(SchemaFieldValidationIssue {
                code: "E008".to_string(),
                message: format!(
                    "[E008] {} field name '{}' is duplicated. Field names must be unique.",
                    schema_label, field_name
                ),
                field_name: Some(field_name),
                row_indices,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_unique_schema_field_names() {
        let issues = validate_schema_fields(
            "Input schema",
            &[
                EditableSchemaField {
                    name: "customer_id".to_string(),
                    field_type: Some("string".to_string()),
                },
                EditableSchemaField {
                    name: "order_id".to_string(),
                    field_type: Some("string".to_string()),
                },
            ],
        );

        assert!(issues.is_empty());
    }

    #[test]
    fn detects_duplicate_schema_field_names_after_trimming() {
        let issues = validate_schema_fields(
            "Input schema",
            &[
                EditableSchemaField {
                    name: "customer_id".to_string(),
                    field_type: Some("string".to_string()),
                },
                EditableSchemaField {
                    name: " order_id ".to_string(),
                    field_type: Some("string".to_string()),
                },
                EditableSchemaField {
                    name: "order_id".to_string(),
                    field_type: Some("number".to_string()),
                },
            ],
        );

        assert_eq!(
            issues,
            vec![SchemaFieldValidationIssue {
                code: "E008".to_string(),
                message: "[E008] Input schema field name 'order_id' is duplicated. Field names must be unique.".to_string(),
                field_name: Some("order_id".to_string()),
                row_indices: vec![1, 2],
            }]
        );
    }

    #[test]
    fn ignores_blank_schema_field_names() {
        let issues = validate_schema_fields(
            "Input schema",
            &[
                EditableSchemaField {
                    name: "".to_string(),
                    field_type: Some("string".to_string()),
                },
                EditableSchemaField {
                    name: "   ".to_string(),
                    field_type: Some("string".to_string()),
                },
            ],
        );

        assert!(issues.is_empty());
    }
}
