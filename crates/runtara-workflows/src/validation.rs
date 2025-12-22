// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Workflow validation for security and correctness.
//!
//! This module validates workflows before compilation to ensure:
//! - Connection data doesn't leak to non-secure agents
//! - Other security and correctness checks

use runtara_dsl::{ExecutionGraph, InputMapping, MappingValue, Step};

#[cfg(test)]
use runtara_dsl::ReferenceValue;
use std::collections::HashSet;

/// Errors that can occur during validation.
#[derive(Debug, Clone)]
pub enum ValidationError {
    /// Connection data is referenced by a non-secure agent.
    ConnectionLeakToNonSecureAgent {
        /// The connection step ID whose data is leaking.
        connection_step_id: String,
        /// The agent step ID that references the connection data.
        agent_step_id: String,
        /// The agent ID (e.g., "transform", "http").
        agent_id: String,
    },
    /// Connection data is referenced by a Finish step.
    ConnectionLeakToFinish {
        /// The connection step ID whose data is leaking.
        connection_step_id: String,
        /// The finish step ID.
        finish_step_id: String,
    },
    /// Connection data is referenced by a Log step.
    ConnectionLeakToLog {
        /// The connection step ID whose data is leaking.
        connection_step_id: String,
        /// The log step ID.
        log_step_id: String,
    },
}

impl std::fmt::Display for ValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ValidationError::ConnectionLeakToNonSecureAgent {
                connection_step_id,
                agent_step_id,
                agent_id,
            } => {
                write!(
                    f,
                    "Security violation: Connection step '{}' outputs are referenced by non-secure agent '{}' (step '{}'). \
                     Connection data can only be passed to secure agents (http, sftp).",
                    connection_step_id, agent_id, agent_step_id
                )
            }
            ValidationError::ConnectionLeakToFinish {
                connection_step_id,
                finish_step_id,
            } => {
                write!(
                    f,
                    "Security violation: Connection step '{}' outputs are referenced by Finish step '{}'. \
                     Connection data cannot be included in workflow outputs.",
                    connection_step_id, finish_step_id
                )
            }
            ValidationError::ConnectionLeakToLog {
                connection_step_id,
                log_step_id,
            } => {
                write!(
                    f,
                    "Security violation: Connection step '{}' outputs are referenced by Log step '{}'. \
                     Connection data cannot be logged.",
                    connection_step_id, log_step_id
                )
            }
        }
    }
}

impl std::error::Error for ValidationError {}

/// Validate a workflow for security and correctness.
///
/// Returns a list of validation errors, or an empty vector if the workflow is valid.
pub fn validate_workflow(graph: &ExecutionGraph) -> Vec<ValidationError> {
    let mut errors = Vec::new();

    // Collect all connection step IDs
    let connection_step_ids: HashSet<String> = graph
        .steps
        .iter()
        .filter_map(|(id, step)| {
            if matches!(step, Step::Connection(_)) {
                Some(id.clone())
            } else {
                None
            }
        })
        .collect();

    // If no connection steps, nothing to validate
    if connection_step_ids.is_empty() {
        return errors;
    }

    // Check each step for connection data leakage
    for (step_id, step) in &graph.steps {
        match step {
            Step::Agent(agent_step) => {
                // Check if agent is secure
                let is_secure = runtara_dsl::agent_meta::find_agent_module(&agent_step.agent_id)
                    .map(|m| m.secure)
                    .unwrap_or(false);

                if !is_secure {
                    // Check input mapping for connection references
                    if let Some(mapping) = &agent_step.input_mapping {
                        for conn_id in find_connection_references(mapping, &connection_step_ids) {
                            errors.push(ValidationError::ConnectionLeakToNonSecureAgent {
                                connection_step_id: conn_id,
                                agent_step_id: step_id.clone(),
                                agent_id: agent_step.agent_id.clone(),
                            });
                        }
                    }
                }
            }
            Step::Finish(finish_step) => {
                // Connection data cannot be in workflow outputs
                if let Some(mapping) = &finish_step.input_mapping {
                    for conn_id in find_connection_references(mapping, &connection_step_ids) {
                        errors.push(ValidationError::ConnectionLeakToFinish {
                            connection_step_id: conn_id,
                            finish_step_id: step_id.clone(),
                        });
                    }
                }
            }
            Step::Log(log_step) => {
                // Connection data cannot be logged
                if let Some(mapping) = &log_step.context {
                    for conn_id in find_connection_references(mapping, &connection_step_ids) {
                        errors.push(ValidationError::ConnectionLeakToLog {
                            connection_step_id: conn_id,
                            log_step_id: step_id.clone(),
                        });
                    }
                }
            }
            Step::Split(split_step) => {
                // Recursively validate subgraph
                errors.extend(validate_workflow(&split_step.subgraph));
            }
            Step::While(while_step) => {
                // Recursively validate subgraph
                errors.extend(validate_workflow(&while_step.subgraph));
            }
            // Other steps don't have input mappings that could leak connection data
            Step::Conditional(_)
            | Step::Switch(_)
            | Step::StartScenario(_)
            | Step::Connection(_) => {}
        }
    }

    errors
}

/// Find connection step IDs referenced in an input mapping.
fn find_connection_references(
    mapping: &InputMapping,
    connection_step_ids: &HashSet<String>,
) -> Vec<String> {
    let mut found = Vec::new();

    for value in mapping.values() {
        if let MappingValue::Reference(ref_value) = value {
            // Check if the reference path starts with "steps.<connection_step_id>"
            if let Some(step_id) = extract_step_id_from_reference(&ref_value.value) {
                if connection_step_ids.contains(&step_id) {
                    found.push(step_id);
                }
            }
        }
    }

    found
}

/// Extract step ID from a reference path like "steps.my_step.outputs.foo"
fn extract_step_id_from_reference(ref_path: &str) -> Option<String> {
    if ref_path.starts_with("steps.") {
        let rest = &ref_path[6..]; // Skip "steps."
        if let Some(dot_pos) = rest.find('.') {
            return Some(rest[..dot_pos].to_string());
        } else {
            // Reference is just "steps.step_id" (unlikely but possible)
            return Some(rest.to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtara_dsl::{AgentStep, ConnectionStep, FinishStep, LogLevel, LogStep};
    use std::collections::HashMap;

    fn create_connection_step(id: &str) -> Step {
        Step::Connection(ConnectionStep {
            id: id.to_string(),
            name: None,
            connection_id: "test-conn".to_string(),
            integration_id: "bearer".to_string(),
        })
    }

    fn create_agent_step(id: &str, agent_id: &str, mapping: Option<InputMapping>) -> Step {
        Step::Agent(AgentStep {
            id: id.to_string(),
            name: None,
            agent_id: agent_id.to_string(),
            capability_id: "test".to_string(),
            connection_id: None,
            input_mapping: mapping,
            max_retries: None,
            retry_delay: None,
            timeout: None,
        })
    }

    fn create_finish_step(id: &str, mapping: Option<InputMapping>) -> Step {
        Step::Finish(FinishStep {
            id: id.to_string(),
            name: None,
            input_mapping: mapping,
        })
    }

    fn create_log_step(id: &str, context: Option<InputMapping>) -> Step {
        Step::Log(LogStep {
            id: id.to_string(),
            name: None,
            level: LogLevel::Info,
            message: "test".to_string(),
            context,
        })
    }

    fn ref_value(path: &str) -> MappingValue {
        MappingValue::Reference(ReferenceValue {
            value: path.to_string(),
            type_hint: None,
            default: None,
        })
    }

    #[test]
    fn test_no_connection_steps_passes() {
        let mut steps = HashMap::new();
        steps.insert(
            "agent".to_string(),
            create_agent_step("agent", "transform", None),
        );
        steps.insert("finish".to_string(), create_finish_step("finish", None));

        let graph = ExecutionGraph {
            name: None,
            description: None,
            steps,
            entry_point: "agent".to_string(),
            execution_plan: vec![],
            variables: HashMap::new(),
            input_schema: HashMap::new(),
            output_schema: HashMap::new(),
            notes: None,
            nodes: None,
            edges: None,
        };

        let errors = validate_workflow(&graph);
        assert!(errors.is_empty());
    }

    #[test]
    fn test_connection_to_secure_agent_passes() {
        let mut steps = HashMap::new();
        steps.insert("conn".to_string(), create_connection_step("conn"));

        let mut mapping = HashMap::new();
        mapping.insert("_connection".to_string(), ref_value("steps.conn.outputs"));
        steps.insert(
            "http_call".to_string(),
            create_agent_step("http_call", "http", Some(mapping)),
        );
        steps.insert("finish".to_string(), create_finish_step("finish", None));

        let graph = ExecutionGraph {
            name: None,
            description: None,
            steps,
            entry_point: "conn".to_string(),
            execution_plan: vec![],
            variables: HashMap::new(),
            input_schema: HashMap::new(),
            output_schema: HashMap::new(),
            notes: None,
            nodes: None,
            edges: None,
        };

        let errors = validate_workflow(&graph);
        assert!(errors.is_empty());
    }

    #[test]
    fn test_connection_to_non_secure_agent_fails() {
        let mut steps = HashMap::new();
        steps.insert("conn".to_string(), create_connection_step("conn"));

        let mut mapping = HashMap::new();
        mapping.insert("data".to_string(), ref_value("steps.conn.outputs"));
        steps.insert(
            "transform".to_string(),
            create_agent_step("transform", "transform", Some(mapping)),
        );
        steps.insert("finish".to_string(), create_finish_step("finish", None));

        let graph = ExecutionGraph {
            name: None,
            description: None,
            steps,
            entry_point: "conn".to_string(),
            execution_plan: vec![],
            variables: HashMap::new(),
            input_schema: HashMap::new(),
            output_schema: HashMap::new(),
            notes: None,
            nodes: None,
            edges: None,
        };

        let errors = validate_workflow(&graph);
        assert_eq!(errors.len(), 1);
        assert!(matches!(
            &errors[0],
            ValidationError::ConnectionLeakToNonSecureAgent { agent_id, .. } if agent_id == "transform"
        ));
    }

    #[test]
    fn test_connection_to_finish_fails() {
        let mut steps = HashMap::new();
        steps.insert("conn".to_string(), create_connection_step("conn"));

        let mut mapping = HashMap::new();
        mapping.insert("credentials".to_string(), ref_value("steps.conn.outputs"));
        steps.insert(
            "finish".to_string(),
            create_finish_step("finish", Some(mapping)),
        );

        let graph = ExecutionGraph {
            name: None,
            description: None,
            steps,
            entry_point: "conn".to_string(),
            execution_plan: vec![],
            variables: HashMap::new(),
            input_schema: HashMap::new(),
            output_schema: HashMap::new(),
            notes: None,
            nodes: None,
            edges: None,
        };

        let errors = validate_workflow(&graph);
        assert_eq!(errors.len(), 1);
        assert!(matches!(
            &errors[0],
            ValidationError::ConnectionLeakToFinish { .. }
        ));
    }

    #[test]
    fn test_connection_to_log_fails() {
        let mut steps = HashMap::new();
        steps.insert("conn".to_string(), create_connection_step("conn"));

        let mut mapping = HashMap::new();
        mapping.insert(
            "secret".to_string(),
            ref_value("steps.conn.outputs.parameters"),
        );
        steps.insert("log".to_string(), create_log_step("log", Some(mapping)));
        steps.insert("finish".to_string(), create_finish_step("finish", None));

        let graph = ExecutionGraph {
            name: None,
            description: None,
            steps,
            entry_point: "conn".to_string(),
            execution_plan: vec![],
            variables: HashMap::new(),
            input_schema: HashMap::new(),
            output_schema: HashMap::new(),
            notes: None,
            nodes: None,
            edges: None,
        };

        let errors = validate_workflow(&graph);
        assert_eq!(errors.len(), 1);
        assert!(matches!(
            &errors[0],
            ValidationError::ConnectionLeakToLog { .. }
        ));
    }
}
