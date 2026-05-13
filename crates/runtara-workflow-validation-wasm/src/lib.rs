// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Browser WASM bindings for workflow validation.

use serde::Serialize;
use serde_json::{Value, json};
use wasm_bindgen::prelude::*;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ValidationResponse {
    success: bool,
    valid: bool,
    errors: Vec<String>,
    warnings: Vec<String>,
    message: String,
}

#[derive(Serialize)]
struct StepTypeInfo {
    id: String,
    name: String,
    description: String,
    category: String,
}

impl ValidationResponse {
    fn ok(errors: Vec<String>, warnings: Vec<String>) -> Self {
        let valid = errors.is_empty();
        let message = if valid {
            "Graph validation passed".to_string()
        } else {
            format!("Graph validation failed with {} error(s)", errors.len())
        };

        Self {
            success: true,
            valid,
            errors,
            warnings,
            message,
        }
    }

    fn parse_error(message: String) -> Self {
        Self {
            success: true,
            valid: false,
            errors: vec![message],
            warnings: Vec::new(),
            message: "Graph validation failed: invalid workflow format".to_string(),
        }
    }

    fn valid(message: impl Into<String>) -> Self {
        Self {
            success: true,
            valid: true,
            errors: Vec::new(),
            warnings: Vec::new(),
            message: message.into(),
        }
    }

    fn invalid(message: impl Into<String>, errors: Vec<String>) -> Self {
        Self {
            success: true,
            valid: false,
            errors,
            warnings: Vec::new(),
            message: message.into(),
        }
    }
}

/// Validate an execution graph JSON string with the same Rust validation path
/// used by the backend.
#[wasm_bindgen(js_name = validateExecutionGraphJson)]
pub fn validate_execution_graph_json(execution_graph_json: &str) -> String {
    let response = validate_execution_graph_json_impl(execution_graph_json);
    serde_json::to_string(&response).unwrap_or_else(|e| {
        json!({
            "success": false,
            "valid": false,
            "errors": [format!("Failed to serialize validation response: {}", e)],
            "warnings": [],
            "message": "Validation failed"
        })
        .to_string()
    })
}

/// Validate workflow start inputs with the same Rust validation path used by
/// backend execution.
#[wasm_bindgen(js_name = validateWorkflowStartInputsJson)]
pub fn validate_workflow_start_inputs_json(input_schema_json: &str, inputs_json: &str) -> String {
    let response = validate_workflow_start_inputs_json_impl(input_schema_json, inputs_json);
    serde_json::to_string(&response).unwrap_or_else(|e| {
        json!({
            "success": false,
            "valid": false,
            "errors": [format!("Failed to serialize validation response: {}", e)],
            "warnings": [],
            "message": "Workflow start input validation failed"
        })
        .to_string()
    })
}

/// Return statically compiled workflow step type metadata.
#[wasm_bindgen(js_name = getStepTypesJson)]
pub fn get_step_types_json() -> String {
    to_json_string(&json!({
        "step_types": step_types()
    }))
}

/// Return the JSON Schema metadata for a statically compiled workflow step type.
#[wasm_bindgen(js_name = getStepTypeSchemaJson)]
pub fn get_step_type_schema_json(step_type: &str) -> String {
    to_json_string(&runtara_dsl::spec::dsl_schema::get_step_type_schema(
        step_type,
    ))
}

/// Return statically compiled agent metadata, including capability schemas.
#[wasm_bindgen(js_name = getAgentsJson)]
pub fn get_agents_json() -> String {
    to_json_string(&json!({
        "agents": agents()
    }))
}

/// Return statically compiled metadata for one agent.
#[wasm_bindgen(js_name = getAgentJson)]
pub fn get_agent_json(agent_id: &str) -> String {
    let agent = agents()
        .into_iter()
        .find(|agent| agent.id.eq_ignore_ascii_case(agent_id));
    to_json_string(&agent)
}

/// Return statically compiled capability metadata for one agent capability.
#[wasm_bindgen(js_name = getCapabilitySchemaJson)]
pub fn get_capability_schema_json(agent_id: &str, capability_id: &str) -> String {
    let capability = agents()
        .into_iter()
        .find(|agent| agent.id.eq_ignore_ascii_case(agent_id))
        .and_then(|agent| {
            agent
                .capabilities
                .into_iter()
                .find(|capability| capability.id.eq_ignore_ascii_case(capability_id))
        });
    to_json_string(&capability)
}

fn validate_execution_graph_json_impl(execution_graph_json: &str) -> ValidationResponse {
    let graph = match serde_json::from_str::<Value>(execution_graph_json) {
        Ok(value) if value.is_object() => value,
        Ok(_) => {
            return ValidationResponse::parse_error(
                "Invalid graph format: graph must be a JSON object".to_string(),
            );
        }
        Err(e) => {
            return ValidationResponse::parse_error(format!("Failed to parse graph JSON: {}", e));
        }
    };

    let workflow = match serde_json::from_value::<runtara_dsl::Workflow>(json!({
        "executionGraph": graph
    })) {
        Ok(workflow) => workflow,
        Err(e) => {
            return ValidationResponse::parse_error(format!("Failed to parse graph: {}", e));
        }
    };

    let validation_result =
        runtara_workflows::validation::validate_workflow(&workflow.execution_graph);
    let errors = validation_result
        .errors
        .iter()
        .map(ToString::to_string)
        .collect();
    let warnings = validation_result
        .warnings
        .iter()
        .map(ToString::to_string)
        .collect();

    ValidationResponse::ok(errors, warnings)
}

fn validate_workflow_start_inputs_json_impl(
    input_schema_json: &str,
    inputs_json: &str,
) -> ValidationResponse {
    let input_schema = match serde_json::from_str::<Value>(input_schema_json) {
        Ok(value) => value,
        Err(e) => {
            return ValidationResponse::invalid(
                "Workflow start input validation failed: invalid input schema JSON",
                vec![format!("Failed to parse input schema JSON: {}", e)],
            );
        }
    };

    let inputs = match serde_json::from_str::<Value>(inputs_json) {
        Ok(value) => value,
        Err(e) => {
            return ValidationResponse::invalid(
                "Workflow start input validation failed: invalid inputs JSON",
                vec![format!("Failed to parse inputs JSON: {}", e)],
            );
        }
    };

    match runtara_workflows::input_validation::validate_workflow_start_inputs(inputs, &input_schema)
    {
        Ok(_) => ValidationResponse::valid("Workflow start input validation passed"),
        Err(e) => {
            ValidationResponse::invalid("Workflow start input validation failed", vec![e.message])
        }
    }
}

fn to_json_string<T: Serialize>(value: &T) -> String {
    serde_json::to_string(value).unwrap_or_else(|e| {
        json!({
            "success": false,
            "error": format!("Failed to serialize response: {}", e)
        })
        .to_string()
    })
}

fn step_types() -> Vec<StepTypeInfo> {
    let mut step_types = vec![StepTypeInfo {
        id: "Start".to_string(),
        name: "Start".to_string(),
        description: "Entry point - receives workflow inputs".to_string(),
        category: "control".to_string(),
    }];

    for meta in runtara_dsl::agent_meta::get_all_step_types() {
        step_types.push(StepTypeInfo {
            id: meta.id.to_string(),
            name: meta.display_name.to_string(),
            description: meta.description.to_string(),
            category: meta.category.to_string(),
        });
    }

    step_types.sort_by(|a, b| a.id.cmp(&b.id));
    step_types
}

fn agents() -> Vec<runtara_dsl::agent_meta::AgentInfo> {
    let http_ids: Vec<String> = runtara_agents::extractors::get_http_extractor_ids()
        .into_iter()
        .map(String::from)
        .collect();

    runtara_agents::registry::get_agents()
        .into_iter()
        .map(|mut agent| {
            if agent.id == "http" {
                agent.integration_ids = http_ids.clone();
            }
            agent
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_empty_graph_with_backend_validator() {
        let response = validate_execution_graph_json_impl("{}");

        assert!(response.success);
        assert!(!response.valid);
        assert!(!response.errors.is_empty());
    }

    #[test]
    fn validates_workflow_start_inputs_with_backend_validator() {
        let response = validate_workflow_start_inputs_json_impl(
            r#"{"name":{"type":"string","required":true}}"#,
            r#"{"data":{"name":"Runtara"},"variables":{}}"#,
        );

        assert!(response.success);
        assert!(response.valid);
        assert!(response.errors.is_empty());
    }

    #[test]
    fn rejects_invalid_workflow_start_inputs_with_backend_validator() {
        let response = validate_workflow_start_inputs_json_impl(
            r#"{"count":{"type":"integer","required":true}}"#,
            r#"{"data":{"count":"not-a-number"},"variables":{}}"#,
        );

        assert!(response.success);
        assert!(!response.valid);
        assert!(response.message.contains("failed"));
        assert!(response.errors.iter().any(|error| error.contains("count")));
    }

    #[test]
    fn surfaces_template_reference_warnings_from_backend_validator() {
        let response = validate_execution_graph_json_impl(
            r#"{
                "steps": {
                    "finish": {
                        "stepType": "Finish",
                        "id": "finish",
                        "inputMapping": {
                            "summary": {
                                "valueType": "template",
                                "value": "{{ steps.missing_archive.outputs.file }}"
                            }
                        }
                    }
                },
                "entryPoint": "finish"
            }"#,
        );

        assert!(response.success);
        assert!(response.valid);
        assert!(response.errors.is_empty());
        assert!(
            response.warnings.iter().any(|warning| {
                warning.contains("[W052]") && warning.contains("missing_archive")
            }),
            "{:?}",
            response.warnings
        );
    }

    #[test]
    fn rejects_finish_output_without_name_from_backend_validator() {
        let response = validate_execution_graph_json_impl(
            r#"{
                "steps": {
                    "finish": {
                        "stepType": "Finish",
                        "id": "finish",
                        "inputMapping": {
                            "": {
                                "valueType": "reference",
                                "value": "data.orderId"
                            }
                        }
                    }
                },
                "entryPoint": "finish"
            }"#,
        );

        assert!(response.success);
        assert!(!response.valid);
        assert!(
            response.errors.iter().any(|error| {
                error.contains("[E117]") && error.contains("has an output with no name")
            }),
            "{:?}",
            response.errors
        );
    }

    #[test]
    fn rejects_finish_output_without_source_from_backend_validator() {
        let response = validate_execution_graph_json_impl(
            r#"{
                "steps": {
                    "finish": {
                        "stepType": "Finish",
                        "id": "finish",
                        "inputMapping": {
                            "orderId": {
                                "valueType": "reference",
                                "value": " "
                            }
                        }
                    }
                },
                "entryPoint": "finish"
            }"#,
        );

        assert!(response.success);
        assert!(!response.valid);
        assert!(
            response
                .errors
                .iter()
                .any(|error| { error.contains("[E118]") && error.contains("orderId") }),
            "{:?}",
            response.errors
        );
    }

    #[test]
    fn rejects_finish_output_without_immediate_source_from_backend_validator() {
        let response = validate_execution_graph_json_impl(
            r#"{
                "steps": {
                    "finish": {
                        "stepType": "Finish",
                        "id": "finish",
                        "inputMapping": {
                            "status": {
                                "valueType": "immediate",
                                "value": ""
                            }
                        }
                    }
                },
                "entryPoint": "finish"
            }"#,
        );

        assert!(response.success);
        assert!(!response.valid);
        assert!(
            response
                .errors
                .iter()
                .any(|error| { error.contains("[E118]") && error.contains("status") }),
            "{:?}",
            response.errors
        );
    }

    #[test]
    fn returns_static_step_type_metadata() {
        let value: Value = serde_json::from_str(&get_step_types_json()).unwrap();
        let step_types = value["step_types"].as_array().unwrap();

        assert!(step_types.iter().any(|step| step["id"] == "Start"));
        assert!(step_types.iter().any(|step| step["id"] == "Agent"));
    }

    #[test]
    fn returns_static_agent_metadata() {
        let value: Value = serde_json::from_str(&get_agents_json()).unwrap();
        let agents = value["agents"].as_array().unwrap();

        let http = agents
            .iter()
            .find(|agent| agent["id"] == "http")
            .expect("http agent should be present");
        assert!(!http["capabilities"].as_array().unwrap().is_empty());
        assert!(
            http["integrationIds"]
                .as_array()
                .unwrap()
                .iter()
                .any(|id| id == "http_bearer")
        );
    }

    #[test]
    fn returns_single_static_capability_metadata() {
        let agents_value: Value = serde_json::from_str(&get_agents_json()).unwrap();
        let first_agent = agents_value["agents"]
            .as_array()
            .unwrap()
            .iter()
            .find(|agent| {
                agent["capabilities"]
                    .as_array()
                    .is_some_and(|capabilities| !capabilities.is_empty())
            })
            .expect("an agent with capabilities should be present");
        let agent_id = first_agent["id"].as_str().unwrap();
        let capability_id = first_agent["capabilities"][0]["id"].as_str().unwrap();

        let value: Value =
            serde_json::from_str(&get_capability_schema_json(agent_id, capability_id)).unwrap();

        assert_eq!(value["id"], capability_id);
        assert!(value["inputs"].is_array());
    }
}
