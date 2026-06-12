//! Connection validation for workflow steps.
//!
//! Validates that connection_id references in Agent steps point to
//! existing connections in the database.

use std::collections::HashSet;

use runtara_agents::registry::get_agents;
use runtara_dsl::{AiAgentStep, Step, Workflow};

use super::reference_validation::{IssueCategory, ValidationIssue};

/// Lightweight view of a tenant connection used by the validator to suggest
/// candidates when a referenced connection is missing. Decoupled from
/// `ConnectionDto` so this module doesn't pull in runtara-connections.
#[derive(Debug, Clone)]
pub struct ConnectionRef {
    pub id: String,
    pub integration_id: Option<String>,
    pub title: String,
}

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
            Step::AiAgent(ai_step) => {
                if let Some(ref conn_id) = ai_step.connection_id
                    && !conn_id.is_empty()
                {
                    connection_ids.insert(conn_id.clone());
                }
            }
            Step::Split(split_step) => {
                // Recursively extract from subgraph
                extract_from_graph(&split_step.subgraph, connection_ids);
            }
            Step::While(while_step) => {
                extract_from_graph(&while_step.subgraph, connection_ids);
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
///
/// This is the legacy entry point — it can't suggest candidates because it
/// doesn't know the tenant's connection metadata. New code should prefer
/// `validate_connections_with_candidates`.
pub fn validate_connections(
    workflow: &Workflow,
    existing_connections: &HashSet<String>,
) -> Vec<ValidationIssue> {
    let mut issues = Vec::new();
    validate_graph_connections(
        &workflow.execution_graph,
        existing_connections,
        &[],
        &mut issues,
        None,
    );
    issues
}

/// Validate connection references and append suggested candidates to the
/// "not found" message. Candidates are tenant connections whose
/// `integration_id` is one of the failing step's agent's `integration_ids`.
pub fn validate_connections_with_candidates(
    workflow: &Workflow,
    tenant_connections: &[ConnectionRef],
) -> Vec<ValidationIssue> {
    let existing: HashSet<String> = tenant_connections.iter().map(|c| c.id.clone()).collect();
    let mut issues = Vec::new();
    validate_graph_connections(
        &workflow.execution_graph,
        &existing,
        tenant_connections,
        &mut issues,
        None,
    );
    issues
}

/// Look up an agent's accepted integration ids from static metadata.
fn integration_ids_for_agent(agent_id: &str) -> Vec<String> {
    get_agents()
        .into_iter()
        .find(|a| a.id == agent_id)
        .map(|a| a.integration_ids)
        .unwrap_or_default()
}

fn agent_requires_connection(agent_id: &str) -> bool {
    if agent_id.eq_ignore_ascii_case("http") {
        return false;
    }

    get_agents()
        .into_iter()
        .find(|agent| agent.id.eq_ignore_ascii_case(agent_id))
        .map(|agent| agent.supports_connections)
        .unwrap_or(false)
}

/// Render up to 5 candidate connections as a human-readable list.
fn format_candidates(candidates: &[&ConnectionRef]) -> String {
    const MAX: usize = 5;
    let shown: Vec<String> = candidates
        .iter()
        .take(MAX)
        .map(|c| {
            let int_id = c.integration_id.as_deref().unwrap_or("?");
            format!("'{}' (id={}, integrationId={})", c.title, c.id, int_id)
        })
        .collect();
    let extra = candidates.len().saturating_sub(MAX);
    if extra > 0 {
        format!("{}, … (+{} more)", shown.join(", "), extra)
    } else {
        shown.join(", ")
    }
}

/// Validate connections in an execution graph
fn validate_graph_connections(
    graph: &runtara_dsl::ExecutionGraph,
    existing_connections: &HashSet<String>,
    tenant_connections: &[ConnectionRef],
    issues: &mut Vec<ValidationIssue>,
    parent_context: Option<&str>,
) {
    for step in graph.steps.values() {
        match step {
            Step::Agent(agent_step) => {
                if agent_requires_connection(&agent_step.agent_id)
                    && agent_step
                        .connection_id
                        .as_ref()
                        .map(|conn_id| conn_id.trim().is_empty())
                        .unwrap_or(true)
                {
                    issues.push(
                        ValidationIssue::error(
                            IssueCategory::MissingConnection,
                            &agent_step.id,
                            format!(
                                "Agent '{}' requires a connection for step '{}'",
                                agent_step.agent_id, agent_step.id
                            ),
                        )
                        .with_field("connection_id"),
                    );
                    continue;
                }

                if let Some(ref conn_id) = agent_step.connection_id
                    && !conn_id.is_empty()
                    && !existing_connections.contains(conn_id)
                {
                    let agent_int_ids = integration_ids_for_agent(&agent_step.agent_id);
                    let candidates: Vec<&ConnectionRef> = tenant_connections
                        .iter()
                        .filter(|c| match &c.integration_id {
                            Some(int_id) => agent_int_ids.iter().any(|aid| aid == int_id),
                            None => false,
                        })
                        .collect();

                    let suggestion = if !candidates.is_empty() {
                        format!(
                            ". Available connections for agent '{}': {}",
                            agent_step.agent_id,
                            format_candidates(&candidates)
                        )
                    } else if !agent_int_ids.is_empty() {
                        format!(
                            ". Agent '{}' accepts integrationIds [{}] — none configured for this tenant; \
                             create one via POST /api/runtime/connections",
                            agent_step.agent_id,
                            agent_int_ids.join(", ")
                        )
                    } else {
                        String::new()
                    };

                    let message = if let Some(parent) = parent_context {
                        format!(
                            "[{}] Connection '{}' not found for step '{}'{}",
                            parent, conn_id, agent_step.id, suggestion
                        )
                    } else {
                        format!(
                            "Connection '{}' not found for step '{}'{}",
                            conn_id, agent_step.id, suggestion
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
            Step::AiAgent(ai_step) => {
                validate_ai_agent_connection(
                    ai_step,
                    existing_connections,
                    tenant_connections,
                    issues,
                    parent_context,
                );
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
                    tenant_connections,
                    issues,
                    Some(&context),
                );
            }
            Step::While(while_step) => {
                let context = if let Some(parent) = parent_context {
                    format!("{}/While '{}'", parent, while_step.id)
                } else {
                    format!("While '{}'", while_step.id)
                };
                validate_graph_connections(
                    &while_step.subgraph,
                    existing_connections,
                    tenant_connections,
                    issues,
                    Some(&context),
                );
            }
            _ => {}
        }
    }
}

fn validate_ai_agent_connection(
    ai_step: &AiAgentStep,
    existing_connections: &HashSet<String>,
    tenant_connections: &[ConnectionRef],
    issues: &mut Vec<ValidationIssue>,
    parent_context: Option<&str>,
) {
    let Some(conn_id) = ai_step.connection_id.as_ref().filter(|id| !id.is_empty()) else {
        return;
    };

    let provider = ai_step
        .config
        .as_ref()
        .map(|config| config.provider.as_str());

    if !existing_connections.contains(conn_id) {
        let candidates: Vec<&ConnectionRef> = provider
            .and_then(runtara_ai::provider::compatible_integration_ids_for_provider)
            .map(|accepted| {
                tenant_connections
                    .iter()
                    .filter(|c| match &c.integration_id {
                        Some(int_id) => accepted.contains(&int_id.as_str()),
                        None => false,
                    })
                    .collect()
            })
            .unwrap_or_default();

        let suggestion = if !candidates.is_empty() {
            format!(
                ". Available connections for provider '{}': {}",
                provider.unwrap_or("unknown"),
                format_candidates(&candidates)
            )
        } else {
            provider
                .and_then(runtara_ai::provider::compatible_integration_ids_for_provider)
                .map(|accepted| {
                    format!(
                        ". AI provider '{}' accepts integrationIds [{}] — none configured for this tenant",
                        provider.unwrap_or("unknown"),
                        accepted.join(", ")
                    )
                })
                .unwrap_or_default()
        };

        let message = if let Some(parent) = parent_context {
            format!(
                "[{}] Connection '{}' not found for AI Agent step '{}'{}",
                parent, conn_id, ai_step.id, suggestion
            )
        } else {
            format!(
                "Connection '{}' not found for AI Agent step '{}'{}",
                conn_id, ai_step.id, suggestion
            )
        };

        issues.push(
            ValidationIssue::error(IssueCategory::MissingConnection, &ai_step.id, message)
                .with_field("connection_id"),
        );
        return;
    }

    let Some(provider) = provider else {
        return;
    };

    // The legacy entry point only provides an existence set. Without tenant
    // metadata there is no DB-backed context to validate compatibility against.
    let Some(connection) = tenant_connections.iter().find(|c| c.id == *conn_id) else {
        return;
    };
    let Some(integration_id) = connection.integration_id.as_deref() else {
        issues.push(
            ValidationIssue::error(
                IssueCategory::MissingConnection,
                &ai_step.id,
                format!(
                    "Connection '{}' has no integrationId; cannot validate compatibility with AI provider '{}'",
                    conn_id, provider
                ),
            )
            .with_field("connection_id"),
        );
        return;
    };

    if !runtara_ai::provider::provider_supports_integration(provider, integration_id) {
        let accepted = runtara_ai::provider::compatible_integration_ids_for_provider(provider)
            .map(|ids| ids.join(", "))
            .unwrap_or_else(|| "none".to_string());
        issues.push(
            ValidationIssue::error(
                IssueCategory::MissingConnection,
                &ai_step.id,
                format!(
                    "Connection '{}' integrationId '{}' is not compatible with AI provider '{}'. Compatible integrationIds: [{}]",
                    conn_id, integration_id, provider, accepted
                ),
            )
            .with_field("connection_id"),
        );
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

    // NOTE: these exercise the candidate-suggestion mechanism with `sftp`, a
    // surviving native agent that still carries static `integration_ids`
    // (["sftp"], supports_connections=true). They previously used `shopify`,
    // which was deleted in "agents: delete legacy native integration agents" —
    // shopify now runs as a WASM component and is not in the static agent
    // registry the validator reads. Suggestions for component agents are a
    // known gap tracked separately; the mechanism itself is unchanged.
    #[test]
    fn test_candidate_suggestions_on_missing_connection() {
        let workflow: Workflow = serde_json::from_value(json!({
            "executionGraph": {
                "steps": {
                    "step1": {
                        "stepType": "Agent",
                        "id": "step1",
                        "agentId": "sftp",
                        "capabilityId": "sftp_list_files",
                        "connectionId": "wrong-id"
                    }
                },
                "entryPoint": "step1",
                "executionPlan": []
            },
            "variables": []
        }))
        .unwrap();

        // Tenant has an sftp connection (matching integration_id) and a
        // postgres one (which should NOT be suggested for an sftp step).
        let tenant = vec![
            ConnectionRef {
                id: "conn-sftp".to_string(),
                integration_id: Some("sftp".to_string()),
                title: "My SFTP Server".to_string(),
            },
            ConnectionRef {
                id: "conn-db".to_string(),
                integration_id: Some("postgres".to_string()),
                title: "Object Model DB".to_string(),
            },
        ];

        let issues = validate_connections_with_candidates(&workflow, &tenant);
        assert_eq!(issues.len(), 1, "expected one missing-connection issue");
        let msg = &issues[0].message;
        assert!(msg.contains("'wrong-id'"), "{msg}");
        assert!(
            msg.contains("My SFTP Server"),
            "should suggest the sftp connection: {msg}"
        );
        assert!(
            msg.contains("conn-sftp"),
            "should include the candidate id: {msg}"
        );
        assert!(
            !msg.contains("Object Model DB"),
            "should not suggest unrelated connections: {msg}"
        );
    }

    #[test]
    fn test_no_candidates_lists_accepted_integration_ids() {
        let workflow: Workflow = serde_json::from_value(json!({
            "executionGraph": {
                "steps": {
                    "step1": {
                        "stepType": "Agent",
                        "id": "step1",
                        "agentId": "sftp",
                        "capabilityId": "sftp_list_files",
                        "connectionId": "wrong-id"
                    }
                },
                "entryPoint": "step1",
                "executionPlan": []
            },
            "variables": []
        }))
        .unwrap();

        // Tenant has only an unrelated connection — the validator should fall
        // back to listing the agent's accepted integrationIds.
        let tenant = vec![ConnectionRef {
            id: "conn-db".to_string(),
            integration_id: Some("postgres".to_string()),
            title: "Object Model DB".to_string(),
        }];

        let issues = validate_connections_with_candidates(&workflow, &tenant);
        assert_eq!(issues.len(), 1);
        let msg = &issues[0].message;
        assert!(
            msg.contains("sftp"),
            "should hint at accepted integration ids: {msg}"
        );
        assert!(msg.contains("none configured"), "{msg}");
    }

    fn ai_agent_workflow(provider: &str, connection_id: &str) -> Workflow {
        serde_json::from_value(json!({
            "executionGraph": {
                "steps": {
                    "ai": {
                        "stepType": "AiAgent",
                        "id": "ai",
                        "connectionId": connection_id,
                        "config": {
                            "systemPrompt": {"valueType": "immediate", "value": "You are helpful"},
                            "userPrompt": {"valueType": "immediate", "value": "Do the thing"},
                            "provider": provider
                        }
                    }
                },
                "entryPoint": "ai",
                "executionPlan": []
            },
            "variables": []
        }))
        .unwrap()
    }

    #[test]
    fn ai_agent_openai_connection_is_valid_when_integration_matches_provider() {
        let workflow = ai_agent_workflow("openai", "conn-openai");
        let tenant = vec![ConnectionRef {
            id: "conn-openai".to_string(),
            integration_id: Some("openai_api_key".to_string()),
            title: "OpenAI".to_string(),
        }];

        let issues = validate_connections_with_candidates(&workflow, &tenant);
        assert!(issues.is_empty(), "expected no issues, got {issues:?}");
    }

    #[test]
    fn ai_agent_provider_connection_mismatch_is_rejected() {
        let workflow = ai_agent_workflow("openai", "conn-aws");
        let tenant = vec![ConnectionRef {
            id: "conn-aws".to_string(),
            integration_id: Some("aws_credentials".to_string()),
            title: "AWS".to_string(),
        }];

        let issues = validate_connections_with_candidates(&workflow, &tenant);
        assert_eq!(issues.len(), 1);
        let issue = &issues[0];
        assert_eq!(issue.step_id, "ai");
        assert_eq!(issue.field_name.as_deref(), Some("connection_id"));
        assert!(
            issue.message.contains("not compatible"),
            "{}",
            issue.message
        );
        assert!(issue.message.contains("openai"), "{}", issue.message);
        assert!(
            issue.message.contains("aws_credentials"),
            "{}",
            issue.message
        );
        assert!(
            issue.message.contains("openai_api_key"),
            "{}",
            issue.message
        );
    }

    #[test]
    fn ai_agent_missing_connection_suggests_provider_compatible_connections() {
        let workflow = ai_agent_workflow("bedrock", "wrong-id");
        let tenant = vec![
            ConnectionRef {
                id: "conn-openai".to_string(),
                integration_id: Some("openai_api_key".to_string()),
                title: "OpenAI".to_string(),
            },
            ConnectionRef {
                id: "conn-bedrock".to_string(),
                integration_id: Some("aws_credentials".to_string()),
                title: "Bedrock".to_string(),
            },
        ];

        let issues = validate_connections_with_candidates(&workflow, &tenant);
        assert_eq!(issues.len(), 1);
        let msg = &issues[0].message;
        assert!(msg.contains("wrong-id"), "{msg}");
        assert!(msg.contains("Bedrock"), "{msg}");
        assert!(msg.contains("conn-bedrock"), "{msg}");
        assert!(!msg.contains("conn-openai"), "{msg}");
    }
}
