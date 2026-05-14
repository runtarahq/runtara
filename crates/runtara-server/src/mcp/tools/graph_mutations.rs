use rmcp::model::{CallToolResult, Content};
use schemars::JsonSchema;
use serde::Deserialize;
use std::collections::{HashMap, HashSet, VecDeque};

use super::super::server::SmoMcpServer;
use super::internal_api::{
    api_get, api_post, api_put, json_object_schema, normalize_json_arg, validate_path_param,
};

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
        .pointer("/data/lastVersionNumber")
        .or_else(|| result.pointer("/data/last_version_number"))
        .and_then(|v| v.as_i64())
        .unwrap_or(1);

    let current_version = result
        .pointer("/data/currentVersionNumber")
        .or_else(|| result.pointer("/data/current_version_number"))
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

async fn fetch_latest_graph_locked(
    server: &SmoMcpServer,
    workflow_id: &str,
) -> Result<
    (
        tokio::sync::OwnedMutexGuard<()>,
        serde_json::Value,
        i64,
        i64,
    ),
    rmcp::ErrorData,
> {
    validate_path_param("workflow_id", workflow_id)?;
    let key = format!("{}:{}", server.tenant_id, workflow_id);
    let lock = server
        .workflow_mutation_locks
        .entry(key)
        .or_insert_with(|| std::sync::Arc::new(tokio::sync::Mutex::new(())))
        .clone();
    let guard = lock.lock_owned().await;
    let (graph, latest_version, current_version) = fetch_latest_graph(server, workflow_id).await?;
    Ok((guard, graph, latest_version, current_version))
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
    #[schemars(schema_with = "json_object_schema")]
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
    #[schemars(schema_with = "json_object_schema")]
    #[schemars(description = "New step definition JSON (must include 'stepType')")]
    pub step: serde_json::Value,
    #[schemars(
        description = "Path to nested subgraph — array of step IDs to traverse. Omit for root graph."
    )]
    pub path: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PatchStepOp {
    #[schemars(description = "Operation: 'replace', 'add', or 'remove'")]
    pub op: String,
    #[schemars(
        description = "JSON Pointer path inside the step (RFC 6901). Examples: \
                       '/inputMapping/url/value', '/name', '/retryPolicy'. Use '~1' for '/' \
                       and '~0' for '~' inside a segment. '-' on arrays means append."
    )]
    pub path: String,
    #[schemars(description = "Value for 'replace' and 'add' (ignored for 'remove')")]
    pub value: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PatchStepParams {
    #[schemars(description = "Workflow ID")]
    pub workflow_id: String,
    #[schemars(description = "Step ID to patch")]
    pub step_id: String,
    #[schemars(
        description = "List of JSON-Patch-style operations applied in order to the step object."
    )]
    pub patches: Vec<PatchStepOp>,
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
    #[schemars(
        description = "Edge label. Required as 'true' or 'false' for outgoing Conditional branches; use 'onError' for error handling."
    )]
    pub label: Option<String>,
    #[schemars(
        description = "Condition expression JSON for the edge. Do not set this on edges from a Conditional step; put the predicate in the Conditional step's condition field."
    )]
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
pub struct SummarizeWorkflowParams {
    #[schemars(description = "Workflow ID")]
    pub workflow_id: String,
    #[schemars(
        description = "Path to nested subgraph — array of step IDs to traverse. Omit for root graph."
    )]
    pub path: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GetWorkflowMetadataParams {
    #[schemars(description = "Workflow ID")]
    pub workflow_id: String,
    #[schemars(
        description = "Path to nested subgraph — array of step IDs to traverse. Omit for root graph."
    )]
    pub path: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListStepsParams {
    #[schemars(description = "Workflow ID")]
    pub workflow_id: String,
    #[schemars(description = "Optional stepType filter")]
    pub step_type: Option<String>,
    #[schemars(description = "Optional case-insensitive substring filter for step name or id")]
    pub name_contains: Option<String>,
    #[schemars(description = "Offset for pagination (default 0)")]
    pub offset: Option<usize>,
    #[schemars(description = "Max steps to return (default 100, max 500)")]
    pub limit: Option<usize>,
    #[schemars(
        description = "Path to nested subgraph — array of step IDs to traverse. Omit for root graph."
    )]
    pub path: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GetStepParams {
    #[schemars(description = "Workflow ID")]
    pub workflow_id: String,
    #[schemars(description = "Step ID")]
    pub step_id: String,
    #[schemars(
        description = "If false, return full step definition including large string values. Default: true."
    )]
    pub compact: Option<bool>,
    #[schemars(
        description = "Path to nested subgraph — array of step IDs to traverse. Omit for root graph."
    )]
    pub path: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListEdgesParams {
    #[schemars(description = "Workflow ID")]
    pub workflow_id: String,
    #[schemars(description = "Optional source step filter")]
    pub from_step: Option<String>,
    #[schemars(description = "Optional target step filter")]
    pub to_step: Option<String>,
    #[schemars(description = "Optional edge label filter")]
    pub label: Option<String>,
    #[schemars(
        description = "Path to nested subgraph — array of step IDs to traverse. Omit for root graph."
    )]
    pub path: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GetStepEdgesParams {
    #[schemars(description = "Workflow ID")]
    pub workflow_id: String,
    #[schemars(description = "Step ID")]
    pub step_id: String,
    #[schemars(description = "Direction filter: incoming, outgoing, or both (default both)")]
    pub direction: Option<String>,
    #[schemars(
        description = "Path to nested subgraph — array of step IDs to traverse. Omit for root graph."
    )]
    pub path: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GetStepMappingsParams {
    #[schemars(description = "Workflow ID")]
    pub workflow_id: String,
    #[schemars(description = "Step ID")]
    pub step_id: String,
    #[schemars(
        description = "Include expected Agent capability inputs when available. Default true."
    )]
    pub include_expected_inputs: Option<bool>,
    #[schemars(
        description = "Path to nested subgraph — array of step IDs to traverse. Omit for root graph."
    )]
    pub path: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GetInputSchemaParams {
    #[schemars(description = "Workflow ID")]
    pub workflow_id: String,
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
pub struct SetInputSchemaFieldParams {
    #[schemars(description = "Workflow ID")]
    pub workflow_id: String,
    #[schemars(description = "Input field name")]
    pub field_name: String,
    #[schemars(
        description = "Input field schema definition in DSL format (e.g., {\"type\": \"string\", \"required\": true})"
    )]
    pub field: serde_json::Value,
    #[schemars(
        description = "Path to nested subgraph — array of step IDs to traverse. Omit for root graph."
    )]
    pub path: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RemoveInputSchemaFieldParams {
    #[schemars(description = "Workflow ID")]
    pub workflow_id: String,
    #[schemars(description = "Input field name to remove")]
    pub field_name: String,
    #[schemars(
        description = "Path to nested subgraph — array of step IDs to traverse. Omit for root graph."
    )]
    pub path: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GetOutputSchemaParams {
    #[schemars(description = "Workflow ID")]
    pub workflow_id: String,
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
pub struct GetWorkflowSliceParams {
    #[schemars(description = "Workflow ID")]
    pub workflow_id: String,
    #[schemars(description = "Center step ID")]
    pub step_id: String,
    #[schemars(description = "Number of graph hops around the center step (default 1, max 5)")]
    pub hops: Option<usize>,
    #[schemars(
        description = "If false, return compact step summaries instead of step definitions. Default true."
    )]
    pub include_step_definitions: Option<bool>,
    #[schemars(
        description = "If false, return full string values inside step definitions. Default true."
    )]
    pub compact: Option<bool>,
    #[schemars(
        description = "Path to nested subgraph — array of step IDs to traverse. Omit for root graph."
    )]
    pub path: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FindReferencesParams {
    #[schemars(description = "Workflow ID")]
    pub workflow_id: String,
    #[schemars(
        description = "Reference to find, e.g. data.orderId, variables.mode, steps.fetch.outputs.item"
    )]
    pub reference: String,
    #[schemars(
        description = "Path to nested subgraph — array of step IDs to traverse. Omit for root graph."
    )]
    pub path: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListUnmappedInputsParams {
    #[schemars(description = "Workflow ID")]
    pub workflow_id: String,
    #[schemars(description = "Optional step ID. Omit to check all Agent steps.")]
    pub step_id: Option<String>,
    #[schemars(description = "Include optional unmapped inputs in the report. Default false.")]
    pub include_optional: Option<bool>,
    #[schemars(
        description = "Path to nested subgraph — array of step IDs to traverse. Omit for root graph."
    )]
    pub path: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListVariablesParams {
    #[schemars(description = "Workflow ID")]
    pub workflow_id: String,
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
pub struct BatchGraphMutation {
    #[schemars(
        description = "Operation name. Supported: set_workflow_metadata, add_step, remove_step, update_step, patch_step, connect_steps, disconnect_steps, set_entry_point, set_mapping, remove_mapping, set_input_schema, set_input_schema_field, remove_input_schema_field, set_output_schema, set_variable, remove_variable"
    )]
    pub op: String,
    pub step_id: Option<String>,
    #[schemars(schema_with = "optional_json_object_schema")]
    pub step: Option<serde_json::Value>,
    pub patches: Option<Vec<PatchStepOp>>,
    pub from_step: Option<String>,
    pub to_step: Option<String>,
    pub label: Option<String>,
    pub condition: Option<serde_json::Value>,
    pub priority: Option<i64>,
    pub input_name: Option<String>,
    pub from_output: Option<String>,
    pub from_input: Option<String>,
    pub from_variable: Option<String>,
    pub immediate_value: Option<serde_json::Value>,
    pub fields: Option<serde_json::Value>,
    pub field_name: Option<String>,
    pub field: Option<serde_json::Value>,
    pub name: Option<String>,
    pub description: Option<String>,
    pub variable: Option<serde_json::Value>,
}

fn optional_json_object_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    schemars::json_schema!({
        "type": ["object", "null"],
        "additionalProperties": true
    })
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ApplyGraphMutationsParams {
    #[schemars(description = "Workflow ID")]
    pub workflow_id: String,
    #[schemars(description = "Batch of graph operations to apply and save once")]
    pub operations: Vec<BatchGraphMutation>,
    #[schemars(
        description = "Path to nested subgraph — array of step IDs to traverse. Omit for root graph. Applies to all operations in this batch."
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
        description = "Step ID to connect after (creates a normal edge from that step to this one). Do not use with a Conditional source; add the step first and call connect_steps with label 'true' or 'false'."
    )]
    pub connect_after: Option<String>,
    #[schemars(
        description = "Step ID to route errors to (creates an onError edge from this step)"
    )]
    pub on_error_step: Option<String>,
    #[schemars(
        description = "Connection UUID for agents that need credentials (shopify, openai, sftp, …). \
                       This is the `id` field from list_connections — NOT the connection title or \
                       integrationId. Discover candidates with list_agents (see `integrationIds`) \
                       then list_connections(integration_id=<one of those>)."
    )]
    pub connection_id: Option<String>,
    #[schemars(
        description = "Path to nested subgraph — array of step IDs to traverse. Omit for root graph."
    )]
    pub path: Option<Vec<String>>,
}

fn truncate_large_strings(value: &mut serde_json::Value) {
    const MAX: usize = 512;
    const PREVIEW: usize = 256;

    fn walk(value: &mut serde_json::Value) {
        match value {
            serde_json::Value::String(s) if s.len() > MAX => {
                let mut cut = PREVIEW.min(s.len());
                while cut > 0 && !s.is_char_boundary(cut) {
                    cut -= 1;
                }
                *value = serde_json::json!({
                    "_truncated": true,
                    "_originalSize": s.len(),
                    "_preview": &s[..cut],
                });
            }
            serde_json::Value::Array(items) => {
                for item in items {
                    walk(item);
                }
            }
            serde_json::Value::Object(map) => {
                for child in map.values_mut() {
                    walk(child);
                }
            }
            _ => {}
        }
    }

    walk(value);
}

fn sorted_object_keys(value: Option<&serde_json::Value>) -> Vec<String> {
    let mut keys: Vec<String> = value
        .and_then(|v| v.as_object())
        .map(|o| o.keys().cloned().collect())
        .unwrap_or_default();
    keys.sort();
    keys
}

fn step_name(step: &serde_json::Value) -> Option<&str> {
    step.get("name")
        .or_else(|| step.get("stepName"))
        .and_then(|v| v.as_str())
}

fn step_type(step: &serde_json::Value) -> Option<&str> {
    step.get("stepType").and_then(|v| v.as_str())
}

fn edge_from(edge: &serde_json::Value) -> Option<&str> {
    edge.get("fromStep").and_then(|v| v.as_str())
}

fn edge_to(edge: &serde_json::Value) -> Option<&str> {
    edge.get("toStep").and_then(|v| v.as_str())
}

fn edge_label(edge: &serde_json::Value) -> Option<&str> {
    edge.get("label").and_then(|v| v.as_str())
}

fn graph_steps(target: &serde_json::Value) -> Option<&serde_json::Map<String, serde_json::Value>> {
    target.get("steps").and_then(|s| s.as_object())
}

fn graph_edges(target: &serde_json::Value) -> Vec<serde_json::Value> {
    target
        .get("executionPlan")
        .and_then(|p| p.as_array())
        .cloned()
        .unwrap_or_default()
}

fn step_edge_counts(
    edges: &[serde_json::Value],
    step_id: &str,
) -> (usize, usize, Vec<String>, Vec<String>) {
    let mut incoming = Vec::new();
    let mut outgoing = Vec::new();
    for edge in edges {
        if edge_to(edge) == Some(step_id)
            && let Some(from) = edge_from(edge)
        {
            incoming.push(from.to_string());
        }
        if edge_from(edge) == Some(step_id)
            && let Some(to) = edge_to(edge)
        {
            outgoing.push(to.to_string());
        }
    }
    incoming.sort();
    outgoing.sort();
    (incoming.len(), outgoing.len(), incoming, outgoing)
}

fn step_summary(
    step_id: &str,
    step: &serde_json::Value,
    edges: &[serde_json::Value],
) -> serde_json::Value {
    let (incoming_count, outgoing_count, incoming, outgoing) = step_edge_counts(edges, step_id);
    serde_json::json!({
        "id": step_id,
        "name": step_name(step),
        "stepType": step_type(step),
        "agentId": step.get("agentId"),
        "capabilityId": step.get("capabilityId"),
        "incomingCount": incoming_count,
        "outgoingCount": outgoing_count,
        "connectedFrom": incoming,
        "connectedTo": outgoing,
    })
}

fn edge_matches_filters(
    edge: &serde_json::Value,
    from_step: Option<&str>,
    to_step: Option<&str>,
    label: Option<&str>,
) -> bool {
    if let Some(filter) = from_step
        && edge_from(edge) != Some(filter)
    {
        return false;
    }
    if let Some(filter) = to_step
        && edge_to(edge) != Some(filter)
    {
        return false;
    }
    if let Some(filter) = label
        && edge_label(edge) != Some(filter)
    {
        return false;
    }
    true
}

fn json_pointer_escape(segment: &str) -> String {
    segment.replace('~', "~0").replace('/', "~1")
}

fn collect_reference_locations(
    value: &serde_json::Value,
    reference: &str,
    path: &str,
    locations: &mut Vec<serde_json::Value>,
) {
    match value {
        serde_json::Value::String(s) => {
            if s.contains(reference) {
                locations.push(serde_json::json!({
                    "path": path,
                    "value": s,
                }));
            }
        }
        serde_json::Value::Array(items) => {
            for (idx, item) in items.iter().enumerate() {
                collect_reference_locations(
                    item,
                    reference,
                    &format!("{}/{}", path, idx),
                    locations,
                );
            }
        }
        serde_json::Value::Object(map) => {
            for (key, child) in map {
                collect_reference_locations(
                    child,
                    reference,
                    &format!("{}/{}", path, json_pointer_escape(key)),
                    locations,
                );
            }
        }
        _ => {}
    }
}

async fn expected_inputs_for_step(
    server: &SmoMcpServer,
    step: &serde_json::Value,
) -> Result<Vec<serde_json::Value>, rmcp::ErrorData> {
    if step_type(step) != Some("Agent") {
        return Ok(Vec::new());
    }

    let Some(agent_id) = step.get("agentId").and_then(|v| v.as_str()) else {
        return Ok(Vec::new());
    };
    let Some(capability_id) = step.get("capabilityId").and_then(|v| v.as_str()) else {
        return Ok(Vec::new());
    };

    let cap_result = api_get(
        server,
        &format!(
            "/api/runtime/agents/{}/capabilities/{}",
            agent_id, capability_id
        ),
    )
    .await?;

    Ok(cap_result
        .get("inputs")
        .and_then(|inputs| inputs.as_array())
        .cloned()
        .unwrap_or_default())
}

fn missing_inputs_report(
    step_id: &str,
    step: &serde_json::Value,
    expected_inputs: &[serde_json::Value],
    include_optional: bool,
) -> serde_json::Value {
    let mapping = step.get("inputMapping").and_then(|m| m.as_object());
    let mut missing = Vec::new();
    let mut mapped = Vec::new();

    if let Some(mapping) = mapping {
        mapped = mapping.keys().cloned().collect();
        mapped.sort();
    }

    for field in expected_inputs {
        let Some(name) = field.get("name").and_then(|v| v.as_str()) else {
            continue;
        };
        if name == "_connection" {
            continue;
        }
        let required = field
            .get("required")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        if !required && !include_optional {
            continue;
        }
        let is_mapped = mapping.is_some_and(|m| m.contains_key(name));
        if !is_mapped {
            missing.push(serde_json::json!({
                "name": name,
                "required": required,
                "type": field.get("type"),
                "description": field.get("description"),
            }));
        }
    }

    serde_json::json!({
        "stepId": step_id,
        "stepName": step_name(step),
        "agentId": step.get("agentId"),
        "capabilityId": step.get("capabilityId"),
        "mappedInputs": mapped,
        "missingInputs": missing,
        "missingCount": missing.len(),
    })
}

fn required_string(
    value: Option<&String>,
    field: &str,
    op: &str,
) -> Result<String, rmcp::ErrorData> {
    value
        .cloned()
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| err(format!("Operation '{}' requires '{}'", op, field)))
}

fn required_value(
    value: Option<&serde_json::Value>,
    field: &str,
    op: &str,
) -> Result<serde_json::Value, rmcp::ErrorData> {
    value
        .cloned()
        .ok_or_else(|| err(format!("Operation '{}' requires '{}'", op, field)))
}

/// Some MCP clients deliver object-shaped arguments as JSON-encoded strings.
/// Recover the parsed object before structural checks like `.get("stepType")`.
fn required_object_value(
    value: Option<&serde_json::Value>,
    field: &str,
    op: &str,
) -> Result<serde_json::Value, rmcp::ErrorData> {
    let raw = required_value(value, field, op)?;
    let parsed = normalize_json_arg(raw, field)?;
    if !parsed.is_object() {
        return Err(err(format!(
            "Operation '{}' field '{}' must be a JSON object",
            op, field
        )));
    }
    Ok(parsed)
}

// ===== Tool Implementations =====

pub async fn summarize_workflow(
    server: &SmoMcpServer,
    params: SummarizeWorkflowParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    let (mut graph, latest, current) = fetch_latest_graph(server, &params.workflow_id).await?;
    let path = params.path.unwrap_or_default();
    let target = resolve_graph_mut(&mut graph, &path)?;

    let steps = graph_steps(target);
    let edges = graph_edges(target);
    let mut step_type_counts: HashMap<String, usize> = HashMap::new();
    if let Some(steps) = steps {
        for step in steps.values() {
            let step_type = step_type(step).unwrap_or("unknown").to_string();
            *step_type_counts.entry(step_type).or_default() += 1;
        }
    }

    let mut step_type_counts: Vec<_> = step_type_counts.into_iter().collect();
    step_type_counts.sort_by(|a, b| a.0.cmp(&b.0));
    let step_type_counts: Vec<serde_json::Value> = step_type_counts
        .into_iter()
        .map(|(step_type, count)| serde_json::json!({ "stepType": step_type, "count": count }))
        .collect();

    let step_ids: HashSet<String> = steps
        .map(|steps| steps.keys().cloned().collect())
        .unwrap_or_default();
    let entry_point = target.get("entryPoint").and_then(|v| v.as_str());
    let mut warnings = Vec::new();
    if entry_point.is_none() {
        warnings.push("Missing entryPoint".to_string());
    } else if let Some(entry) = entry_point
        && !step_ids.contains(entry)
    {
        warnings.push(format!("entryPoint '{}' does not exist in steps", entry));
    }

    for edge in &edges {
        if let Some(from) = edge_from(edge)
            && !step_ids.contains(from)
        {
            warnings.push(format!("Edge references missing fromStep '{}'", from));
        }
        if let Some(to) = edge_to(edge)
            && !step_ids.contains(to)
        {
            warnings.push(format!("Edge references missing toStep '{}'", to));
        }
    }

    json_result(serde_json::json!({
        "workflowId": params.workflow_id,
        "version": {
            "latest": latest,
            "current": current,
            "hasDraft": latest != current,
        },
        "path": path,
        "metadata": {
            "name": target.get("name"),
            "description": target.get("description"),
            "entryPoint": entry_point,
        },
        "counts": {
            "steps": step_ids.len(),
            "edges": edges.len(),
            "inputFields": sorted_object_keys(target.get("inputSchema")).len(),
            "outputFields": sorted_object_keys(target.get("outputSchema")).len(),
            "variables": sorted_object_keys(target.get("variables")).len(),
        },
        "stepTypeCounts": step_type_counts,
        "inputFields": sorted_object_keys(target.get("inputSchema")),
        "outputFields": sorted_object_keys(target.get("outputSchema")),
        "variables": sorted_object_keys(target.get("variables")),
        "warnings": warnings,
    }))
}

pub async fn get_workflow_metadata(
    server: &SmoMcpServer,
    params: GetWorkflowMetadataParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    let (mut graph, latest, current) = fetch_latest_graph(server, &params.workflow_id).await?;
    let path = params.path.unwrap_or_default();
    let target = resolve_graph_mut(&mut graph, &path)?;

    json_result(serde_json::json!({
        "workflowId": params.workflow_id,
        "version": {
            "latest": latest,
            "current": current,
            "hasDraft": latest != current,
        },
        "path": path,
        "name": target.get("name"),
        "description": target.get("description"),
        "entryPoint": target.get("entryPoint"),
    }))
}

pub async fn list_steps(
    server: &SmoMcpServer,
    params: ListStepsParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    let (mut graph, _latest, _current) = fetch_latest_graph(server, &params.workflow_id).await?;
    let path = params.path.unwrap_or_default();
    let target = resolve_graph_mut(&mut graph, &path)?;
    let edges = graph_edges(target);
    let needle = params.name_contains.as_ref().map(|s| s.to_lowercase());

    let mut steps: Vec<serde_json::Value> = graph_steps(target)
        .map(|steps| {
            steps
                .iter()
                .filter(|(step_id, step)| {
                    if let Some(ref step_type_filter) = params.step_type
                        && step_type(step) != Some(step_type_filter.as_str())
                    {
                        return false;
                    }
                    if let Some(ref needle) = needle {
                        let id_match = step_id.to_lowercase().contains(needle);
                        let name_match = step_name(step)
                            .map(|name| name.to_lowercase().contains(needle))
                            .unwrap_or(false);
                        return id_match || name_match;
                    }
                    true
                })
                .map(|(step_id, step)| step_summary(step_id, step, &edges))
                .collect()
        })
        .unwrap_or_default();

    steps.sort_by(|a, b| {
        a.get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .cmp(b.get("id").and_then(|v| v.as_str()).unwrap_or(""))
    });

    let total = steps.len();
    let offset = params.offset.unwrap_or(0);
    let limit = params.limit.unwrap_or(100).min(500);
    let page: Vec<_> = steps.into_iter().skip(offset).take(limit).collect();

    json_result(serde_json::json!({
        "workflowId": params.workflow_id,
        "path": path,
        "steps": page,
        "total": total,
        "offset": offset,
        "limit": limit,
    }))
}

pub async fn get_step(
    server: &SmoMcpServer,
    params: GetStepParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    let (mut graph, _latest, _current) = fetch_latest_graph(server, &params.workflow_id).await?;
    let path = params.path.unwrap_or_default();
    let target = resolve_graph_mut(&mut graph, &path)?;
    let edges = graph_edges(target);

    let mut step = graph_steps(target)
        .and_then(|steps| steps.get(&params.step_id))
        .cloned()
        .ok_or_else(|| err(format!("Step '{}' not found in graph", params.step_id)))?;
    if params.compact != Some(false) {
        truncate_large_strings(&mut step);
    }

    let incoming: Vec<_> = edges
        .iter()
        .filter(|edge| edge_to(edge) == Some(params.step_id.as_str()))
        .cloned()
        .collect();
    let outgoing: Vec<_> = edges
        .iter()
        .filter(|edge| edge_from(edge) == Some(params.step_id.as_str()))
        .cloned()
        .collect();

    json_result(serde_json::json!({
        "workflowId": params.workflow_id,
        "path": path,
        "stepId": params.step_id,
        "step": step,
        "incomingEdges": incoming,
        "outgoingEdges": outgoing,
    }))
}

pub async fn list_edges(
    server: &SmoMcpServer,
    params: ListEdgesParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    let (mut graph, _latest, _current) = fetch_latest_graph(server, &params.workflow_id).await?;
    let path = params.path.unwrap_or_default();
    let target = resolve_graph_mut(&mut graph, &path)?;

    let edges: Vec<_> = graph_edges(target)
        .into_iter()
        .filter(|edge| {
            edge_matches_filters(
                edge,
                params.from_step.as_deref(),
                params.to_step.as_deref(),
                params.label.as_deref(),
            )
        })
        .collect();

    let count = edges.len();
    json_result(serde_json::json!({
        "workflowId": params.workflow_id,
        "path": path,
        "edges": edges,
        "count": count,
    }))
}

pub async fn get_step_edges(
    server: &SmoMcpServer,
    params: GetStepEdgesParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    let (mut graph, _latest, _current) = fetch_latest_graph(server, &params.workflow_id).await?;
    let path = params.path.unwrap_or_default();
    let target = resolve_graph_mut(&mut graph, &path)?;
    let direction = params.direction.as_deref().unwrap_or("both");

    if !graph_steps(target).is_some_and(|steps| steps.contains_key(&params.step_id)) {
        return Err(err(format!("Step '{}' not found in graph", params.step_id)));
    }

    let mut incoming = Vec::new();
    let mut outgoing = Vec::new();
    for edge in graph_edges(target) {
        if edge_to(&edge) == Some(params.step_id.as_str()) {
            incoming.push(edge.clone());
        }
        if edge_from(&edge) == Some(params.step_id.as_str()) {
            outgoing.push(edge);
        }
    }

    match direction {
        "incoming" => outgoing.clear(),
        "outgoing" => incoming.clear(),
        "both" => {}
        other => {
            return Err(err(format!(
                "Invalid direction '{}'. Use incoming, outgoing, or both.",
                other
            )));
        }
    }

    json_result(serde_json::json!({
        "workflowId": params.workflow_id,
        "path": path,
        "stepId": params.step_id,
        "incomingEdges": incoming,
        "outgoingEdges": outgoing,
    }))
}

pub async fn get_step_mappings(
    server: &SmoMcpServer,
    params: GetStepMappingsParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    let (mut graph, _latest, _current) = fetch_latest_graph(server, &params.workflow_id).await?;
    let path = params.path.unwrap_or_default();
    let target = resolve_graph_mut(&mut graph, &path)?;

    let step = graph_steps(target)
        .and_then(|steps| steps.get(&params.step_id))
        .cloned()
        .ok_or_else(|| err(format!("Step '{}' not found in graph", params.step_id)))?;
    let input_mapping = step
        .get("inputMapping")
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}));
    let expected_inputs = if params.include_expected_inputs != Some(false) {
        expected_inputs_for_step(server, &step).await?
    } else {
        Vec::new()
    };

    json_result(serde_json::json!({
        "workflowId": params.workflow_id,
        "path": path,
        "stepId": params.step_id,
        "stepName": step_name(&step),
        "stepType": step_type(&step),
        "agentId": step.get("agentId"),
        "capabilityId": step.get("capabilityId"),
        "inputMapping": input_mapping,
        "expectedInputs": expected_inputs,
    }))
}

pub async fn add_step(
    server: &SmoMcpServer,
    params: AddStepParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    let step = normalize_json_arg(params.step, "step")?;
    if !step.is_object() {
        return Err(err(
            "'step' must be a JSON object containing at least a 'stepType' field",
        ));
    }
    if step.get("stepType").is_none() {
        return Err(err("Step definition must include 'stepType' field"));
    }

    // Typo linting: catch common field name mistakes
    if step.get("inputMappings").is_some() {
        return Err(err(
            "Found 'inputMappings' (plural) — use 'inputMapping' (singular)",
        ));
    }
    if step.get("agent").is_some() && step.get("agentId").is_none() {
        return Err(err("Found 'agent' — use 'agentId' instead"));
    }
    if step.get("capability").is_some() && step.get("capabilityId").is_none() {
        return Err(err("Found 'capability' — use 'capabilityId' instead"));
    }

    let (_guard, mut graph, latest, current) =
        fetch_latest_graph_locked(server, &params.workflow_id).await?;
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
    let mut step = step;
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
    let (_guard, mut graph, latest, current) =
        fetch_latest_graph_locked(server, &params.workflow_id).await?;
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
    let step = normalize_json_arg(params.step, "step")?;
    if !step.is_object() {
        return Err(err(
            "'step' must be a JSON object containing at least a 'stepType' field",
        ));
    }
    if step.get("stepType").is_none() {
        return Err(err("Step definition must include 'stepType' field"));
    }

    let (_guard, mut graph, latest, current) =
        fetch_latest_graph_locked(server, &params.workflow_id).await?;
    let path = params.path.unwrap_or_default();
    let target = resolve_graph_mut(&mut graph, &path)?;

    let step_exists = target
        .get("steps")
        .and_then(|s| s.as_object())
        .is_some_and(|s| s.contains_key(&params.step_id));
    if !step_exists {
        return Err(err(format!("Step '{}' not found in graph", params.step_id)));
    }

    let mut step = step;
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

/// Decode a JSON Pointer segment per RFC 6901 (~1 → '/', ~0 → '~').
fn unescape_pointer_segment(segment: &str) -> String {
    segment.replace("~1", "/").replace("~0", "~")
}

/// Split `/a/b/c` into (`/a/b`, `c`). Returns an error for empty pointers.
fn split_pointer(pointer: &str) -> Result<(String, String), rmcp::ErrorData> {
    if pointer.is_empty() {
        return Err(err(
            "Cannot patch at root of step — use update_step instead",
        ));
    }
    let idx = pointer
        .rfind('/')
        .ok_or_else(|| err(format!("Invalid JSON pointer: '{}'", pointer)))?;
    Ok((pointer[..idx].to_string(), pointer[idx + 1..].to_string()))
}

fn apply_patches(
    step: &mut serde_json::Value,
    patches: &[PatchStepParamsOp],
) -> Result<(), rmcp::ErrorData> {
    for patch in patches {
        match patch.op.as_str() {
            "replace" => {
                let target = step
                    .pointer_mut(&patch.path)
                    .ok_or_else(|| err(format!("Path '{}' not found on step", patch.path)))?;
                let value = patch
                    .value
                    .clone()
                    .ok_or_else(|| err("'replace' op requires 'value'"))?;
                *target = value;
            }
            "add" => {
                let value = patch
                    .value
                    .clone()
                    .ok_or_else(|| err("'add' op requires 'value'"))?;
                let (parent_path, key) = split_pointer(&patch.path)?;
                let parent = step
                    .pointer_mut(&parent_path)
                    .ok_or_else(|| err(format!("Parent path '{}' not found", parent_path)))?;
                match parent {
                    serde_json::Value::Object(o) => {
                        o.insert(unescape_pointer_segment(&key), value);
                    }
                    serde_json::Value::Array(a) => {
                        if key == "-" {
                            a.push(value);
                        } else {
                            let idx: usize = key.parse().map_err(|_| {
                                err(format!("Invalid array index '{}' for 'add'", key))
                            })?;
                            if idx > a.len() {
                                return Err(err(format!(
                                    "Array index {} out of bounds (len={})",
                                    idx,
                                    a.len()
                                )));
                            }
                            a.insert(idx, value);
                        }
                    }
                    _ => {
                        return Err(err(format!(
                            "Parent at '{}' is not an object or array",
                            parent_path
                        )));
                    }
                }
            }
            "remove" => {
                let (parent_path, key) = split_pointer(&patch.path)?;
                let parent = step
                    .pointer_mut(&parent_path)
                    .ok_or_else(|| err(format!("Parent path '{}' not found", parent_path)))?;
                match parent {
                    serde_json::Value::Object(o) => {
                        let k = unescape_pointer_segment(&key);
                        o.remove(&k)
                            .ok_or_else(|| err(format!("Key '{}' not found", k)))?;
                    }
                    serde_json::Value::Array(a) => {
                        let idx: usize = key.parse().map_err(|_| {
                            err(format!("Invalid array index '{}' for 'remove'", key))
                        })?;
                        if idx >= a.len() {
                            return Err(err(format!(
                                "Array index {} out of bounds (len={})",
                                idx,
                                a.len()
                            )));
                        }
                        a.remove(idx);
                    }
                    _ => {
                        return Err(err(format!(
                            "Parent at '{}' is not an object or array",
                            parent_path
                        )));
                    }
                }
            }
            other => {
                return Err(err(format!(
                    "Unsupported op '{}'. Supported: replace, add, remove",
                    other
                )));
            }
        }
    }
    Ok(())
}

/// Alias kept so apply_patches reads naturally regardless of which param struct
/// passes in the ops. Both PatchStepOp and (future) callers share the same shape.
type PatchStepParamsOp = PatchStepOp;

pub async fn patch_step(
    server: &SmoMcpServer,
    params: PatchStepParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    if params.patches.is_empty() {
        return Err(err("patches must not be empty"));
    }

    let (_guard, mut graph, latest, current) =
        fetch_latest_graph_locked(server, &params.workflow_id).await?;
    let path = params.path.unwrap_or_default();
    let target = resolve_graph_mut(&mut graph, &path)?;

    let step = target
        .get_mut("steps")
        .and_then(|s| s.as_object_mut())
        .and_then(|s| s.get_mut(&params.step_id))
        .ok_or_else(|| err(format!("Step '{}' not found in graph", params.step_id)))?;

    apply_patches(step, &params.patches)?;

    let (version, new_version) =
        save_graph(server, &params.workflow_id, graph, latest, current).await?;
    json_result(serde_json::json!({
        "success": true,
        "workflowId": params.workflow_id,
        "version": version,
        "newVersion": new_version,
        "stepId": params.step_id,
        "patchesApplied": params.patches.len(),
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

    let (_guard, mut graph, latest, current) =
        fetch_latest_graph_locked(server, &params.workflow_id).await?;
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

    let from_step_type = steps
        .get(&params.from_step)
        .and_then(|step| step.get("stepType"))
        .and_then(|step_type| step_type.as_str())
        .unwrap_or("")
        .to_string();
    if from_step_type == "Conditional" {
        match params.label.as_deref() {
            Some("true") | Some("false") => {}
            _ => {
                return Err(err(format!(
                    "Conditional step '{}' must connect outgoing branches with label 'true' or 'false'. Put the predicate in the step.condition field; do not route Conditional edges with edge.condition or steps.{}.outputs.result.",
                    params.from_step, params.from_step
                )));
            }
        }
        if params.condition.is_some() {
            return Err(err(format!(
                "Conditional step '{}' already owns the branch predicate in step.condition. Remove edge.condition and use label 'true' or 'false' on the edge.",
                params.from_step
            )));
        }
        if params.priority.is_some() {
            return Err(err(format!(
                "Conditional step '{}' true/false branches are mutually exclusive; remove edge priority.",
                params.from_step
            )));
        }
    }

    if target.get("executionPlan").is_none() || !target["executionPlan"].is_array() {
        target["executionPlan"] = serde_json::json!([]);
    }

    // Check for duplicate edge
    let plan = target["executionPlan"].as_array().unwrap();
    if from_step_type == "Conditional"
        && let Some(existing) = plan.iter().find(|edge| {
            edge.get("fromStep").and_then(|v| v.as_str()) == Some(params.from_step.as_str())
                && edge.get("label").and_then(|v| v.as_str()) == params.label.as_deref()
        })
    {
        let existing_target = existing
            .get("toStep")
            .and_then(|v| v.as_str())
            .unwrap_or("(unknown)");
        return Err(err(format!(
            "Conditional step '{}' already has a '{}' branch to '{}'. Each Conditional branch label can target only one step.",
            params.from_step,
            params.label.as_deref().unwrap_or("(default)"),
            existing_target
        )));
    }
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
    let (_guard, mut graph, latest, current) =
        fetch_latest_graph_locked(server, &params.workflow_id).await?;
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
    let (_guard, mut graph, latest, current) =
        fetch_latest_graph_locked(server, &params.workflow_id).await?;
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

    let (_guard, mut graph, latest, current) =
        fetch_latest_graph_locked(server, &params.workflow_id).await?;
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
    let (_guard, mut graph, latest, current) =
        fetch_latest_graph_locked(server, &params.workflow_id).await?;
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

pub async fn get_input_schema(
    server: &SmoMcpServer,
    params: GetInputSchemaParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    let (mut graph, _latest, _current) = fetch_latest_graph(server, &params.workflow_id).await?;
    let path = params.path.unwrap_or_default();
    let target = resolve_graph_mut(&mut graph, &path)?;

    let input_schema = target
        .get("inputSchema")
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}));
    let count = input_schema
        .as_object()
        .map(|fields| fields.len())
        .unwrap_or(0);

    json_result(serde_json::json!({
        "workflowId": params.workflow_id,
        "path": path,
        "inputSchema": input_schema,
        "count": count,
    }))
}

pub async fn set_input_schema(
    server: &SmoMcpServer,
    params: SetInputSchemaParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    let (_guard, mut graph, latest, current) =
        fetch_latest_graph_locked(server, &params.workflow_id).await?;
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

pub async fn set_input_schema_field(
    server: &SmoMcpServer,
    params: SetInputSchemaFieldParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    if params.field_name.trim().is_empty() {
        return Err(err("Input field name must not be empty"));
    }

    let (_guard, mut graph, latest, current) =
        fetch_latest_graph_locked(server, &params.workflow_id).await?;
    let path = params.path.unwrap_or_default();
    let target = resolve_graph_mut(&mut graph, &path)?;

    if target.get("inputSchema").is_none() || !target["inputSchema"].is_object() {
        target["inputSchema"] = serde_json::json!({});
    }

    target["inputSchema"][&params.field_name] = params.field;

    let (version, new_version) =
        save_graph(server, &params.workflow_id, graph, latest, current).await?;
    json_result(serde_json::json!({
        "success": true,
        "workflowId": params.workflow_id,
        "version": version,
        "newVersion": new_version,
        "field": params.field_name,
    }))
}

pub async fn remove_input_schema_field(
    server: &SmoMcpServer,
    params: RemoveInputSchemaFieldParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    let (_guard, mut graph, latest, current) =
        fetch_latest_graph_locked(server, &params.workflow_id).await?;
    let path = params.path.unwrap_or_default();
    let target = resolve_graph_mut(&mut graph, &path)?;

    let input_schema = target
        .get_mut("inputSchema")
        .and_then(|schema| schema.as_object_mut())
        .ok_or_else(|| err(format!("Input field '{}' not found", params.field_name)))?;

    if input_schema.remove(&params.field_name).is_none() {
        return Err(err(format!(
            "Input field '{}' not found",
            params.field_name
        )));
    }

    let (version, new_version) =
        save_graph(server, &params.workflow_id, graph, latest, current).await?;
    json_result(serde_json::json!({
        "success": true,
        "workflowId": params.workflow_id,
        "version": version,
        "newVersion": new_version,
        "removedField": params.field_name,
    }))
}

pub async fn get_output_schema(
    server: &SmoMcpServer,
    params: GetOutputSchemaParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    let (mut graph, _latest, _current) = fetch_latest_graph(server, &params.workflow_id).await?;
    let path = params.path.unwrap_or_default();
    let target = resolve_graph_mut(&mut graph, &path)?;

    let output_schema = target
        .get("outputSchema")
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}));
    let count = output_schema
        .as_object()
        .map(|fields| fields.len())
        .unwrap_or(0);

    json_result(serde_json::json!({
        "workflowId": params.workflow_id,
        "path": path,
        "outputSchema": output_schema,
        "count": count,
    }))
}

pub async fn set_output_schema(
    server: &SmoMcpServer,
    params: SetOutputSchemaParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    let (_guard, mut graph, latest, current) =
        fetch_latest_graph_locked(server, &params.workflow_id).await?;
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

pub async fn list_variables(
    server: &SmoMcpServer,
    params: ListVariablesParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    let (mut graph, _latest, _current) = fetch_latest_graph(server, &params.workflow_id).await?;
    let path = params.path.unwrap_or_default();
    let target = resolve_graph_mut(&mut graph, &path)?;

    let variables = target
        .get("variables")
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}));
    let count = variables
        .as_object()
        .map(|fields| fields.len())
        .unwrap_or(0);

    json_result(serde_json::json!({
        "workflowId": params.workflow_id,
        "path": path,
        "variables": variables,
        "count": count,
    }))
}

pub async fn get_workflow_slice(
    server: &SmoMcpServer,
    params: GetWorkflowSliceParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    let (mut graph, _latest, _current) = fetch_latest_graph(server, &params.workflow_id).await?;
    let path = params.path.unwrap_or_default();
    let target = resolve_graph_mut(&mut graph, &path)?;
    let steps = graph_steps(target).ok_or_else(|| err("No steps in graph"))?;
    if !steps.contains_key(&params.step_id) {
        return Err(err(format!("Step '{}' not found in graph", params.step_id)));
    }

    let hops = params.hops.unwrap_or(1).min(5);
    let edges = graph_edges(target);
    let mut included: HashSet<String> = HashSet::new();
    let mut queue = VecDeque::new();
    included.insert(params.step_id.clone());
    queue.push_back((params.step_id.clone(), 0usize));

    while let Some((step_id, depth)) = queue.pop_front() {
        if depth >= hops {
            continue;
        }
        for edge in &edges {
            let neighbor = if edge_from(edge) == Some(step_id.as_str()) {
                edge_to(edge)
            } else if edge_to(edge) == Some(step_id.as_str()) {
                edge_from(edge)
            } else {
                None
            };
            if let Some(neighbor) = neighbor
                && steps.contains_key(neighbor)
                && included.insert(neighbor.to_string())
            {
                queue.push_back((neighbor.to_string(), depth + 1));
            }
        }
    }

    let mut included_ids: Vec<String> = included.iter().cloned().collect();
    included_ids.sort();
    let included_edges: Vec<_> = edges
        .iter()
        .filter(|edge| {
            edge_from(edge).is_some_and(|from| included.contains(from))
                && edge_to(edge).is_some_and(|to| included.contains(to))
        })
        .cloned()
        .collect();
    let boundary_edges: Vec<_> = edges
        .iter()
        .filter(|edge| {
            let from_in = edge_from(edge).is_some_and(|from| included.contains(from));
            let to_in = edge_to(edge).is_some_and(|to| included.contains(to));
            from_in ^ to_in
        })
        .cloned()
        .collect();

    let steps_value = if params.include_step_definitions != Some(false) {
        let mut step_defs = serde_json::Map::new();
        for step_id in &included_ids {
            if let Some(step) = steps.get(step_id) {
                let mut step = step.clone();
                if params.compact != Some(false) {
                    truncate_large_strings(&mut step);
                }
                step_defs.insert(step_id.clone(), step);
            }
        }
        serde_json::Value::Object(step_defs)
    } else {
        serde_json::Value::Array(
            included_ids
                .iter()
                .filter_map(|step_id| {
                    steps
                        .get(step_id)
                        .map(|step| step_summary(step_id, step, &edges))
                })
                .collect(),
        )
    };

    json_result(serde_json::json!({
        "workflowId": params.workflow_id,
        "path": path,
        "centerStepId": params.step_id,
        "hops": hops,
        "stepIds": included_ids,
        "steps": steps_value,
        "edges": included_edges,
        "boundaryEdges": boundary_edges,
    }))
}

pub async fn find_references(
    server: &SmoMcpServer,
    params: FindReferencesParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    if params.reference.trim().is_empty() {
        return Err(err("reference must not be empty"));
    }

    let (mut graph, _latest, _current) = fetch_latest_graph(server, &params.workflow_id).await?;
    let path = params.path.unwrap_or_default();
    let target = resolve_graph_mut(&mut graph, &path)?;

    let mut hits = Vec::new();
    if let Some(steps) = graph_steps(target) {
        for (step_id, step) in steps {
            let mut locations = Vec::new();
            collect_reference_locations(step, &params.reference, "", &mut locations);
            if !locations.is_empty() {
                hits.push(serde_json::json!({
                    "scope": "step",
                    "stepId": step_id,
                    "stepName": step_name(step),
                    "stepType": step_type(step),
                    "locations": locations,
                }));
            }
        }
    }

    for (scope, value) in [
        ("inputSchema", target.get("inputSchema")),
        ("outputSchema", target.get("outputSchema")),
        ("variables", target.get("variables")),
        ("executionPlan", target.get("executionPlan")),
    ] {
        if let Some(value) = value {
            let mut locations = Vec::new();
            collect_reference_locations(value, &params.reference, "", &mut locations);
            if !locations.is_empty() {
                hits.push(serde_json::json!({
                    "scope": scope,
                    "locations": locations,
                }));
            }
        }
    }

    let count = hits.len();
    json_result(serde_json::json!({
        "workflowId": params.workflow_id,
        "path": path,
        "reference": params.reference,
        "hits": hits,
        "count": count,
    }))
}

pub async fn list_unmapped_inputs(
    server: &SmoMcpServer,
    params: ListUnmappedInputsParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    let (mut graph, _latest, _current) = fetch_latest_graph(server, &params.workflow_id).await?;
    let path = params.path.unwrap_or_default();
    let target = resolve_graph_mut(&mut graph, &path)?;
    let steps = graph_steps(target).ok_or_else(|| err("No steps in graph"))?;
    let include_optional = params.include_optional.unwrap_or(false);

    let mut reports = Vec::new();
    for (step_id, step) in steps {
        if let Some(filter) = &params.step_id
            && filter != step_id
        {
            continue;
        }
        if step_type(step) != Some("Agent") {
            continue;
        }
        let expected = expected_inputs_for_step(server, step).await?;
        reports.push(missing_inputs_report(
            step_id,
            step,
            &expected,
            include_optional,
        ));
    }

    if let Some(step_id) = &params.step_id
        && reports.is_empty()
        && !steps.contains_key(step_id)
    {
        return Err(err(format!("Step '{}' not found in graph", step_id)));
    }

    let missing_count: usize = reports
        .iter()
        .map(|report| {
            report
                .get("missingCount")
                .and_then(|count| count.as_u64())
                .unwrap_or(0) as usize
        })
        .sum();
    let steps_checked = reports.len();

    json_result(serde_json::json!({
        "workflowId": params.workflow_id,
        "path": path,
        "reports": reports,
        "stepsChecked": steps_checked,
        "missingCount": missing_count,
    }))
}

pub async fn set_variable(
    server: &SmoMcpServer,
    params: SetVariableParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    let (_guard, mut graph, latest, current) =
        fetch_latest_graph_locked(server, &params.workflow_id).await?;
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
            } else if step_type == "Conditional" {
                let ref_path = format!("steps.{}.outputs.result", step_id);
                references.push(serde_json::json!({
                    "reference": &ref_path,
                    "source": "step",
                    "stepId": step_id,
                    "stepType": step_type,
                    "field": "result",
                    "type": "boolean",
                    "mapping": { "valueType": "reference", "value": &ref_path },
                    "note": "Conditional evaluation result. Use for inspection or later mappings only; outgoing Conditional branches must use executionPlan labels 'true' and 'false', not edge conditions."
                }));
            } else if step_type == "Finish" {
                // Finish returns workflow output and has no downstream mapping surface.
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

    let (_guard, mut graph, latest, current) =
        fetch_latest_graph_locked(server, &params.workflow_id).await?;
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

    // If a connection is provided, verify the agent supports connections and
    // the connection's integrationId is one the agent accepts. Catches the
    // common "wrong connection for this agent" mistake at create time instead
    // of letting it slip through to compile/deploy.
    if let Some(ref conn_id) = params.connection_id
        && !conn_id.is_empty()
    {
        validate_path_param("connection_id", conn_id)?;

        let agent_info = api_get(server, &format!("/api/runtime/agents/{}", params.agent_id))
            .await
            .map_err(|_| {
                err(format!(
                    "Agent '{}' not found. Use list_agents to discover valid agent IDs.",
                    params.agent_id
                ))
            })?;

        let supports = agent_info
            .get("supportsConnections")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let agent_int_ids: Vec<String> = agent_info
            .get("integrationIds")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();

        if !supports {
            return Err(err(format!(
                "Agent '{}' does not accept a connection (supportsConnections=false). \
                 Drop connection_id.",
                params.agent_id
            )));
        }

        let conn_resp = api_get(server, &format!("/api/runtime/connections/{}", conn_id))
            .await
            .map_err(|_| {
                err(format!(
                    "Connection '{}' not found. The connection_id must be the UUID `id` field \
                 from list_connections — not a title or integrationId.",
                    conn_id
                ))
            })?;

        let conn_int_id = conn_resp
            .pointer("/connection/integrationId")
            .and_then(|v| v.as_str())
            .map(String::from);

        match &conn_int_id {
            Some(int_id) if agent_int_ids.iter().any(|aid| aid == int_id) => {}
            Some(int_id) => {
                return Err(err(format!(
                    "Connection '{}' has integrationId '{}', but agent '{}' accepts [{}]. \
                     Call list_connections(integration_id=...) with one of these to find a \
                     compatible connection.",
                    conn_id,
                    int_id,
                    params.agent_id,
                    agent_int_ids.join(", ")
                )));
            }
            None => {
                return Err(err(format!(
                    "Connection '{}' has no integrationId set; cannot validate compatibility \
                     with agent '{}'.",
                    conn_id, params.agent_id
                )));
            }
        }
    }

    // Build step definition
    let mut step = serde_json::json!({
        "id": params.step_id,
        "stepType": "Agent",
        "name": params.step_name,
        "agentId": params.agent_id,
        "capabilityId": params.capability_id,
    });
    if let Some(ref conn_id) = params.connection_id
        && !conn_id.is_empty()
    {
        step["connectionId"] = serde_json::Value::String(conn_id.clone());
    }

    // Add the step
    let (_guard, mut graph, latest, current) =
        fetch_latest_graph_locked(server, &params.workflow_id).await?;
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

    if let Some(ref after) = params.connect_after {
        let after_step_type = target
            .get("steps")
            .and_then(|steps| steps.get(after))
            .and_then(|step| step.get("stepType"))
            .and_then(|step_type| step_type.as_str())
            .unwrap_or("");
        if after_step_type == "Conditional" {
            return Err(err(format!(
                "connect_after cannot create a valid edge from Conditional step '{}'. Add the agent step without connect_after, then call connect_steps with label 'true' or 'false'.",
                after
            )));
        }
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
        "connectionId": params.connection_id,
        "expectedInputs": expected_inputs,
        "hint": "Use set_mapping to map each expected input to a reference or value",
    }))
}

pub async fn remove_variable(
    server: &SmoMcpServer,
    params: RemoveVariableParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    let (_guard, mut graph, latest, current) =
        fetch_latest_graph_locked(server, &params.workflow_id).await?;
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

pub async fn apply_graph_mutations(
    server: &SmoMcpServer,
    params: ApplyGraphMutationsParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    if params.operations.is_empty() {
        return Err(err("operations must not be empty"));
    }

    let (_guard, mut graph, latest, current) =
        fetch_latest_graph_locked(server, &params.workflow_id).await?;
    let path = params.path.unwrap_or_default();
    let applied = {
        let target = resolve_graph_mut(&mut graph, &path)?;
        let mut applied = Vec::new();

        for (index, operation) in params.operations.iter().enumerate() {
            match operation.op.as_str() {
                "set_workflow_metadata" => {
                    if operation.name.is_none() && operation.description.is_none() {
                        return Err(err(format!(
                            "Operation '{}' at index {} requires name and/or description",
                            operation.op, index
                        )));
                    }
                    if let Some(name) = &operation.name {
                        target["name"] = serde_json::Value::String(name.clone());
                    }
                    if let Some(description) = &operation.description {
                        target["description"] = serde_json::Value::String(description.clone());
                    }
                }
                "add_step" => {
                    let step_id =
                        required_string(operation.step_id.as_ref(), "step_id", &operation.op)?;
                    let mut step =
                        required_object_value(operation.step.as_ref(), "step", &operation.op)?;
                    if step.get("stepType").is_none() {
                        return Err(err(format!(
                            "Operation '{}' at index {} step must include 'stepType'",
                            operation.op, index
                        )));
                    }
                    if step.get("inputMappings").is_some() {
                        return Err(err(
                            "Found 'inputMappings' (plural) — use 'inputMapping' (singular)",
                        ));
                    }
                    if target
                        .get("steps")
                        .and_then(|steps| steps.as_object())
                        .is_some_and(|steps| steps.contains_key(&step_id))
                    {
                        return Err(err(format!("Step '{}' already exists in graph", step_id)));
                    }
                    if target.get("steps").is_none() || !target["steps"].is_object() {
                        target["steps"] = serde_json::json!({});
                    }
                    step["id"] = serde_json::Value::String(step_id.clone());
                    target["steps"][&step_id] = step;
                    if target["steps"].as_object().map(|s| s.len()).unwrap_or(0) == 1 {
                        target["entryPoint"] = serde_json::Value::String(step_id.clone());
                    }
                }
                "remove_step" => {
                    let step_id =
                        required_string(operation.step_id.as_ref(), "step_id", &operation.op)?;
                    let steps = target
                        .get_mut("steps")
                        .and_then(|steps| steps.as_object_mut())
                        .ok_or_else(|| err(format!("Step '{}' not found in graph", step_id)))?;
                    if steps.remove(&step_id).is_none() {
                        return Err(err(format!("Step '{}' not found in graph", step_id)));
                    }
                    if let Some(plan) = target
                        .get_mut("executionPlan")
                        .and_then(|plan| plan.as_array_mut())
                    {
                        plan.retain(|edge| {
                            edge_from(edge) != Some(step_id.as_str())
                                && edge_to(edge) != Some(step_id.as_str())
                        });
                    }
                }
                "update_step" => {
                    let step_id =
                        required_string(operation.step_id.as_ref(), "step_id", &operation.op)?;
                    let mut step =
                        required_object_value(operation.step.as_ref(), "step", &operation.op)?;
                    if step.get("stepType").is_none() {
                        return Err(err(format!(
                            "Operation '{}' at index {} step must include 'stepType'",
                            operation.op, index
                        )));
                    }
                    let steps = target
                        .get_mut("steps")
                        .and_then(|steps| steps.as_object_mut())
                        .ok_or_else(|| err(format!("Step '{}' not found in graph", step_id)))?;
                    if !steps.contains_key(&step_id) {
                        return Err(err(format!("Step '{}' not found in graph", step_id)));
                    }
                    step["id"] = serde_json::Value::String(step_id.clone());
                    steps.insert(step_id, step);
                }
                "patch_step" => {
                    let step_id =
                        required_string(operation.step_id.as_ref(), "step_id", &operation.op)?;
                    let patches = operation
                        .patches
                        .as_ref()
                        .filter(|patches| !patches.is_empty())
                        .ok_or_else(|| {
                            err("Operation 'patch_step' requires non-empty 'patches'")
                        })?;
                    let step = target
                        .get_mut("steps")
                        .and_then(|steps| steps.as_object_mut())
                        .and_then(|steps| steps.get_mut(&step_id))
                        .ok_or_else(|| err(format!("Step '{}' not found in graph", step_id)))?;
                    apply_patches(step, patches)?;
                }
                "connect_steps" => {
                    let from_step =
                        required_string(operation.from_step.as_ref(), "from_step", &operation.op)?;
                    let to_step =
                        required_string(operation.to_step.as_ref(), "to_step", &operation.op)?;
                    let steps = target
                        .get("steps")
                        .and_then(|steps| steps.as_object())
                        .ok_or_else(|| err("No steps in graph"))?;
                    if !steps.contains_key(&from_step) {
                        return Err(err(format!("Step '{}' not found in graph", from_step)));
                    }
                    if !steps.contains_key(&to_step) {
                        return Err(err(format!("Step '{}' not found in graph", to_step)));
                    }
                    let from_step_type = steps
                        .get(&from_step)
                        .and_then(step_type)
                        .unwrap_or("")
                        .to_string();
                    if from_step_type == "Conditional" {
                        match operation.label.as_deref() {
                            Some("true") | Some("false") => {}
                            _ => {
                                return Err(err(format!(
                                    "Conditional step '{}' must connect outgoing branches with label 'true' or 'false'",
                                    from_step
                                )));
                            }
                        }
                        if operation.condition.is_some() || operation.priority.is_some() {
                            return Err(err(format!(
                                "Conditional step '{}' branches must not set edge condition or priority",
                                from_step
                            )));
                        }
                    }
                    if target.get("executionPlan").is_none() || !target["executionPlan"].is_array()
                    {
                        target["executionPlan"] = serde_json::json!([]);
                    }
                    let plan = target["executionPlan"].as_array().unwrap();
                    if from_step_type == "Conditional"
                        && let Some(existing) = plan.iter().find(|edge| {
                            edge_from(edge) == Some(from_step.as_str())
                                && edge_label(edge) == operation.label.as_deref()
                        })
                    {
                        let existing_target = edge_to(existing).unwrap_or("(unknown)");
                        return Err(err(format!(
                            "Conditional step '{}' already has a '{}' branch to '{}'",
                            from_step,
                            operation.label.as_deref().unwrap_or("(default)"),
                            existing_target
                        )));
                    }
                    if plan.iter().any(|edge| {
                        edge_from(edge) == Some(from_step.as_str())
                            && edge_to(edge) == Some(to_step.as_str())
                            && edge_label(edge) == operation.label.as_deref()
                    }) {
                        return Err(err(format!(
                            "Edge from '{}' to '{}' already exists",
                            from_step, to_step
                        )));
                    }
                    let mut edge = serde_json::json!({
                        "fromStep": from_step,
                        "toStep": to_step,
                    });
                    if let Some(label) = &operation.label {
                        edge["label"] = serde_json::Value::String(label.clone());
                    }
                    if let Some(condition) = &operation.condition {
                        edge["condition"] = condition.clone();
                    }
                    if let Some(priority) = operation.priority {
                        edge["priority"] = serde_json::Value::Number(priority.into());
                    }
                    target["executionPlan"].as_array_mut().unwrap().push(edge);
                }
                "disconnect_steps" => {
                    let from_step =
                        required_string(operation.from_step.as_ref(), "from_step", &operation.op)?;
                    let to_step =
                        required_string(operation.to_step.as_ref(), "to_step", &operation.op)?;
                    let plan = target
                        .get_mut("executionPlan")
                        .and_then(|plan| plan.as_array_mut())
                        .ok_or_else(|| {
                            err(format!(
                                "No edges found from '{}' to '{}'",
                                from_step, to_step
                            ))
                        })?;
                    let before = plan.len();
                    plan.retain(|edge| {
                        if edge_from(edge) != Some(from_step.as_str())
                            || edge_to(edge) != Some(to_step.as_str())
                        {
                            return true;
                        }
                        if let Some(label) = &operation.label {
                            return edge_label(edge) != Some(label.as_str());
                        }
                        false
                    });
                    if before == plan.len() {
                        return Err(err(format!(
                            "No edges found from '{}' to '{}'",
                            from_step, to_step
                        )));
                    }
                }
                "set_entry_point" => {
                    let step_id =
                        required_string(operation.step_id.as_ref(), "step_id", &operation.op)?;
                    if !target
                        .get("steps")
                        .and_then(|steps| steps.as_object())
                        .is_some_and(|steps| steps.contains_key(&step_id))
                    {
                        return Err(err(format!("Step '{}' not found in graph", step_id)));
                    }
                    target["entryPoint"] = serde_json::Value::String(step_id);
                }
                "set_mapping" => {
                    let step_id =
                        required_string(operation.step_id.as_ref(), "step_id", &operation.op)?;
                    let input_name = required_string(
                        operation.input_name.as_ref(),
                        "input_name",
                        &operation.op,
                    )?;
                    let source_count = [
                        operation.from_step.is_some(),
                        operation.from_input.is_some(),
                        operation.from_variable.is_some(),
                        operation.immediate_value.is_some(),
                    ]
                    .into_iter()
                    .filter(|is_set| *is_set)
                    .count();
                    if source_count != 1 {
                        return Err(err(
                            "Operation 'set_mapping' requires exactly one of from_step+from_output, from_input, from_variable, or immediate_value",
                        ));
                    }
                    let mapping_value = if let Some(from_step) = &operation.from_step {
                        let from_output = operation.from_output.as_deref().ok_or_else(|| {
                            err("Operation 'set_mapping' requires from_output when from_step is set")
                        })?;
                        if !target
                            .get("steps")
                            .and_then(|steps| steps.as_object())
                            .is_some_and(|steps| steps.contains_key(from_step))
                        {
                            return Err(err(format!(
                                "Referenced step '{}' not found in graph",
                                from_step
                            )));
                        }
                        serde_json::json!({
                            "valueType": "reference",
                            "value": format!("steps.{}.outputs.{}", from_step, from_output)
                        })
                    } else if let Some(from_input) = &operation.from_input {
                        let root_key = from_input.split('.').next().unwrap_or(from_input);
                        if !target
                            .get("inputSchema")
                            .and_then(|schema| schema.as_object())
                            .is_some_and(|schema| schema.contains_key(root_key))
                        {
                            return Err(err(format!(
                                "Referenced input '{}' not found in inputSchema",
                                root_key
                            )));
                        }
                        serde_json::json!({
                            "valueType": "reference",
                            "value": format!("data.{}", from_input)
                        })
                    } else if let Some(from_variable) = &operation.from_variable {
                        let root_key = from_variable.split('.').next().unwrap_or(from_variable);
                        if !target
                            .get("variables")
                            .and_then(|variables| variables.as_object())
                            .is_some_and(|variables| variables.contains_key(root_key))
                        {
                            return Err(err(format!(
                                "Referenced variable '{}' not found in variables",
                                root_key
                            )));
                        }
                        serde_json::json!({
                            "valueType": "reference",
                            "value": format!("variables.{}", from_variable)
                        })
                    } else if let Some(value) = &operation.immediate_value {
                        serde_json::json!({
                            "valueType": "immediate",
                            "value": value
                        })
                    } else {
                        return Err(err(
                            "Operation 'set_mapping' requires one of from_step+from_output, from_input, from_variable, or immediate_value",
                        ));
                    };
                    let step = target
                        .get_mut("steps")
                        .and_then(|steps| steps.as_object_mut())
                        .and_then(|steps| steps.get_mut(&step_id))
                        .ok_or_else(|| err(format!("Step '{}' not found in graph", step_id)))?;
                    if step.get("inputMapping").is_none() || !step["inputMapping"].is_object() {
                        step["inputMapping"] = serde_json::json!({});
                    }
                    step["inputMapping"][&input_name] = mapping_value;
                }
                "remove_mapping" => {
                    let step_id =
                        required_string(operation.step_id.as_ref(), "step_id", &operation.op)?;
                    let input_name = required_string(
                        operation.input_name.as_ref(),
                        "input_name",
                        &operation.op,
                    )?;
                    let mapping = target
                        .get_mut("steps")
                        .and_then(|steps| steps.as_object_mut())
                        .and_then(|steps| steps.get_mut(&step_id))
                        .and_then(|step| step.get_mut("inputMapping"))
                        .and_then(|mapping| mapping.as_object_mut())
                        .ok_or_else(|| {
                            err(format!(
                                "Input mapping '{}' not found on step '{}'",
                                input_name, step_id
                            ))
                        })?;
                    if mapping.remove(&input_name).is_none() {
                        return Err(err(format!(
                            "Input mapping '{}' not found on step '{}'",
                            input_name, step_id
                        )));
                    }
                }
                "set_input_schema" => {
                    target["inputSchema"] =
                        required_value(operation.fields.as_ref(), "fields", &operation.op)?;
                }
                "set_input_schema_field" => {
                    let field_name = required_string(
                        operation.field_name.as_ref(),
                        "field_name",
                        &operation.op,
                    )?;
                    let field = required_value(operation.field.as_ref(), "field", &operation.op)?;
                    if target.get("inputSchema").is_none() || !target["inputSchema"].is_object() {
                        target["inputSchema"] = serde_json::json!({});
                    }
                    target["inputSchema"][&field_name] = field;
                }
                "remove_input_schema_field" => {
                    let field_name = required_string(
                        operation.field_name.as_ref(),
                        "field_name",
                        &operation.op,
                    )?;
                    let input_schema = target
                        .get_mut("inputSchema")
                        .and_then(|schema| schema.as_object_mut())
                        .ok_or_else(|| err(format!("Input field '{}' not found", field_name)))?;
                    if input_schema.remove(&field_name).is_none() {
                        return Err(err(format!("Input field '{}' not found", field_name)));
                    }
                }
                "set_output_schema" => {
                    target["outputSchema"] =
                        required_value(operation.fields.as_ref(), "fields", &operation.op)?;
                }
                "set_variable" => {
                    let name = required_string(operation.name.as_ref(), "name", &operation.op)?;
                    let variable =
                        required_value(operation.variable.as_ref(), "variable", &operation.op)?;
                    if target.get("variables").is_none() || !target["variables"].is_object() {
                        target["variables"] = serde_json::json!({});
                    }
                    target["variables"][&name] = variable;
                }
                "remove_variable" => {
                    let name = required_string(operation.name.as_ref(), "name", &operation.op)?;
                    let variables = target
                        .get_mut("variables")
                        .and_then(|variables| variables.as_object_mut())
                        .ok_or_else(|| err(format!("Variable '{}' not found", name)))?;
                    if variables.remove(&name).is_none() {
                        return Err(err(format!("Variable '{}' not found", name)));
                    }
                }
                other => {
                    return Err(err(format!(
                        "Unsupported batch operation '{}' at index {}",
                        other, index
                    )));
                }
            }

            applied.push(serde_json::json!({
                "index": index,
                "op": &operation.op,
            }));
        }

        applied
    };

    let (version, new_version) =
        save_graph(server, &params.workflow_id, graph, latest, current).await?;
    let operation_count = applied.len();
    json_result(serde_json::json!({
        "success": true,
        "workflowId": params.workflow_id,
        "version": version,
        "newVersion": new_version,
        "path": path,
        "applied": applied,
        "operationCount": operation_count,
    }))
}

#[cfg(test)]
mod patch_tests {
    use super::*;

    fn op(op: &str, path: &str, value: Option<serde_json::Value>) -> PatchStepOp {
        PatchStepOp {
            op: op.to_string(),
            path: path.to_string(),
            value,
        }
    }

    #[test]
    fn replace_sets_leaf_value() {
        let mut step = serde_json::json!({
            "stepType": "Agent",
            "inputMapping": { "url": { "valueType": "immediate", "value": "old" } }
        });
        apply_patches(
            &mut step,
            &[op(
                "replace",
                "/inputMapping/url/value",
                Some(serde_json::json!("new")),
            )],
        )
        .unwrap();
        assert_eq!(
            step["inputMapping"]["url"]["value"],
            serde_json::json!("new")
        );
    }

    #[test]
    fn replace_missing_path_errors() {
        let mut step = serde_json::json!({ "stepType": "Agent" });
        let e = apply_patches(
            &mut step,
            &[op("replace", "/nope", Some(serde_json::json!(1)))],
        )
        .unwrap_err();
        assert!(e.message.contains("not found"));
    }

    #[test]
    fn add_creates_missing_object_key() {
        let mut step = serde_json::json!({ "stepType": "Agent", "inputMapping": {} });
        apply_patches(
            &mut step,
            &[op(
                "add",
                "/inputMapping/newKey",
                Some(serde_json::json!({"valueType": "immediate", "value": 42})),
            )],
        )
        .unwrap();
        assert_eq!(step["inputMapping"]["newKey"]["value"], 42);
    }

    #[test]
    fn add_appends_to_array_with_dash() {
        let mut step = serde_json::json!({ "stepType": "Agent", "tags": ["a"] });
        apply_patches(
            &mut step,
            &[op("add", "/tags/-", Some(serde_json::json!("b")))],
        )
        .unwrap();
        assert_eq!(step["tags"], serde_json::json!(["a", "b"]));
    }

    #[test]
    fn remove_deletes_object_key() {
        let mut step = serde_json::json!({ "stepType": "Agent", "retryPolicy": {"max": 3} });
        apply_patches(&mut step, &[op("remove", "/retryPolicy", None)]).unwrap();
        assert!(step.get("retryPolicy").is_none());
    }

    #[test]
    fn remove_missing_key_errors() {
        let mut step = serde_json::json!({ "stepType": "Agent" });
        let e = apply_patches(&mut step, &[op("remove", "/nope", None)]).unwrap_err();
        assert!(e.message.contains("not found"));
    }

    #[test]
    fn escaped_segment_resolves() {
        // A key containing '/' is represented as '~1' in the pointer.
        let mut step = serde_json::json!({
            "stepType": "Agent",
            "inputMapping": { "path/with/slashes": { "value": "x" } }
        });
        apply_patches(
            &mut step,
            &[op(
                "replace",
                "/inputMapping/path~1with~1slashes/value",
                Some(serde_json::json!("y")),
            )],
        )
        .unwrap();
        assert_eq!(
            step["inputMapping"]["path/with/slashes"]["value"],
            serde_json::json!("y")
        );
    }

    #[test]
    fn unsupported_op_errors() {
        let mut step = serde_json::json!({ "stepType": "Agent" });
        let e =
            apply_patches(&mut step, &[op("move", "/a", Some(serde_json::json!(1)))]).unwrap_err();
        assert!(e.message.contains("Unsupported op"));
    }

    #[test]
    fn add_step_schema_declares_step_as_object() {
        // Regression: prior schema for `step` was an empty `{description: ...}`,
        // which left some MCP clients unsure how to encode the value and they
        // sent it as a stringified JSON (or dropped it). The advertised schema
        // must now name `step` as an object so clients pass it through intact.
        let schema = serde_json::to_value(schemars::schema_for!(AddStepParams)).unwrap();
        let step_schema = schema
            .pointer("/properties/step")
            .expect("step property exists");
        assert_eq!(
            step_schema.get("type").and_then(|v| v.as_str()),
            Some("object"),
            "step schema missing `type: object`: {}",
            step_schema
        );
    }

    /// Some MCP clients deliver object args as a JSON-encoded string. The handler
    /// normalizes that back into an object before checking `stepType`.
    #[test]
    fn add_step_accepts_stringified_step_payload() {
        let original = serde_json::json!({
            "stepType": "Conditional",
            "name": "branch",
            "condition": {"valueType": "immediate", "value": true}
        });
        let stringified = serde_json::Value::String(original.to_string());
        let normalized = normalize_json_arg(stringified, "step").expect("normalize");
        assert!(normalized.is_object(), "expected object after normalize");
        assert_eq!(
            normalized.get("stepType").and_then(|v| v.as_str()),
            Some("Conditional")
        );
    }
}
