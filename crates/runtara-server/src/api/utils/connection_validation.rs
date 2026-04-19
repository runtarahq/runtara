//! Connection validation for workflow steps.
//!
//! Validates that connection_id references in Agent steps point to
//! existing connections in the database.

use std::collections::HashSet;

use runtara_dsl::{Workflow, Step};

use super::reference_validation::{IssueCategory, ValidationIssue};

/// Extract all connection IDs referenced in a workflow
pub fn extract_connection_ids(workflow: &Workflow) -> HashSet<String> {
    let mut connection_ids = HashSet::new();
    extract_from_graph(&workflow.execution_graph, &mut connection_ids);
    connection_ids
}

/// Extract connection IDs from an execution graph (including subgraphs)
fn extract_from_graph(graph: &runtara_dsl::ExecutionGraph, connection_ids: &mut HashSet<String>) {
    for step in graph.steps.values() {
        match step {
            Step::Agent(agent_step) => {
                if let Some(ref conn_id) = agent_step.connection_id
                    && !conn_id.is_empty()
                {
                    connection_ids.insert(conn_id.clone());
                }
            }
            Step::Split(split_step) => {
                // Recursively extract from subgraph
                extract_from_graph(&split_step.subgraph, connection_ids);
            }
            // Other step types don't have connections
            _ => {}
        }
    }
}

/// Validate connection references against a set of existing connection IDs.
///
/// Returns validation issues for any connection_id that is not in the
/// `existing_connections` set.
pub fn validate_connections(
    workflow: &Workflow,
    existing_connections: &HashSet<String>,
) -> Vec<ValidationIssue> {
    let mut issues = Vec::new();
    validate_graph_connections(
        &workflow.execution_graph,
        existing_connections,
        &mut issues,
        None,
    );
    issues
}

/// Validate connections in an execution graph
fn validate_graph_connections(
    graph: &runtara_dsl::ExecutionGraph,
    existing_connections: &HashSet<String>,
    issues: &mut Vec<ValidationIssue>,
    parent_context: Option<&str>,
) {
    for step in graph.steps.values() {
        match step {
            Step::Agent(agent_step) => {
                if let Some(ref conn_id) = agent_step.connection_id
                    && !conn_id.is_empty()
                    && !existing_connections.contains(conn_id)
                {
                    let message = if let Some(parent) = parent_context {
                        format!(
                            "[{}] Connection '{}' not found for step '{}'",
                            parent, conn_id, agent_step.id
                        )
                    } else {
                        format!(
                            "Connection '{}' not found for step '{}'",
                            conn_id, agent_step.id
                        )
                    };

                    issues.push(
                        ValidationIssue::error(
                            IssueCategory::MissingConnection,
                            &agent_step.id,
                            message,
                        )
                        .with_field("connection_id"),
                    );
                }
            }
            Step::Split(split_step) => {
                // Recursively validate subgraph
                let context = if let Some(parent) = parent_context {
                    format!("{}/Split '{}'", parent, split_step.id)
                } else {
                    format!("Split '{}'", split_step.id)
                };
                validate_graph_connections(
                    &split_step.subgraph,
                    existing_connections,
                    issues,
                    Some(&context),
                );
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_extract_connection_ids() {
        let workflow: Workflow = serde_json::from_value(json!({
            "executionGraph": {
                "steps": {
                    "step1": {
                        "stepType": "Agent",
                        "id": "step1",
                        "agentId": "http",
                        "capabilityId": "http-request",
                        "connectionId": "my-connection"
                    },
                    "step2": {
                        "stepType": "Agent",
                        "id": "step2",
                        "agentId": "shopify",
                        "capabilityId": "get-products",
                        "connectionId": "shopify-conn"
                    },
                    "step3": {
                        "stepType": "Agent",
                        "id": "step3",
                        "agentId": "utils",
                        "capabilityId": "random-double"
                    }
                },
                "entryPoint": "step1",
                "executionPlan": []
            },
            "variables": []
        }))
        .unwrap();

        let conn_ids = extract_connection_ids(&workflow);
        assert_eq!(conn_ids.len(), 2);
        assert!(conn_ids.contains("my-connection"));
        assert!(conn_ids.contains("shopify-conn"));
    }

    #[test]
    fn test_validate_missing_connection() {
        let workflow: Workflow = serde_json::from_value(json!({
            "executionGraph": {
                "steps": {
                    "step1": {
                        "stepType": "Agent",
                        "id": "step1",
                        "agentId": "http",
                        "capabilityId": "http-request",
                        "connectionId": "nonexistent-connection"
                    }
                },
                "entryPoint": "step1",
                "executionPlan": []
            },
            "variables": []
        }))
        .unwrap();

        let existing: HashSet<String> = HashSet::new();
        let issues = validate_connections(&workflow, &existing);

        assert_eq!(issues.len(), 1);
        assert!(issues[0].message.contains("nonexistent-connection"));
        assert!(issues[0].message.contains("not found"));
    }

    #[test]
    fn test_validate_existing_connection() {
        let workflow: Workflow = serde_json::from_value(json!({
            "executionGraph": {
                "steps": {
                    "step1": {
                        "stepType": "Agent",
                        "id": "step1",
                        "agentId": "http",
                        "capabilityId": "http-request",
                        "connectionId": "my-connection"
                    }
                },
                "entryPoint": "step1",
                "executionPlan": []
            },
            "variables": []
        }))
        .unwrap();

        let mut existing: HashSet<String> = HashSet::new();
        existing.insert("my-connection".to_string());

        let issues = validate_connections(&workflow, &existing);
        assert!(issues.is_empty());
    }
}
