//! Connection validation for workflow steps.
//!
//! Validates that connection_id references in Agent steps point to
//! existing connections in the database.

use std::collections::HashSet;

use runtara_dsl::agent_meta::AgentCatalog;
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
    catalog: &AgentCatalog,
) -> Vec<ValidationIssue> {
    let mut issues = Vec::new();
    validate_graph_connections(
        &workflow.execution_graph,
        existing_connections,
        &[],
        catalog,
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
    catalog: &AgentCatalog,
) -> Vec<ValidationIssue> {
    let existing: HashSet<String> = tenant_connections.iter().map(|c| c.id.clone()).collect();
    let mut issues = Vec::new();
    validate_graph_connections(
        &workflow.execution_graph,
        &existing,
        tenant_connections,
        catalog,
        &mut issues,
        None,
    );
    issues
}

/// Whether `agent_id` requires a connection, per the runtime agent catalog
/// (`ComponentDispatcherService::catalog()`) — the same source of truth the
/// dynamic workflow validator and `GET /api/runtime/agents` use.
///
/// This used to consult the statically-compiled
/// `runtara_agents::registry::get_agents()`, which only lists agents with
/// compiled-in capability registrations. Every integration that now runs as
/// a WASM component (shopify, hubspot, stripe, slack, …) was absent from
/// that list, so this check silently passed steps with no connection
/// configured at all instead of flagging them.
fn agent_requires_connection(catalog: &AgentCatalog, agent_id: &str) -> bool {
    if agent_id.eq_ignore_ascii_case("http") {
        return false;
    }

    catalog
        .agent(agent_id)
        .is_some_and(|agent| agent.supports_connections)
}

/// The connection id a step's binding pins at SAVE time, if any: the literal
/// `connection_id`, or a `connection_ref` whose value is an IMMEDIATE string —
/// semantically the same literal, just spelled through the resolvable channel,
/// so it gets the same existence/ownership validation. Reference / template /
/// composite refs resolve at runtime and have nothing checkable here. An
/// empty/whitespace immediate is treated as no binding (not a satisfied one).
fn statically_bound_connection_id<'a>(
    connection_id: Option<&'a String>,
    connection_ref: Option<&'a runtara_dsl::MappingValue>,
) -> Option<&'a str> {
    if let Some(id) = connection_id.map(String::as_str).map(str::trim)
        && !id.is_empty()
    {
        return Some(id);
    }
    if let Some(runtara_dsl::MappingValue::Immediate(imm)) = connection_ref
        && let Some(id) = imm.value.as_str()
        && !id.trim().is_empty()
    {
        return Some(id.trim());
    }
    None
}

/// Whether a `connection_ref` defers to runtime resolution (reference,
/// template, or composite — anything but an immediate literal).
fn is_runtime_resolved_ref(connection_ref: Option<&runtara_dsl::MappingValue>) -> bool {
    matches!(
        connection_ref,
        Some(
            runtara_dsl::MappingValue::Reference(_)
                | runtara_dsl::MappingValue::Template(_)
                | runtara_dsl::MappingValue::Composite(_)
        )
    )
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
    catalog: &AgentCatalog,
    issues: &mut Vec<ValidationIssue>,
    parent_context: Option<&str>,
) {
    for step in graph.steps.values() {
        match step {
            Step::Agent(agent_step) => {
                // A RUNTIME-resolved `connection_ref` (a caller-supplied
                // `connection` input, a rotated value, a dynamic selection)
                // satisfies the connection requirement — its concrete id is
                // bound at runtime, so there is nothing to check for
                // existence/ownership here. An IMMEDIATE-valued ref is a
                // literal in disguise and gets the full literal treatment
                // (and an empty immediate is NOT a binding).
                let bound_literal = statically_bound_connection_id(
                    agent_step.connection_id.as_ref(),
                    agent_step.connection_ref.as_ref(),
                );
                let has_binding = bound_literal.is_some()
                    || is_runtime_resolved_ref(agent_step.connection_ref.as_ref());

                if agent_requires_connection(catalog, &agent_step.agent_id) && !has_binding {
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

                if let Some(conn_id) = bound_literal
                    && !existing_connections.contains(conn_id)
                {
                    let agent_int_ids = catalog.integration_ids_for(&agent_step.agent_id);
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
                    catalog,
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
                    catalog,
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
    // Same immediate-ref folding as the Agent arm: an immediate-valued
    // `connection_ref` is a literal and gets the existence check.
    let Some(conn_id) = statically_bound_connection_id(
        ai_step.connection_id.as_ref(),
        ai_step.connection_ref.as_ref(),
    ) else {
        return;
    };

    if !existing_connections.contains(conn_id) {
        let candidates: Vec<&ConnectionRef> = tenant_connections
            .iter()
            .filter(|connection| {
                connection
                    .integration_id
                    .as_deref()
                    .is_some_and(|integration_id| {
                        runtara_connections::features_for_integration(integration_id)
                            .iter()
                            .any(|feature| feature.key == "ai.chat")
                    })
            })
            .collect();

        let suggestion = if !candidates.is_empty() {
            format!(
                ". Available AI connections: {}",
                format_candidates(&candidates)
            )
        } else {
            ". No connection exposing the 'ai.chat' feature is configured for this tenant"
                .to_string()
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
                    "Connection '{}' has no integrationId and cannot provide AI Agent routing metadata",
                    conn_id
                ),
            )
            .with_field("connection_id"),
        );
        return;
    };

    let supports_ai_chat = runtara_connections::features_for_integration(integration_id)
        .iter()
        .any(|feature| feature.key == "ai.chat" && feature.driver.is_some());
    if !supports_ai_chat {
        issues.push(
            ValidationIssue::error(
                IssueCategory::MissingConnection,
                &ai_step.id,
                format!(
                    "Connection '{}' integrationId '{}' does not expose the 'ai.chat' feature required by an AI Agent",
                    conn_id, integration_id
                ),
            )
            .with_field("connection_id"),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtara_dsl::agent_meta::AgentInfo;
    use serde_json::json;

    /// Build a single-agent catalog for tests that need the connection
    /// guard to recognize an agent the way the WASM component dispatcher
    /// would — this stands in for a `runtara_agent_<id>.meta.json` sidecar.
    fn agent_catalog(id: &str, integration_ids: &[&str]) -> AgentCatalog {
        AgentCatalog::from_agents(vec![AgentInfo {
            id: id.to_string(),
            name: id.to_string(),
            description: String::new(),
            has_side_effects: true,
            supports_connections: true,
            integration_ids: integration_ids.iter().map(|s| s.to_string()).collect(),
            capabilities: Vec::new(),
        }])
    }

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
        let issues = validate_connections(&workflow, &existing, &AgentCatalog::new());

        assert_eq!(issues.len(), 1);
        assert!(issues[0].message.contains("nonexistent-connection"));
        assert!(issues[0].message.contains("not found"));
    }

    /// Regression test for the connection guard being blind to agents that
    /// only exist in the dynamic catalog (i.e. every non-`http`/`sftp`
    /// integration after native-agent deletion). Before threading the
    /// catalog through, `agent_requires_connection` always fell back to
    /// `false` for `shopify` because it wasn't in the compiled-in registry,
    /// so a connection-less Shopify step saved clean and only failed later
    /// at runtime with an opaque credential error.
    #[test]
    fn test_connectionless_shopify_step_is_flagged() {
        let workflow: Workflow = serde_json::from_value(json!({
            "executionGraph": {
                "steps": {
                    "step1": {
                        "stepType": "Agent",
                        "id": "step1",
                        "agentId": "shopify",
                        "capabilityId": "get-products"
                    }
                },
                "entryPoint": "step1",
                "executionPlan": []
            },
            "variables": []
        }))
        .unwrap();

        let existing: HashSet<String> = HashSet::new();
        let catalog = agent_catalog("shopify", &["shopify"]);
        let issues = validate_connections(&workflow, &existing, &catalog);

        assert_eq!(
            issues.len(),
            1,
            "expected a missing-connection issue, got {issues:?}"
        );
        assert!(issues[0].message.contains("requires a connection"));
        assert!(issues[0].message.contains("shopify"));
    }

    /// An IMMEDIATE-valued `connection_ref` is a literal id in disguise: it
    /// must get the same existence check as `connectionId`, not the runtime
    /// free pass a reference/template ref gets.
    #[test]
    fn test_immediate_connection_ref_gets_existence_check() {
        let workflow: Workflow = serde_json::from_value(json!({
            "executionGraph": {
                "steps": {
                    "step1": {
                        "stepType": "Agent",
                        "id": "step1",
                        "agentId": "shopify",
                        "capabilityId": "get-products",
                        "connectionRef": { "valueType": "immediate", "value": "no-such-conn" }
                    }
                },
                "entryPoint": "step1",
                "executionPlan": []
            }
        }))
        .expect("workflow parses");
        let existing: HashSet<String> = ["real-conn".to_string()].into();
        let catalog = agent_catalog("shopify", &["shopify"]);
        let issues = validate_connections(&workflow, &existing, &catalog);

        assert_eq!(
            issues.len(),
            1,
            "an immediate ref naming a non-existent connection must be flagged, got {issues:?}"
        );
        assert!(issues[0].message.contains("no-such-conn"));
        assert!(issues[0].message.contains("not found"));
    }

    #[test]
    fn test_immediate_connection_ref_with_existing_id_passes() {
        let workflow: Workflow = serde_json::from_value(json!({
            "executionGraph": {
                "steps": {
                    "step1": {
                        "stepType": "Agent",
                        "id": "step1",
                        "agentId": "shopify",
                        "capabilityId": "get-products",
                        "connectionRef": { "valueType": "immediate", "value": "real-conn" }
                    }
                },
                "entryPoint": "step1",
                "executionPlan": []
            }
        }))
        .expect("workflow parses");
        let existing: HashSet<String> = ["real-conn".to_string()].into();
        let catalog = agent_catalog("shopify", &["shopify"]);
        let issues = validate_connections(&workflow, &existing, &catalog);
        assert!(
            issues.is_empty(),
            "existing immediate id passes: {issues:?}"
        );
    }

    /// An empty immediate is NOT a satisfied binding — the step is missing
    /// its connection, same as no binding at all.
    #[test]
    fn test_empty_immediate_connection_ref_is_not_a_binding() {
        let workflow: Workflow = serde_json::from_value(json!({
            "executionGraph": {
                "steps": {
                    "step1": {
                        "stepType": "Agent",
                        "id": "step1",
                        "agentId": "shopify",
                        "capabilityId": "get-products",
                        "connectionRef": { "valueType": "immediate", "value": "" }
                    }
                },
                "entryPoint": "step1",
                "executionPlan": []
            }
        }))
        .expect("workflow parses");
        let existing: HashSet<String> = HashSet::new();
        let catalog = agent_catalog("shopify", &["shopify"]);
        let issues = validate_connections(&workflow, &existing, &catalog);
        assert_eq!(issues.len(), 1, "empty immediate is no binding: {issues:?}");
        assert!(issues[0].message.contains("requires a connection"));
    }

    /// A connection-requiring agent bound via `connection_ref` (a caller-supplied
    /// `connection` input) carries no literal `connection_id`, yet must NOT be
    /// flagged as missing a connection — the concrete id is bound at runtime, so
    /// there is nothing to existence/ownership-check at author time.
    #[test]
    fn connection_ref_satisfies_the_requirement_without_a_literal_id() {
        let workflow: Workflow = serde_json::from_value(json!({
            "executionGraph": {
                "steps": {
                    "step1": {
                        "stepType": "Agent",
                        "id": "step1",
                        "agentId": "shopify",
                        "capabilityId": "get-products",
                        "connectionRef": {"valueType": "reference", "value": "data.store"}
                    }
                },
                "entryPoint": "step1",
                "executionPlan": [],
                "inputSchema": {
                    "store": {"type": "connection", "integration": "shopify", "required": true}
                }
            },
            "variables": []
        }))
        .unwrap();

        let existing: HashSet<String> = HashSet::new();
        let catalog = agent_catalog("shopify", &["shopify"]);
        let issues = validate_connections(&workflow, &existing, &catalog);
        assert!(
            issues.is_empty(),
            "connection_ref should satisfy the requirement, got {issues:?}"
        );

        // And a literal connection_id is still ownership-checked as before: an
        // unknown id under the same agent is flagged.
        let no_ref: Workflow = serde_json::from_value(json!({
            "executionGraph": {
                "steps": {
                    "step1": {
                        "stepType": "Agent",
                        "id": "step1",
                        "agentId": "shopify",
                        "capabilityId": "get-products",
                        "connectionId": "unknown-id"
                    }
                },
                "entryPoint": "step1",
                "executionPlan": []
            },
            "variables": []
        }))
        .unwrap();
        let issues = validate_connections(&no_ref, &existing, &catalog);
        assert_eq!(issues.len(), 1, "literal id still checked: {issues:?}");
        assert!(issues[0].message.contains("unknown-id"));
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

        let issues = validate_connections(&workflow, &existing, &AgentCatalog::new());
        assert!(issues.is_empty());
    }

    // Exercises the candidate-suggestion mechanism with `shopify` — a
    // component agent with no compiled-in registry entry, which is exactly
    // the case the static `get_agents()` lookup used to be blind to. Now
    // that the guard reads the runtime catalog instead, suggestions work for
    // component agents the same way they always did for statically-compiled
    // ones.
    #[test]
    fn test_candidate_suggestions_on_missing_connection() {
        let workflow: Workflow = serde_json::from_value(json!({
            "executionGraph": {
                "steps": {
                    "step1": {
                        "stepType": "Agent",
                        "id": "step1",
                        "agentId": "shopify",
                        "capabilityId": "get-products",
                        "connectionId": "wrong-id"
                    }
                },
                "entryPoint": "step1",
                "executionPlan": []
            },
            "variables": []
        }))
        .unwrap();

        // Tenant has a shopify connection (matching integration_id) and a
        // postgres one (which should NOT be suggested for a shopify step).
        let tenant = vec![
            ConnectionRef {
                id: "conn-shopify".to_string(),
                integration_id: Some("shopify".to_string()),
                title: "My Shopify Store".to_string(),
            },
            ConnectionRef {
                id: "conn-db".to_string(),
                integration_id: Some("postgres".to_string()),
                title: "Object Model DB".to_string(),
            },
        ];

        let catalog = agent_catalog("shopify", &["shopify"]);
        let issues = validate_connections_with_candidates(&workflow, &tenant, &catalog);
        assert_eq!(issues.len(), 1, "expected one missing-connection issue");
        let msg = &issues[0].message;
        assert!(msg.contains("'wrong-id'"), "{msg}");
        assert!(
            msg.contains("My Shopify Store"),
            "should suggest the shopify connection: {msg}"
        );
        assert!(
            msg.contains("conn-shopify"),
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
                        "agentId": "shopify",
                        "capabilityId": "get-products",
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

        let catalog = agent_catalog("shopify", &["shopify"]);
        let issues = validate_connections_with_candidates(&workflow, &tenant, &catalog);
        assert_eq!(issues.len(), 1);
        let msg = &issues[0].message;
        assert!(
            msg.contains("shopify"),
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
                            "provider": {"valueType": "immediate", "value": provider}
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
    fn ai_agent_openai_connection_is_valid_when_it_exposes_ai_chat() {
        let workflow = ai_agent_workflow("openai", "conn-openai");
        let tenant = vec![ConnectionRef {
            id: "conn-openai".to_string(),
            integration_id: Some("openai_api_key".to_string()),
            title: "OpenAI".to_string(),
        }];

        let issues = validate_connections_with_candidates(&workflow, &tenant, &AgentCatalog::new());
        assert!(issues.is_empty(), "expected no issues, got {issues:?}");
    }

    #[test]
    fn ai_agent_legacy_provider_is_ignored_in_favor_of_connection_metadata() {
        let workflow = ai_agent_workflow("openai", "conn-aws");
        let tenant = vec![ConnectionRef {
            id: "conn-aws".to_string(),
            integration_id: Some("aws_credentials".to_string()),
            title: "AWS".to_string(),
        }];

        let issues = validate_connections_with_candidates(&workflow, &tenant, &AgentCatalog::new());
        assert!(
            issues.is_empty(),
            "connection metadata is authoritative: {issues:?}"
        );
    }

    #[test]
    fn ai_agent_rejects_connection_without_ai_chat_metadata() {
        let workflow = ai_agent_workflow("openai", "conn-db");
        let tenant = vec![ConnectionRef {
            id: "conn-db".to_string(),
            integration_id: Some("postgres".to_string()),
            title: "Database".to_string(),
        }];

        let issues = validate_connections_with_candidates(&workflow, &tenant, &AgentCatalog::new());
        assert_eq!(issues.len(), 1);
        assert!(
            issues[0].message.contains("ai.chat"),
            "{}",
            issues[0].message
        );
    }

    #[test]
    fn ai_agent_missing_connection_suggests_all_ai_connections() {
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
            ConnectionRef {
                id: "conn-db".to_string(),
                integration_id: Some("postgres".to_string()),
                title: "Database".to_string(),
            },
        ];

        let issues = validate_connections_with_candidates(&workflow, &tenant, &AgentCatalog::new());
        assert_eq!(issues.len(), 1);
        let msg = &issues[0].message;
        assert!(msg.contains("wrong-id"), "{msg}");
        assert!(msg.contains("Bedrock"), "{msg}");
        assert!(msg.contains("conn-bedrock"), "{msg}");
        assert!(msg.contains("conn-openai"), "{msg}");
        assert!(!msg.contains("conn-db"), "{msg}");
    }
}
