use rmcp::model::{CallToolResult, Content};
use schemars::JsonSchema;
use serde::Deserialize;

use super::super::server::SmoMcpServer;
use super::internal_api::{api_get, api_post, api_put, validate_path_param};

fn json_result(value: serde_json::Value) -> Result<CallToolResult, rmcp::ErrorData> {
    Ok(CallToolResult::success(vec![Content::text(
        serde_json::to_string_pretty(&value).unwrap_or_default(),
    )]))
}

fn err(msg: impl Into<String>) -> rmcp::ErrorData {
    rmcp::ErrorData::internal_error(msg.into(), None)
}

/// Returns true if the type name suggests the value supports nested dot-path access.
/// This includes objects, arrays, and any custom struct types (not primitives like string/integer/boolean).
fn supports_nested_access(type_name: &str) -> bool {
    let t = type_name.to_lowercase();
    // Primitives that don't support nested access
    if matches!(
        t.as_str(),
        "string"
            | "integer"
            | "number"
            | "boolean"
            | "float"
            | "double"
            | "i32"
            | "i64"
            | "u32"
            | "u64"
            | "f32"
            | "f64"
            | "bool"
            | "usize"
            | "unknown"
    ) {
        return false;
    }
    // Everything else (object, array, Vec<...>, Option<SomeStruct>, custom types) may have nested fields
    true
}

// ===== Shared Helpers =====

/// Validate an output path (e.g., "items", "result.name") against a capability's output schema.
/// Walks the path segments against known fields. Stops validating when the schema
/// doesn't have sub-field info (opaque/dynamic type) — further nesting is allowed but unverified.
/// Errors only when a segment doesn't match any known field at a level where fields ARE known.
fn validate_output_path(
    cap_data: &serde_json::Value,
    output_path: &str,
    step_id: &str,
    agent_id: &str,
    capability_id: &str,
) -> Result<(), rmcp::ErrorData> {
    let segments: Vec<&str> = output_path.split('.').collect();
    if segments.is_empty() {
        return Ok(());
    }

    // Start with top-level output fields: capability.output.fields
    let mut current_fields = cap_data
        .pointer("/output/fields")
        .and_then(|f| f.as_array());

    for (i, segment) in segments.iter().enumerate() {
        let Some(fields) = current_fields else {
            // No field metadata at this level — can't validate further, allow it
            return Ok(());
        };

        // Find the matching field
        let matched_field = fields.iter().find(|f| {
            f.get("name")
                .and_then(|n| n.as_str())
                .is_some_and(|n| n == *segment)
        });

        let Some(field) = matched_field else {
            let known: Vec<&str> = fields
                .iter()
                .filter_map(|f| f.get("name").and_then(|n| n.as_str()))
                .collect();
            let path_so_far = segments[..=i].join(".");
            return Err(err(format!(
                "Output field '{}' not found on step '{}' ({}/{}). Known fields at this level: [{}]",
                path_so_far,
                step_id,
                agent_id,
                capability_id,
                known.join(", ")
            )));
        };

        // If there are more segments, try to descend into sub-fields
        if i < segments.len() - 1 {
            current_fields = field.get("fields").and_then(|f| f.as_array());
            // If no sub-fields but type supports nesting, allow remaining segments unverified
        }
    }

    Ok(())
}

/// Fetch the latest execution graph for a workflow.
/// Returns (execution_graph, latest_version, current_version).
///
/// When an unpublished draft exists (latest > current), `GET /workflows/{id}` without
/// `versionNumber` returns the *published* version's graph — so subsequent mutations
/// would re-base off current and clobber earlier edits when PUT in-place to latest.
/// We detect that case and re-fetch with `versionNumber=latest` so mutations stack
/// on the draft.
async fn fetch_latest_graph(
    server: &SmoMcpServer,
    workflow_id: &str,
) -> Result<(serde_json::Value, i64, i64), rmcp::ErrorData> {
    validate_path_param("workflow_id", workflow_id)?;
    let result = api_get(server, &format!("/api/runtime/workflows/{}", workflow_id)).await?;

    let latest_version = result
        .pointer("/data/latestVersion")
        .or_else(|| result.pointer("/data/latest_version"))
        .and_then(|v| v.as_i64())
        .unwrap_or(1);

    let current_version = result
        .pointer("/data/currentVersion")
        .or_else(|| result.pointer("/data/current_version"))
        .and_then(|v| v.as_i64())
        .unwrap_or(latest_version);

    let graph_source = if latest_version != current_version {
        api_get(
            server,
            &format!(
                "/api/runtime/workflows/{}?versionNumber={}",
                workflow_id, latest_version
            ),
        )
        .await?
    } else {
        result
    };

    let graph = graph_source
        .pointer("/data/definition/executionGraph")
        .or_else(|| graph_source.pointer("/data/executionGraph"))
        .cloned()
        .ok_or_else(|| err("Workflow has no executionGraph"))?;

    Ok((graph, latest_version, current_version))
}

/// Save the graph — creates a new version if latest == current (first mutation),
/// otherwise patches the latest version in-place (subsequent mutations).
///
/// For the first mutation, we create a new version by POSTing the EXISTING valid graph,
/// then immediately patch it with the mutated graph via PUT. This avoids running full
/// workflow validation (reachability, connections) on an incomplete graph — PUT only
/// checks DSL structure, with full checks deferred to compile time.
/// Returns (version, is_new_version).
async fn save_graph(
    server: &SmoMcpServer,
    workflow_id: &str,
    graph: serde_json::Value,
    latest_version: i64,
    current_version: i64,
) -> Result<(String, bool), rmcp::ErrorData> {
    if latest_version == current_version {
        // First mutation — create a new version. Strategy:
        //
        // 1. Try POST /update with the MUTATED graph. If the mutation made the
        //    graph valid (e.g., connect_steps completing reachability), this succeeds.
        //
        // 2. If that fails with validation (e.g., adding an unreachable step),
        //    try POST /update with the EXISTING graph (which was valid before the
        //    mutation), then patch the new version via PUT (which skips reachability).
        //
        // 3. If the existing graph is also invalid or empty, return the original error.
        let mutated_body = serde_json::json!({ "executionGraph": graph });
        let result = api_post(
            server,
            &format!("/api/runtime/workflows/{}/update", workflow_id),
            Some(mutated_body),
        )
        .await;

        match result {
            Ok(res) => {
                // Mutated graph passed validation — done
                let version = res
                    .get("version")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown")
                    .to_string();
                Ok((version, true))
            }
            Err(original_err) => {
                // Mutated graph failed validation — try existing graph + patch
                let existing_graph =
                    fetch_latest_graph(server, workflow_id)
                        .await
                        .ok()
                        .and_then(|(g, _, _)| {
                            let has_steps = g
                                .get("steps")
                                .and_then(|s| s.as_object())
                                .is_some_and(|s| !s.is_empty());
                            if has_steps { Some(g) } else { None }
                        });

                let Some(existing) = existing_graph else {
                    // No valid existing graph to fall back to
                    return Err(original_err);
                };

                let existing_body = serde_json::json!({ "executionGraph": existing });
                let res = api_post(
                    server,
                    &format!("/api/runtime/workflows/{}/update", workflow_id),
                    Some(existing_body),
                )
                .await
                .map_err(|_| original_err)?;

                let version = res
                    .get("version")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown")
                    .to_string();

                // Patch with the mutated graph (PUT skips reachability validation)
                let patch_body = serde_json::json!({ "executionGraph": graph });
                api_put(
                    server,
                    &format!(
                        "/api/runtime/workflows/{}/versions/{}/graph",
                        workflow_id, version
                    ),
                    Some(patch_body),
                )
                .await?;

                Ok((version, true))
            }
        }
    } else {
        // Subsequent mutation — patch in-place via PUT /versions/{v}/graph
        let body = serde_json::json!({ "executionGraph": graph });
        api_put(
            server,
            &format!(
                "/api/runtime/workflows/{}/versions/{}/graph",
                workflow_id, latest_version
            ),
            Some(body),
        )
        .await?;
        Ok((latest_version.to_string(), false))
    }
}

/// Navigate into nested subgraphs by following the path.
/// Each element is a step ID whose `subgraph` field to descend into.
fn resolve_graph_mut<'a>(
    graph: &'a mut serde_json::Value,
    path: &[String],
) -> Result<&'a mut serde_json::Value, rmcp::ErrorData> {
    let mut current = graph;
    for step_id in path {
        let steps = current
            .get("steps")
            .and_then(|s| s.as_object())
            .ok_or_else(|| {
                err(format!(
                    "No steps found during path traversal at '{}'",
                    step_id
                ))
            })?;

        if !steps.contains_key(step_id) {
            return Err(err(format!(
                "Step '{}' not found in path traversal",
                step_id
            )));
        }

        let step = current
            .get_mut("steps")
            .and_then(|s| s.as_object_mut())
            .and_then(|s| s.get_mut(step_id))
            .unwrap();

        if step.get("subgraph").is_none() {
            return Err(err(format!("Step '{}' does not have a subgraph", step_id)));
        }

        current = step.get_mut("subgraph").unwrap();
    }
    Ok(current)
}

// ===== Parameter Structs =====

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AddStepParams {
    #[schemars(description = "Workflow ID")]
    pub workflow_id: String,
    #[schemars(description = "Unique step ID within the graph")]
    pub step_id: String,
    #[schemars(
        description = "Step definition JSON. Must include 'stepType'. For Agent steps: {\"stepType\": \"Agent\", \"name\": \"...\", \"agentId\": \"http\", \"capabilityId\": \"http-request\", \"inputMapping\": {\"url\": {\"valueType\": \"immediate\", \"value\": \"...\"}}}. Field is 'inputMapping' (SINGULAR)."
    )]
    pub step: serde_json::Value,
    #[schemars(
        description = "Path to nested subgraph — array of step IDs to traverse (e.g., [\"split1\", \"inner_while\"]). Omit for root graph."
    )]
    pub path: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RemoveStepParams {
    #[schemars(description = "Workflow ID")]
    pub workflow_id: String,
    #[schemars(description = "Step ID to remove")]
    pub step_id: String,
    #[schemars(
        description = "Path to nested subgraph — array of step IDs to traverse. Omit for root graph."
    )]
    pub path: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateStepParams {
    #[schemars(description = "Workflow ID")]
    pub workflow_id: String,
    #[schemars(description = "Step ID to update")]
    pub step_id: String,
    #[schemars(description = "New step definition JSON (must include 'stepType')")]
    pub step: serde_json::Value,
    #[schemars(
        description = "Path to nested subgraph — array of step IDs to traverse. Omit for root graph."
    )]
    pub path: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConnectStepsParams {
    #[schemars(description = "Workflow ID")]
    pub workflow_id: String,
    #[schemars(description = "Source step ID")]
    pub from_step: String,
    #[schemars(description = "Target step ID")]
    pub to_step: String,
    #[schemars(description = "Edge label (e.g., 'true', 'false' for conditionals)")]
    pub label: Option<String>,
    #[schemars(description = "Condition expression JSON for the edge")]
    pub condition: Option<serde_json::Value>,
    #[schemars(description = "Edge priority (lower = evaluated first)")]
    pub priority: Option<i64>,
    #[schemars(
        description = "Path to nested subgraph — array of step IDs to traverse. Omit for root graph."
    )]
    pub path: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DisconnectStepsParams {
    #[schemars(description = "Workflow ID")]
    pub workflow_id: String,
    #[schemars(description = "Source step ID")]
    pub from_step: String,
    #[schemars(description = "Target step ID")]
    pub to_step: String,
    #[schemars(description = "Only remove edges with this label (if specified)")]
    pub label: Option<String>,
    #[schemars(
        description = "Path to nested subgraph — array of step IDs to traverse. Omit for root graph."
    )]
    pub path: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetEntryPointParams {
    #[schemars(description = "Workflow ID")]
    pub workflow_id: String,
    #[schemars(description = "Step ID to set as entry point")]
    pub step_id: String,
    #[schemars(
        description = "Path to nested subgraph — array of step IDs to traverse. Omit for root graph."
    )]
    pub path: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetMappingParams {
    #[schemars(description = "Workflow ID")]
    pub workflow_id: String,
    #[schemars(description = "Step ID to set mapping on")]
    pub step_id: String,
    #[schemars(description = "Input field name to map")]
    pub input_name: String,
    #[schemars(
        description = "Reference a step's output: the step ID (e.g., \"generate_random\"). Builds: steps.<from_step>.outputs.<from_output>"
    )]
    pub from_step: Option<String>,
    #[schemars(
        description = "Output field path from the referenced step (e.g., \"value\", \"items\", \"result.name\", \"data.orders\"). Supports dot-notation for nested object access. Required when from_step is set."
    )]
    pub from_output: Option<String>,
    #[schemars(
        description = "Reference a workflow input field (e.g., \"orderId\", \"config.mode\"). Supports dot-notation for nested access. Builds: data.<from_input>"
    )]
    pub from_input: Option<String>,
    #[schemars(
        description = "Reference a variable (e.g., \"counter\"). Builds: variables.<from_variable>"
    )]
    pub from_variable: Option<String>,
    #[schemars(
        description = "Set a literal/immediate value (string, number, boolean, object, array)"
    )]
    pub immediate_value: Option<serde_json::Value>,
    #[schemars(
        description = "Path to nested subgraph — array of step IDs to traverse. Omit for root graph."
    )]
    pub path: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListReferencesParams {
    #[schemars(description = "Workflow ID")]
    pub workflow_id: String,
    #[schemars(
        description = "Path to nested subgraph — array of step IDs to traverse. Omit for root graph."
    )]
    pub path: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RemoveMappingParams {
    #[schemars(description = "Workflow ID")]
    pub workflow_id: String,
    #[schemars(description = "Step ID to remove mapping from")]
    pub step_id: String,
    #[schemars(description = "Input field name to remove")]
    pub input_name: String,
    #[schemars(
        description = "Path to nested subgraph — array of step IDs to traverse. Omit for root graph."
    )]
    pub path: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetInputSchemaParams {
    #[schemars(description = "Workflow ID")]
    pub workflow_id: String,
    #[schemars(
        description = "Input schema fields in DSL flat-map format (e.g., {\"orderId\": {\"type\": \"string\", \"required\": true}})"
    )]
    pub fields: serde_json::Value,
    #[schemars(
        description = "Path to nested subgraph — array of step IDs to traverse. Omit for root graph."
    )]
    pub path: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetOutputSchemaParams {
    #[schemars(description = "Workflow ID")]
    pub workflow_id: String,
    #[schemars(
        description = "Output schema fields in DSL flat-map format (e.g., {\"result\": {\"type\": \"string\", \"required\": true}})"
    )]
    pub fields: serde_json::Value,
    #[schemars(
        description = "Path to nested subgraph — array of step IDs to traverse. Omit for root graph."
    )]
    pub path: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetVariableParams {
    #[schemars(description = "Workflow ID")]
    pub workflow_id: String,
    #[schemars(description = "Variable name")]
    pub name: String,
    #[schemars(description = "Variable definition JSON")]
    pub variable: serde_json::Value,
    #[schemars(
        description = "Path to nested subgraph — array of step IDs to traverse. Omit for root graph."
    )]
    pub path: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RemoveVariableParams {
    #[schemars(description = "Workflow ID")]
    pub workflow_id: String,
    #[schemars(description = "Variable name to remove")]
    pub name: String,
    #[schemars(
        description = "Path to nested subgraph — array of step IDs to traverse. Omit for root graph."
    )]
    pub path: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetWorkflowMetadataParams {
    #[schemars(description = "Workflow ID")]
    pub workflow_id: String,
    #[schemars(description = "Workflow name")]
    pub name: Option<String>,
    #[schemars(description = "Workflow description")]
    pub description: Option<String>,
    #[schemars(
        description = "Path to nested subgraph — array of step IDs to traverse. Omit for root graph."
    )]
    pub path: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AddAgentStepParams {
    #[schemars(description = "Workflow ID")]
    pub workflow_id: String,
    #[schemars(description = "Unique step ID within the graph")]
    pub step_id: String,
    #[schemars(description = "Human-readable step name")]
    pub step_name: String,
    #[schemars(description = "Agent ID (e.g., 'shopify', 'http'). Use list_agents to discover.")]
    pub agent_id: String,
    #[schemars(
        description = "Capability ID (e.g., 'http-request', 'get-product-variant-by-sku'). Use get_agent to discover."
    )]
    pub capability_id: String,
    #[schemars(
        description = "Step ID to connect after (creates a normal edge from that step to this one)"
    )]
    pub connect_after: Option<String>,
    #[schemars(
        description = "Step ID to route errors to (creates an onError edge from this step)"
    )]
    pub on_error_step: Option<String>,
    #[schemars(
        description = "Path to nested subgraph — array of step IDs to traverse. Omit for root graph."
    )]
    pub path: Option<Vec<String>>,
}

// ===== Tool Implementations =====

pub async fn add_step(
    server: &SmoMcpServer,
    params: AddStepParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    if params.step.get("stepType").is_none() {
        return Err(err("Step definition must include 'stepType' field"));
    }

    // Typo linting: catch common field name mistakes
    if params.step.get("inputMappings").is_some() {
        return Err(err(
            "Found 'inputMappings' (plural) — use 'inputMapping' (singular)",
        ));
    }
    if params.step.get("agent").is_some() && params.step.get("agentId").is_none() {
        return Err(err("Found 'agent' — use 'agentId' instead"));
    }
    if params.step.get("capability").is_some() && params.step.get("capabilityId").is_none() {
        return Err(err("Found 'capability' — use 'capabilityId' instead"));
    }

    let (mut graph, latest, current) = fetch_latest_graph(server, &params.workflow_id).await?;
    let path = params.path.unwrap_or_default();
    let target = resolve_graph_mut(&mut graph, &path)?;

    // Check step doesn't already exist
    if target
        .get("steps")
        .and_then(|s| s.as_object())
        .is_some_and(|s| s.contains_key(&params.step_id))
    {
        return Err(err(format!(
            "Step '{}' already exists in graph",
            params.step_id
        )));
    }

    // Ensure steps object exists
    if target.get("steps").is_none() || !target["steps"].is_object() {
        target["steps"] = serde_json::json!({});
    }

    // Insert step with id field set
    let mut step = params.step;
    step["id"] = serde_json::Value::String(params.step_id.clone());
    target["steps"][&params.step_id] = step;

    // If this is the first step, set it as entry point
    let steps_count = target["steps"].as_object().map(|s| s.len()).unwrap_or(0);
    let set_entry = steps_count == 1;
    if set_entry {
        target["entryPoint"] = serde_json::Value::String(params.step_id.clone());
    }

    let (version, new_version) =
        save_graph(server, &params.workflow_id, graph, latest, current).await?;
    json_result(serde_json::json!({
        "success": true,
        "workflowId": params.workflow_id,
        "version": version,
        "newVersion": new_version,
        "stepId": params.step_id,
        "setAsEntryPoint": set_entry,
    }))
}

pub async fn remove_step(
    server: &SmoMcpServer,
    params: RemoveStepParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    let (mut graph, latest, current) = fetch_latest_graph(server, &params.workflow_id).await?;
    let path = params.path.unwrap_or_default();
    let target = resolve_graph_mut(&mut graph, &path)?;

    let step_exists = target
        .get("steps")
        .and_then(|s| s.as_object())
        .is_some_and(|s| s.contains_key(&params.step_id));
    if !step_exists {
        return Err(err(format!("Step '{}' not found in graph", params.step_id)));
    }

    target["steps"]
        .as_object_mut()
        .unwrap()
        .remove(&params.step_id);

    // Remove edges referencing this step
    let mut removed_edges = 0;
    if let Some(plan) = target
        .get_mut("executionPlan")
        .and_then(|p| p.as_array_mut())
    {
        let before = plan.len();
        plan.retain(|edge| {
            let from = edge.get("fromStep").and_then(|v| v.as_str()).unwrap_or("");
            let to = edge.get("toStep").and_then(|v| v.as_str()).unwrap_or("");
            from != params.step_id && to != params.step_id
        });
        removed_edges = before - plan.len();
    }

    let was_entry_point = target
        .get("entryPoint")
        .and_then(|v| v.as_str())
        .is_some_and(|ep| ep == params.step_id);

    let (version, new_version) =
        save_graph(server, &params.workflow_id, graph, latest, current).await?;
    json_result(serde_json::json!({
        "success": true,
        "workflowId": params.workflow_id,
        "version": version,
        "newVersion": new_version,
        "removedStepId": params.step_id,
        "removedEdges": removed_edges,
        "warning": if was_entry_point {
            Some("Removed step was the entryPoint — you should set a new entry point")
        } else {
            None
        },
    }))
}

pub async fn update_step(
    server: &SmoMcpServer,
    params: UpdateStepParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    if params.step.get("stepType").is_none() {
        return Err(err("Step definition must include 'stepType' field"));
    }

    let (mut graph, latest, current) = fetch_latest_graph(server, &params.workflow_id).await?;
    let path = params.path.unwrap_or_default();
    let target = resolve_graph_mut(&mut graph, &path)?;

    let step_exists = target
        .get("steps")
        .and_then(|s| s.as_object())
        .is_some_and(|s| s.contains_key(&params.step_id));
    if !step_exists {
        return Err(err(format!("Step '{}' not found in graph", params.step_id)));
    }

    let mut step = params.step;
    step["id"] = serde_json::Value::String(params.step_id.clone());
    target["steps"][&params.step_id] = step;

    let (version, new_version) =
        save_graph(server, &params.workflow_id, graph, latest, current).await?;
    json_result(serde_json::json!({
        "success": true,
        "workflowId": params.workflow_id,
        "version": version,
        "newVersion": new_version,
        "stepId": params.step_id,
    }))
}

pub async fn connect_steps(
    server: &SmoMcpServer,
    params: ConnectStepsParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    // Typo linting: catch common label mistakes
    if let Some(label) = &params.label {
        let lower = label.to_lowercase();
        if (lower == "on_error" || lower == "onerror") && label != "onError" {
            return Err(err(format!(
                "Label '{}' looks like a typo — use 'onError' (camelCase)",
                label
            )));
        }
    }

    let (mut graph, latest, current) = fetch_latest_graph(server, &params.workflow_id).await?;
    let path = params.path.unwrap_or_default();
    let target = resolve_graph_mut(&mut graph, &path)?;

    let steps = target
        .get("steps")
        .and_then(|s| s.as_object())
        .ok_or_else(|| err("No steps in graph"))?;
    if !steps.contains_key(&params.from_step) {
        return Err(err(format!(
            "Step '{}' not found in graph",
            params.from_step
        )));
    }
    if !steps.contains_key(&params.to_step) {
        return Err(err(format!("Step '{}' not found in graph", params.to_step)));
    }

    if target.get("executionPlan").is_none() || !target["executionPlan"].is_array() {
        target["executionPlan"] = serde_json::json!([]);
    }

    // Check for duplicate edge
    let plan = target["executionPlan"].as_array().unwrap();
    let duplicate = plan.iter().any(|edge| {
        let from = edge.get("fromStep").and_then(|v| v.as_str()).unwrap_or("");
        let to = edge.get("toStep").and_then(|v| v.as_str()).unwrap_or("");
        let lbl = edge.get("label").and_then(|v| v.as_str());
        from == params.from_step && to == params.to_step && lbl == params.label.as_deref()
    });
    if duplicate {
        return Err(err(format!(
            "Edge from '{}' to '{}' already exists",
            params.from_step, params.to_step
        )));
    }

    let mut edge = serde_json::json!({
        "fromStep": params.from_step,
        "toStep": params.to_step,
    });
    if let Some(label) = &params.label {
        edge["label"] = serde_json::Value::String(label.clone());
    }
    if let Some(condition) = params.condition {
        edge["condition"] = condition;
    }
    if let Some(priority) = params.priority {
        edge["priority"] = serde_json::Value::Number(priority.into());
    }

    target["executionPlan"].as_array_mut().unwrap().push(edge);

    let (version, new_version) =
        save_graph(server, &params.workflow_id, graph, latest, current).await?;
    json_result(serde_json::json!({
        "success": true,
        "workflowId": params.workflow_id,
        "version": version,
        "newVersion": new_version,
        "fromStep": params.from_step,
        "toStep": params.to_step,
    }))
}

pub async fn disconnect_steps(
    server: &SmoMcpServer,
    params: DisconnectStepsParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    let (mut graph, latest, current) = fetch_latest_graph(server, &params.workflow_id).await?;
    let path = params.path.unwrap_or_default();
    let target = resolve_graph_mut(&mut graph, &path)?;

    let plan = target
        .get_mut("executionPlan")
        .and_then(|p| p.as_array_mut())
        .ok_or_else(|| {
            err(format!(
                "No edges found from '{}' to '{}'",
                params.from_step, params.to_step
            ))
        })?;

    let before = plan.len();
    plan.retain(|edge| {
        let from = edge.get("fromStep").and_then(|v| v.as_str()).unwrap_or("");
        let to = edge.get("toStep").and_then(|v| v.as_str()).unwrap_or("");
        if from != params.from_step || to != params.to_step {
            return true;
        }
        if let Some(ref label) = params.label {
            let edge_label = edge.get("label").and_then(|v| v.as_str()).unwrap_or("");
            return edge_label != label.as_str();
        }
        false
    });
    let removed = before - plan.len();

    if removed == 0 {
        return Err(err(format!(
            "No edges found from '{}' to '{}'",
            params.from_step, params.to_step
        )));
    }

    let (version, new_version) =
        save_graph(server, &params.workflow_id, graph, latest, current).await?;
    json_result(serde_json::json!({
        "success": true,
        "workflowId": params.workflow_id,
        "version": version,
        "newVersion": new_version,
        "removedEdges": removed,
    }))
}

pub async fn set_entry_point(
    server: &SmoMcpServer,
    params: SetEntryPointParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    let (mut graph, latest, current) = fetch_latest_graph(server, &params.workflow_id).await?;
    let path = params.path.unwrap_or_default();
    let target = resolve_graph_mut(&mut graph, &path)?;

    let step_exists = target
        .get("steps")
        .and_then(|s| s.as_object())
        .is_some_and(|s| s.contains_key(&params.step_id));
    if !step_exists {
        return Err(err(format!("Step '{}' not found in graph", params.step_id)));
    }

    target["entryPoint"] = serde_json::Value::String(params.step_id.clone());

    let (version, new_version) =
        save_graph(server, &params.workflow_id, graph, latest, current).await?;
    json_result(serde_json::json!({
        "success": true,
        "workflowId": params.workflow_id,
        "version": version,
        "newVersion": new_version,
        "entryPoint": params.step_id,
    }))
}

pub async fn set_mapping(
    server: &SmoMcpServer,
    params: SetMappingParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    // Build the mapping value from typed parameters
    let mapping_value = if let Some(ref from_step) = params.from_step {
        let from_output = params
            .from_output
            .as_deref()
            .ok_or_else(|| err("from_output is required when from_step is set"))?;
        serde_json::json!({
            "valueType": "reference",
            "value": format!("steps.{}.outputs.{}", from_step, from_output)
        })
    } else if let Some(ref from_input) = params.from_input {
        serde_json::json!({
            "valueType": "reference",
            "value": format!("data.{}", from_input)
        })
    } else if let Some(ref from_variable) = params.from_variable {
        serde_json::json!({
            "valueType": "reference",
            "value": format!("variables.{}", from_variable)
        })
    } else if let Some(ref immediate) = params.immediate_value {
        serde_json::json!({
            "valueType": "immediate",
            "value": immediate
        })
    } else {
        return Err(err(
            "Exactly one of from_step+from_output, from_input, from_variable, or immediate_value must be provided",
        ));
    };

    let (mut graph, latest, current) = fetch_latest_graph(server, &params.workflow_id).await?;
    let path = params.path.unwrap_or_default();
    let target = resolve_graph_mut(&mut graph, &path)?;

    // Validate referenced step exists and output path is valid (if from_step)
    if let Some(ref from_step) = params.from_step {
        let step_exists = target
            .get("steps")
            .and_then(|s| s.as_object())
            .is_some_and(|s| s.contains_key(from_step));
        if !step_exists {
            return Err(err(format!(
                "Referenced step '{}' not found in graph",
                from_step
            )));
        }

        // Validate output path against capability schema (as deep as possible)
        let from_output = params.from_output.as_deref().unwrap_or("");
        let step_def = target.get("steps").and_then(|s| s.get(from_step)).unwrap();
        let is_agent = step_def
            .get("stepType")
            .and_then(|t| t.as_str())
            .is_some_and(|t| t == "Agent");
        if is_agent
            && let (Some(agent_id), Some(capability_id)) = (
                step_def.get("agentId").and_then(|a| a.as_str()),
                step_def.get("capabilityId").and_then(|c| c.as_str()),
            )
        {
            let cap_result = api_get(
                server,
                &format!(
                    "/api/runtime/agents/{}/capabilities/{}",
                    agent_id, capability_id
                ),
            )
            .await;
            if let Ok(cap_data) = cap_result {
                validate_output_path(&cap_data, from_output, from_step, agent_id, capability_id)?;
            }
        }
    }

    // Validate referenced input exists (if from_input) — check root key only (dot-paths into objects can't be verified)
    if let Some(ref from_input) = params.from_input {
        let root_key = from_input.split('.').next().unwrap_or(from_input);
        let input_exists = target
            .get("inputSchema")
            .and_then(|s| s.as_object())
            .is_some_and(|s| s.contains_key(root_key));
        if !input_exists {
            return Err(err(format!(
                "Referenced input '{}' not found in inputSchema",
                root_key
            )));
        }
    }

    // Validate referenced variable exists (if from_variable) — check root key only
    if let Some(ref from_variable) = params.from_variable {
        let root_key = from_variable.split('.').next().unwrap_or(from_variable);
        let var_exists = target
            .get("variables")
            .and_then(|s| s.as_object())
            .is_some_and(|s| s.contains_key(root_key));
        if !var_exists {
            return Err(err(format!(
                "Referenced variable '{}' not found in variables",
                root_key
            )));
        }
    }

    let step = target
        .get_mut("steps")
        .and_then(|s| s.as_object_mut())
        .and_then(|s| s.get_mut(&params.step_id))
        .ok_or_else(|| err(format!("Step '{}' not found in graph", params.step_id)))?;

    if step.get("inputMapping").is_none() || !step["inputMapping"].is_object() {
        step["inputMapping"] = serde_json::json!({});
    }

    step["inputMapping"][&params.input_name] = mapping_value;

    let (version, new_version) =
        save_graph(server, &params.workflow_id, graph, latest, current).await?;
    json_result(serde_json::json!({
        "success": true,
        "workflowId": params.workflow_id,
        "version": version,
        "newVersion": new_version,
        "stepId": params.step_id,
        "inputName": params.input_name,
    }))
}

pub async fn remove_mapping(
    server: &SmoMcpServer,
    params: RemoveMappingParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    let (mut graph, latest, current) = fetch_latest_graph(server, &params.workflow_id).await?;
    let path = params.path.unwrap_or_default();
    let target = resolve_graph_mut(&mut graph, &path)?;

    let step = target
        .get_mut("steps")
        .and_then(|s| s.as_object_mut())
        .and_then(|s| s.get_mut(&params.step_id))
        .ok_or_else(|| err(format!("Step '{}' not found in graph", params.step_id)))?;

    let mapping = step
        .get_mut("inputMapping")
        .and_then(|m| m.as_object_mut())
        .ok_or_else(|| {
            err(format!(
                "Input mapping '{}' not found on step '{}'",
                params.input_name, params.step_id
            ))
        })?;

    if mapping.remove(&params.input_name).is_none() {
        return Err(err(format!(
            "Input mapping '{}' not found on step '{}'",
            params.input_name, params.step_id
        )));
    }

    let (version, new_version) =
        save_graph(server, &params.workflow_id, graph, latest, current).await?;
    json_result(serde_json::json!({
        "success": true,
        "workflowId": params.workflow_id,
        "version": version,
        "newVersion": new_version,
        "stepId": params.step_id,
        "removedInput": params.input_name,
    }))
}

pub async fn set_input_schema(
    server: &SmoMcpServer,
    params: SetInputSchemaParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    let (mut graph, latest, current) = fetch_latest_graph(server, &params.workflow_id).await?;
    let path = params.path.unwrap_or_default();
    let target = resolve_graph_mut(&mut graph, &path)?;

    target["inputSchema"] = params.fields;

    let (version, new_version) =
        save_graph(server, &params.workflow_id, graph, latest, current).await?;
    json_result(serde_json::json!({
        "success": true,
        "workflowId": params.workflow_id,
        "version": version,
        "newVersion": new_version,
    }))
}

pub async fn set_output_schema(
    server: &SmoMcpServer,
    params: SetOutputSchemaParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    let (mut graph, latest, current) = fetch_latest_graph(server, &params.workflow_id).await?;
    let path = params.path.unwrap_or_default();
    let target = resolve_graph_mut(&mut graph, &path)?;

    target["outputSchema"] = params.fields;

    let (version, new_version) =
        save_graph(server, &params.workflow_id, graph, latest, current).await?;
    json_result(serde_json::json!({
        "success": true,
        "workflowId": params.workflow_id,
        "version": version,
        "newVersion": new_version,
    }))
}

pub async fn set_variable(
    server: &SmoMcpServer,
    params: SetVariableParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    let (mut graph, latest, current) = fetch_latest_graph(server, &params.workflow_id).await?;
    let path = params.path.unwrap_or_default();
    let target = resolve_graph_mut(&mut graph, &path)?;

    if target.get("variables").is_none() || !target["variables"].is_object() {
        target["variables"] = serde_json::json!({});
    }

    target["variables"][&params.name] = params.variable;

    let (version, new_version) =
        save_graph(server, &params.workflow_id, graph, latest, current).await?;
    json_result(serde_json::json!({
        "success": true,
        "workflowId": params.workflow_id,
        "version": version,
        "newVersion": new_version,
        "variable": params.name,
    }))
}

pub async fn list_references(
    server: &SmoMcpServer,
    params: ListReferencesParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    let (mut graph, _latest, _current) = fetch_latest_graph(server, &params.workflow_id).await?;
    let path = params.path.unwrap_or_default();
    let target = resolve_graph_mut(&mut graph, &path)?;

    let mut references: Vec<serde_json::Value> = Vec::new();

    // 1. Workflow inputs → data.<field>
    if let Some(input_schema) = target.get("inputSchema").and_then(|s| s.as_object()) {
        for (field, schema) in input_schema {
            let field_type = schema
                .get("type")
                .and_then(|t| t.as_str())
                .unwrap_or("unknown");
            let ref_path = format!("data.{}", field);
            let mut entry = serde_json::json!({
                "reference": &ref_path,
                "source": "input",
                "field": field,
                "type": field_type,
                "mapping": { "valueType": "reference", "value": &ref_path },
                "setMappingArgs": { "from_input": field },
            });
            if supports_nested_access(field_type) {
                entry["nested"] = serde_json::json!(true);
                entry["note"] = serde_json::json!(
                    "Object/array type — use dot-notation for nested access (e.g., data.{field}.subField)"
                );
            }
            references.push(entry);
        }
    }

    // 2. Variables → variables.<name>
    if let Some(variables) = target.get("variables").and_then(|v| v.as_object()) {
        for (name, var_def) in variables {
            let var_type = var_def
                .get("type")
                .and_then(|t| t.as_str())
                .unwrap_or("unknown");
            let ref_path = format!("variables.{}", name);
            let mut entry = serde_json::json!({
                "reference": &ref_path,
                "source": "variable",
                "field": name,
                "type": var_type,
                "mapping": { "valueType": "reference", "value": &ref_path },
                "setMappingArgs": { "from_variable": name },
            });
            if supports_nested_access(var_type) {
                entry["nested"] = serde_json::json!(true);
                entry["note"] = serde_json::json!(
                    "Object/array type — use dot-notation for nested access (e.g., variables.{name}.subField)"
                );
            }
            references.push(entry);
        }
    }

    // 3. Step outputs → steps.<stepId>.outputs.<field>
    // For Agent steps, fetch capability output schema from the agents API
    if let Some(steps) = target.get("steps").and_then(|s| s.as_object()) {
        for (step_id, step_def) in steps {
            let step_type = step_def
                .get("stepType")
                .and_then(|t| t.as_str())
                .unwrap_or("");

            if step_type == "Agent" {
                let agent_id = step_def.get("agentId").and_then(|a| a.as_str());
                let capability_id = step_def.get("capabilityId").and_then(|c| c.as_str());

                if let (Some(agent_id), Some(capability_id)) = (agent_id, capability_id) {
                    // Fetch capability output schema from agents API
                    let cap_result = api_get(
                        server,
                        &format!(
                            "/api/runtime/agents/{}/capabilities/{}",
                            agent_id, capability_id
                        ),
                    )
                    .await;

                    if let Ok(cap_data) = cap_result {
                        // CapabilityInfo serializes as: { "output": { "type": "...", "fields": [...] } }
                        if let Some(fields) = cap_data
                            .pointer("/output/fields")
                            .and_then(|f| f.as_array())
                        {
                            for output_field in fields {
                                let Some(field_name) =
                                    output_field.get("name").and_then(|n| n.as_str())
                                else {
                                    continue;
                                };
                                let field_type = output_field
                                    .get("type")
                                    .and_then(|t| t.as_str())
                                    .unwrap_or("unknown");
                                let ref_path = format!("steps.{}.outputs.{}", step_id, field_name);
                                let mut entry = serde_json::json!({
                                    "reference": &ref_path,
                                    "source": "step",
                                    "stepId": step_id,
                                    "agentId": agent_id,
                                    "capabilityId": capability_id,
                                    "field": field_name,
                                    "type": field_type,
                                    "mapping": { "valueType": "reference", "value": &ref_path },
                                    "setMappingArgs": { "from_step": step_id, "from_output": field_name },
                                });
                                if supports_nested_access(field_type) {
                                    entry["nested"] = serde_json::json!(true);
                                    entry["note"] = serde_json::json!(format!(
                                        "Object/array type — use dot-notation for nested access (e.g., steps.{}.outputs.{}.subField)",
                                        step_id, field_name
                                    ));
                                }
                                references.push(entry);
                            }
                        }
                    }
                }
            } else if step_type == "Conditional" || step_type == "Finish" {
                // These step types don't produce outputs
            } else {
                // For other step types (Split, While, Log, etc.), note the step exists
                // but output fields depend on subgraph configuration
                references.push(serde_json::json!({
                    "reference": format!("steps.{}.outputs", step_id),
                    "source": "step",
                    "stepId": step_id,
                    "stepType": step_type,
                    "note": "Use get_capability or get_step_type_schema for output fields",
                }));
            }
        }
    }

    json_result(serde_json::json!({
        "workflowId": params.workflow_id,
        "references": references,
        "count": references.len(),
    }))
}

pub async fn set_workflow_metadata(
    server: &SmoMcpServer,
    params: SetWorkflowMetadataParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    if params.name.is_none() && params.description.is_none() {
        return Err(err(
            "At least one of 'name' or 'description' must be provided",
        ));
    }

    let (mut graph, latest, current) = fetch_latest_graph(server, &params.workflow_id).await?;
    let path = params.path.unwrap_or_default();
    let target = resolve_graph_mut(&mut graph, &path)?;

    if let Some(name) = &params.name {
        target["name"] = serde_json::Value::String(name.clone());
    }
    if let Some(description) = &params.description {
        target["description"] = serde_json::Value::String(description.clone());
    }

    let (version, new_version) =
        save_graph(server, &params.workflow_id, graph, latest, current).await?;
    json_result(serde_json::json!({
        "success": true,
        "workflowId": params.workflow_id,
        "version": version,
        "newVersion": new_version,
        "name": params.name,
        "description": params.description,
    }))
}

pub async fn add_agent_step(
    server: &SmoMcpServer,
    params: AddAgentStepParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    validate_path_param("workflow_id", &params.workflow_id)?;

    // Validate agent/capability exist and get capability info
    let cap_result = api_get(
        server,
        &format!(
            "/api/runtime/agents/{}/capabilities/{}",
            params.agent_id, params.capability_id
        ),
    )
    .await
    .map_err(|_| {
        err(format!(
            "Capability '{}/{}' not found. Use list_agents and get_agent to discover valid IDs.",
            params.agent_id, params.capability_id
        ))
    })?;

    // Build step definition
    let step = serde_json::json!({
        "id": params.step_id,
        "stepType": "Agent",
        "name": params.step_name,
        "agentId": params.agent_id,
        "capabilityId": params.capability_id,
    });

    // Add the step
    let (mut graph, latest, current) = fetch_latest_graph(server, &params.workflow_id).await?;
    let path = params.path.clone().unwrap_or_default();
    let target = resolve_graph_mut(&mut graph, &path)?;

    if target
        .get("steps")
        .and_then(|s| s.as_object())
        .is_some_and(|s| s.contains_key(&params.step_id))
    {
        return Err(err(format!(
            "Step '{}' already exists in graph",
            params.step_id
        )));
    }

    if target.get("steps").is_none() || !target["steps"].is_object() {
        target["steps"] = serde_json::json!({});
    }
    target["steps"][&params.step_id] = step;

    let steps_count = target["steps"].as_object().map(|s| s.len()).unwrap_or(0);
    let set_entry = steps_count == 1;
    if set_entry {
        target["entryPoint"] = serde_json::Value::String(params.step_id.clone());
    }

    // Connect edges
    if target.get("executionPlan").is_none() || !target["executionPlan"].is_array() {
        target["executionPlan"] = serde_json::json!([]);
    }
    let plan = target["executionPlan"].as_array_mut().unwrap();

    if let Some(ref after) = params.connect_after {
        plan.push(serde_json::json!({
            "fromStep": after,
            "toStep": params.step_id,
        }));
    }
    if let Some(ref error_step) = params.on_error_step {
        plan.push(serde_json::json!({
            "fromStep": params.step_id,
            "toStep": error_step,
            "label": "onError",
        }));
    }

    let (version, new_version) =
        save_graph(server, &params.workflow_id, graph, latest, current).await?;

    // Extract expected inputs from capability schema
    let expected_inputs = cap_result
        .get("input")
        .and_then(|i| i.get("fields"))
        .cloned()
        .unwrap_or(serde_json::json!([]));

    json_result(serde_json::json!({
        "success": true,
        "workflowId": params.workflow_id,
        "version": version,
        "newVersion": new_version,
        "stepId": params.step_id,
        "setAsEntryPoint": set_entry,
        "connectedAfter": params.connect_after,
        "onErrorStep": params.on_error_step,
        "expectedInputs": expected_inputs,
        "hint": "Use set_mapping to map each expected input to a reference or value",
    }))
}

pub async fn remove_variable(
    server: &SmoMcpServer,
    params: RemoveVariableParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    let (mut graph, latest, current) = fetch_latest_graph(server, &params.workflow_id).await?;
    let path = params.path.unwrap_or_default();
    let target = resolve_graph_mut(&mut graph, &path)?;

    let variables = target
        .get_mut("variables")
        .and_then(|v| v.as_object_mut())
        .ok_or_else(|| err(format!("Variable '{}' not found", params.name)))?;

    if variables.remove(&params.name).is_none() {
        return Err(err(format!("Variable '{}' not found", params.name)));
    }

    let (version, new_version) =
        save_graph(server, &params.workflow_id, graph, latest, current).await?;
    json_result(serde_json::json!({
        "success": true,
        "workflowId": params.workflow_id,
        "version": version,
        "newVersion": new_version,
        "removedVariable": params.name,
    }))
}
