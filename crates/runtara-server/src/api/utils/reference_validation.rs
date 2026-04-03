//! Validation types for scenario validation.
//!
//! These types are used by connection validation and the validation API responses.

use serde::Serialize;

/// Severity of a validation issue
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, serde::Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum IssueSeverity {
    /// Blocking error - definitely wrong
    Error,
    /// Non-blocking warning - possibly wrong or uncertain
    Warning,
}

/// Category of validation issue
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, serde::Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum IssueCategory {
    /// Reference to a step that doesn't exist in the graph
    MissingStep,
    /// Field path on generic object that cannot be verified
    UnknownFieldPath,
    /// Malformed reference path
    InvalidReferencePath,
    /// Connection ID not found in database
    MissingConnection,
}

/// A validation issue with structured information
#[derive(Debug, Clone, Serialize, serde::Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ValidationIssue {
    /// Severity: error (blocking) or warning (non-blocking)
    pub severity: IssueSeverity,

    /// Category of the issue
    pub category: IssueCategory,

    /// Step ID where the issue was found
    pub step_id: String,

    /// Field name in input_mapping (if applicable)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub field_name: Option<String>,

    /// The problematic reference path (if applicable)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reference_path: Option<String>,

    /// Human-readable message
    pub message: String,
}

impl ValidationIssue {
    pub fn error(
        category: IssueCategory,
        step_id: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            severity: IssueSeverity::Error,
            category,
            step_id: step_id.into(),
            field_name: None,
            reference_path: None,
            message: message.into(),
        }
    }

    pub fn warning(
        category: IssueCategory,
        step_id: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            severity: IssueSeverity::Warning,
            category,
            step_id: step_id.into(),
            field_name: None,
            reference_path: None,
            message: message.into(),
        }
    }

    pub fn with_field(mut self, field_name: impl Into<String>) -> Self {
        self.field_name = Some(field_name.into());
        self
    }
}
