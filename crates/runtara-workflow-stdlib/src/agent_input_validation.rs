// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Runtime validation for resolved agent capability inputs.
//!
//! Static validation can prove that required input mappings exist, but it
//! cannot prove that runtime references resolve to non-null values. This module
//! checks the resolved input object immediately before capability dispatch.

use serde_json::Value;
use std::fmt;

/// Information about an agent input field required by a capability.
#[derive(Debug, Clone)]
pub struct RequiredAgentInput {
    /// Field name.
    pub name: &'static str,
    /// Field type for diagnostics.
    pub field_type: &'static str,
    /// Optional field description.
    pub description: Option<&'static str>,
}

/// Reason why a required agent input is invalid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentInputMissingReason {
    /// The field was not present in the resolved input object.
    NotProvided,
    /// The field was present but resolved to JSON null.
    WasNull,
}

impl AgentInputMissingReason {
    fn code(&self) -> &'static str {
        match self {
            AgentInputMissingReason::NotProvided => "STEP_REQUIRED_INPUT_MISSING",
            AgentInputMissingReason::WasNull => "STEP_REQUIRED_INPUT_NULL",
        }
    }
}

impl fmt::Display for AgentInputMissingReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AgentInputMissingReason::NotProvided => write!(f, "not provided"),
            AgentInputMissingReason::WasNull => write!(f, "was null"),
        }
    }
}

/// Missing or null required input.
#[derive(Debug, Clone)]
pub struct MissingAgentInput {
    /// Field name.
    pub name: String,
    /// Field type.
    pub field_type: String,
    /// Optional field description.
    pub description: Option<String>,
    /// Why the input is missing.
    pub reason: AgentInputMissingReason,
}

/// Error returned when resolved agent inputs fail requiredness checks.
#[derive(Debug, Clone)]
pub struct AgentInputValidationError {
    /// Step ID.
    pub step_id: String,
    /// Step display name, if provided.
    pub step_name: Option<String>,
    /// Agent module ID.
    pub agent_id: String,
    /// Capability ID.
    pub capability_id: String,
    /// Missing/null required fields.
    pub missing_inputs: Vec<MissingAgentInput>,
}

impl AgentInputValidationError {
    /// Serialize this validation error as the structured JSON error envelope
    /// used by capability failures.
    pub fn to_json_string(&self) -> String {
        let primary_reason = self
            .missing_inputs
            .first()
            .map(|input| input.reason.code())
            .unwrap_or("STEP_REQUIRED_INPUT_MISSING");

        serde_json::json!({
            "code": primary_reason,
            "message": self.to_string(),
            "category": "permanent",
            "severity": "error",
            "stepId": self.step_id,
            "stepName": self.step_name,
            "stepType": "Agent",
            "agentId": self.agent_id,
            "capabilityId": self.capability_id,
            "missingInputs": self.missing_inputs.iter().map(|input| {
                serde_json::json!({
                    "field": input.name,
                    "expectedType": input.field_type,
                    "description": input.description,
                    "reason": input.reason.to_string(),
                    "code": input.reason.code(),
                })
            }).collect::<Vec<_>>(),
        })
        .to_string()
    }
}

impl fmt::Display for AgentInputValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(
            f,
            "Agent step '{}' is missing required inputs for '{}:{}':",
            self.step_id, self.agent_id, self.capability_id
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

impl std::error::Error for AgentInputValidationError {}

/// Validate resolved agent inputs against required capability fields.
///
/// Returns `Ok(())` when all required fields are present and non-null.
pub fn validate_agent_inputs(
    step_id: &str,
    step_name: Option<&str>,
    agent_id: &str,
    capability_id: &str,
    inputs: &Value,
    required_fields: &[RequiredAgentInput],
) -> Result<(), AgentInputValidationError> {
    let input_obj = inputs.as_object();
    let mut missing_inputs = Vec::new();

    for field in required_fields {
        let value = input_obj.and_then(|obj| obj.get(field.name));
        let reason = match value {
            None => Some(AgentInputMissingReason::NotProvided),
            Some(Value::Null) => Some(AgentInputMissingReason::WasNull),
            Some(_) => None,
        };

        if let Some(reason) = reason {
            missing_inputs.push(MissingAgentInput {
                name: field.name.to_string(),
                field_type: field.field_type.to_string(),
                description: field.description.map(String::from),
                reason,
            });
        }
    }

    if missing_inputs.is_empty() {
        Ok(())
    } else {
        Err(AgentInputValidationError {
            step_id: step_id.to_string(),
            step_name: step_name.map(String::from),
            agent_id: agent_id.to_string(),
            capability_id: capability_id.to_string(),
            missing_inputs,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    static REQUIRED: &[RequiredAgentInput] = &[RequiredAgentInput {
        name: "url",
        field_type: "string",
        description: Some("URL to request"),
    }];

    #[test]
    fn accepts_present_required_input() {
        let result = validate_agent_inputs(
            "fetch",
            Some("Fetch"),
            "http",
            "http-request",
            &json!({
                "url": "https://example.com"
            }),
            REQUIRED,
        );

        assert!(result.is_ok());
    }

    #[test]
    fn rejects_missing_required_input() {
        let err =
            validate_agent_inputs("fetch", None, "http", "http-request", &json!({}), REQUIRED)
                .unwrap_err();

        assert_eq!(err.missing_inputs[0].name, "url");
        assert_eq!(
            err.missing_inputs[0].reason,
            AgentInputMissingReason::NotProvided
        );
        assert!(err.to_json_string().contains("STEP_REQUIRED_INPUT_MISSING"));
    }

    #[test]
    fn rejects_null_required_input() {
        let err = validate_agent_inputs(
            "fetch",
            None,
            "http",
            "http-request",
            &json!({ "url": null }),
            REQUIRED,
        )
        .unwrap_err();

        assert_eq!(
            err.missing_inputs[0].reason,
            AgentInputMissingReason::WasNull
        );
        assert!(err.to_json_string().contains("STEP_REQUIRED_INPUT_NULL"));
    }
}
