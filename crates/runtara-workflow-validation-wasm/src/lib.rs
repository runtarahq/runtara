// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Browser WASM bindings for workflow validation.

use serde::Serialize;
use serde_json::{Value, json};
use std::sync::OnceLock;
use wasm_bindgen::prelude::*;

const AGENTS_JSON: &str = include_str!(concat!(env!("OUT_DIR"), "/agents.json"));
static AGENTS: OnceLock<Vec<runtara_dsl::agent_meta::AgentInfo>> = OnceLock::new();

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

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AgentSummary {
    id: String,
    name: String,
    description: String,
    supports_connections: bool,
    integration_ids: Vec<String>,
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

/// Return statically compiled agent summaries without capability schemas.
#[wasm_bindgen(js_name = getAgentsJson)]
pub fn get_agents_json() -> String {
    to_json_string(&json!({
        "agents": agent_summaries(agents())
    }))
}

/// Return statically compiled metadata for one agent.
#[wasm_bindgen(js_name = getAgentJson)]
pub fn get_agent_json(agent_id: &str) -> String {
    let agent = agents()
        .iter()
        .find(|agent| agent.id.eq_ignore_ascii_case(agent_id));
    to_json_string(&agent)
}

/// Return statically compiled capability metadata for one agent capability.
#[wasm_bindgen(js_name = getCapabilitySchemaJson)]
pub fn get_capability_schema_json(agent_id: &str, capability_id: &str) -> String {
    let capability = agents()
        .iter()
        .find(|agent| agent.id.eq_ignore_ascii_case(agent_id))
        .and_then(|agent| {
            agent
                .capabilities
                .iter()
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

    let validation_result = runtara_workflows::validation::validate_workflow_with_agent_metadata(
        &workflow.execution_graph,
        agents(),
    );
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

fn agents() -> &'static [runtara_dsl::agent_meta::AgentInfo] {
    AGENTS
        .get_or_init(|| {
            serde_json::from_str(AGENTS_JSON).expect("generated agent metadata must be valid JSON")
        })
        .as_slice()
}

fn agent_summaries(agents: &[runtara_dsl::agent_meta::AgentInfo]) -> Vec<AgentSummary> {
    agents
        .iter()
        .map(|agent| AgentSummary {
            id: agent.id.clone(),
            name: agent.name.clone(),
            description: agent.description.clone(),
            supports_connections: agent.supports_connections,
            integration_ids: agent.integration_ids.clone(),
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
    fn returns_static_step_type_metadata() {
        let value: Value = serde_json::from_str(&get_step_types_json()).unwrap();
        let step_types = value["step_types"].as_array().unwrap();

        assert!(step_types.iter().any(|step| step["id"] == "Start"));
        assert!(step_types.iter().any(|step| step["id"] == "Agent"));
    }

    #[test]
    fn returns_static_agent_summaries_without_capability_schemas() {
        let value: Value = serde_json::from_str(&get_agents_json()).unwrap();
        let agents = value["agents"].as_array().unwrap();

        let http = agents
            .iter()
            .find(|agent| agent["id"] == "http")
            .expect("http agent should be present");
        assert!(http.get("capabilities").is_none());
        assert!(
            http["integrationIds"]
                .as_array()
                .unwrap()
                .iter()
                .any(|id| id == "http_bearer")
        );
    }

    #[test]
    fn generated_metadata_includes_native_proxied_agents() {
        assert!(
            agents().iter().any(|agent| agent.id == "compression"),
            "native-proxied agent metadata should be generated at build time"
        );
    }

    #[test]
    fn returns_single_static_capability_metadata() {
        let full_agent = agents()
            .iter()
            .find(|agent| !agent.capabilities.is_empty())
            .expect("an agent with capabilities should be present");
        let agent_id = full_agent.id.clone();
        let capability_id = full_agent.capabilities[0].id.clone();

        let agent_value: Value = serde_json::from_str(&get_agent_json(&agent_id)).unwrap();
        assert_eq!(agent_value["id"], agent_id);
        assert!(
            agent_value["capabilities"]
                .as_array()
                .is_some_and(|capabilities| !capabilities.is_empty())
        );

        let value: Value =
            serde_json::from_str(&get_capability_schema_json(&agent_id, &capability_id)).unwrap();

        assert_eq!(value["id"], capability_id);
        assert!(value["inputs"].is_array());
    }

    #[test]
    fn returns_full_static_agent_metadata_on_demand() {
        let summaries_value: Value = serde_json::from_str(&get_agents_json()).unwrap();
        let first_agent_id = summaries_value["agents"]
            .as_array()
            .unwrap()
            .iter()
            .find_map(|agent| agent["id"].as_str())
            .expect("an agent summary should be present");

        let agent: Value = serde_json::from_str(&get_agent_json(first_agent_id)).unwrap();

        assert_eq!(agent["id"], first_agent_id);
        assert!(!agent["capabilities"].as_array().unwrap().is_empty());
    }
}
