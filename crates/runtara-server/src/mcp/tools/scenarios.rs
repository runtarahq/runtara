use rmcp::model::{CallToolResult, Content};
use schemars::JsonSchema;
use serde::Deserialize;

use super::super::server::SmoMcpServer;
use super::internal_api::{api_get, api_post, validate_path_param};

fn json_result(value: serde_json::Value) -> Result<CallToolResult, rmcp::ErrorData> {
    Ok(CallToolResult::success(vec![Content::text(
        serde_json::to_string_pretty(&value).unwrap_or_default(),
    )]))
}

fn err(msg: impl Into<String>) -> rmcp::ErrorData {
    rmcp::ErrorData::internal_error(msg.into(), None)
}

// ===== Parameter Structs =====

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListScenariosParams {
    #[schemars(description = "Page number (1-based)")]
    pub page: Option<i64>,
    #[schemars(description = "Items per page")]
    pub page_size: Option<i64>,
    #[schemars(description = "Filter by folder path (e.g., '/Sales/')")]
    pub path: Option<String>,
    #[schemars(description = "Include subfolders when filtering by path")]
    pub recursive: Option<bool>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetScenarioParams {
    #[schemars(description = "Scenario ID")]
    pub scenario_id: String,
    #[schemars(description = "Specific version number (omit for latest)")]
    pub version: Option<i32>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CreateScenarioParams {
    #[schemars(description = "Scenario name")]
    pub name: String,
    #[schemars(description = "Scenario description")]
    pub description: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct UpdateScenarioParams {
    #[schemars(description = "Scenario ID")]
    pub scenario_id: String,
    #[schemars(description = "Complete execution graph JSON definition")]
    pub execution_graph: serde_json::Value,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CompileScenarioParams {
    #[schemars(description = "Scenario ID")]
    pub scenario_id: String,
    #[schemars(description = "Version number to compile")]
    pub version: i32,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ExecuteScenarioParams {
    #[schemars(description = "Scenario ID")]
    pub scenario_id: String,
    #[schemars(
        description = "Input data as JSON: {\"data\": {...}, \"variables\": {...}}. Omit for scenarios with no inputs — defaults to empty data/variables."
    )]
    pub inputs: Option<serde_json::Value>,
    #[schemars(description = "Specific version to execute (default: current)")]
    pub version: Option<i32>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ExecuteScenarioSyncParams {
    #[schemars(description = "Scenario ID")]
    pub scenario_id: String,
    #[schemars(description = "Request body forwarded to scenario as inputs")]
    pub body: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SetCurrentVersionParams {
    #[schemars(description = "Scenario ID")]
    pub scenario_id: String,
    #[schemars(description = "Version number to set as current")]
    pub version: i32,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct DeployScenarioParams {
    #[schemars(description = "Scenario ID")]
    pub scenario_id: String,
    #[schemars(description = "Complete execution graph JSON definition")]
    pub execution_graph: serde_json::Value,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct DiffScenarioVersionsParams {
    #[schemars(description = "Scenario ID")]
    pub scenario_id: String,
    #[schemars(description = "First version number to compare")]
    pub version_a: i32,
    #[schemars(description = "Second version number to compare")]
    pub version_b: i32,
}

// ===== Tool Implementations =====

pub async fn list_scenarios(
    server: &SmoMcpServer,
    params: ListScenariosParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    let mut query = Vec::new();
    if let Some(p) = params.page {
        query.push(format!("page={}", p));
    }
    if let Some(ps) = params.page_size {
        query.push(format!("pageSize={}", ps));
    }
    if let Some(path) = &params.path {
        query.push(format!("path={}", path));
    }
    if let Some(recursive) = params.recursive {
        query.push(format!("recursive={}", recursive));
    }
    let qs = if query.is_empty() {
        String::new()
    } else {
        format!("?{}", query.join("&"))
    };
    let result = api_get(server, &format!("/api/runtime/scenarios{}", qs)).await?;
    json_result(result)
}

pub async fn get_scenario(
    server: &SmoMcpServer,
    params: GetScenarioParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    validate_path_param("scenario_id", &params.scenario_id)?;
    let qs = match params.version {
        Some(v) => format!("?versionNumber={}", v),
        None => String::new(),
    };
    let result = api_get(
        server,
        &format!("/api/runtime/scenarios/{}{}", params.scenario_id, qs),
    )
    .await?;
    json_result(result)
}

pub async fn create_scenario(
    server: &SmoMcpServer,
    params: CreateScenarioParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    let body = serde_json::json!({
        "name": params.name,
        "description": params.description,
    });
    let result = api_post(server, "/api/runtime/scenarios/create", Some(body)).await?;
    json_result(result)
}

pub async fn update_scenario(
    server: &SmoMcpServer,
    params: UpdateScenarioParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    validate_path_param("scenario_id", &params.scenario_id)?;
    let body = serde_json::json!({
        "executionGraph": params.execution_graph,
    });
    let result = api_post(
        server,
        &format!("/api/runtime/scenarios/{}/update", params.scenario_id),
        Some(body),
    )
    .await?;
    json_result(result)
}

pub async fn compile_scenario(
    server: &SmoMcpServer,
    params: CompileScenarioParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    validate_path_param("scenario_id", &params.scenario_id)?;
    let result = api_post(
        server,
        &format!(
            "/api/runtime/scenarios/{}/versions/{}/compile",
            params.scenario_id, params.version
        ),
        None,
    )
    .await?;
    json_result(result)
}

pub async fn execute_scenario(
    server: &SmoMcpServer,
    params: ExecuteScenarioParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    validate_path_param("scenario_id", &params.scenario_id)?;
    let qs = match params.version {
        Some(v) => format!("?version={}", v),
        None => String::new(),
    };
    let body = serde_json::json!({
        "inputs": params.inputs.unwrap_or(serde_json::json!({"data": {}, "variables": {}})),
    });
    let result = api_post(
        server,
        &format!(
            "/api/runtime/scenarios/{}/execute{}",
            params.scenario_id, qs
        ),
        Some(body),
    )
    .await?;
    json_result(result)
}

pub async fn execute_scenario_sync(
    server: &SmoMcpServer,
    params: ExecuteScenarioSyncParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    validate_path_param("scenario_id", &params.scenario_id)?;
    let result = api_post(
        server,
        &format!("/api/runtime/events/http-sync/{}", params.scenario_id),
        params.body,
    )
    .await?;
    json_result(result)
}

pub async fn set_current_version(
    server: &SmoMcpServer,
    params: SetCurrentVersionParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    validate_path_param("scenario_id", &params.scenario_id)?;
    let result = api_post(
        server,
        &format!(
            "/api/runtime/scenarios/{}/versions/{}/set-current",
            params.scenario_id, params.version
        ),
        None,
    )
    .await?;
    json_result(result)
}

/// Extract StartScenario step references from an execution graph JSON,
/// including those nested inside subgraphs (Split, While, etc.).
fn extract_child_refs(graph: &serde_json::Value) -> Vec<ChildScenarioRef> {
    let mut refs = Vec::new();
    extract_child_refs_recursive(graph, &mut refs);
    refs
}

fn extract_child_refs_recursive(graph: &serde_json::Value, refs: &mut Vec<ChildScenarioRef>) {
    let Some(steps) = graph.get("steps").and_then(|v| v.as_object()) else {
        return;
    };
    for (step_id, step_def) in steps {
        if step_def.get("stepType").and_then(|v| v.as_str()) == Some("StartScenario") {
            if let Some(child_id) = step_def.get("childScenarioId").and_then(|v| v.as_str()) {
                let version = step_def
                    .get("childVersion")
                    .map(|v| match v {
                        serde_json::Value::String(s) => s.clone(),
                        serde_json::Value::Number(n) => n.to_string(),
                        _ => "latest".to_string(),
                    })
                    .unwrap_or_else(|| "latest".to_string());
                refs.push(ChildScenarioRef {
                    step_id: step_id.clone(),
                    child_scenario_id: child_id.to_string(),
                    child_version: version,
                });
            }
        }
        // Recurse into subgraphs (Split, While, etc.)
        if let Some(subgraph) = step_def.get("subgraph") {
            extract_child_refs_recursive(subgraph, refs);
        }
    }
}

struct ChildScenarioRef {
    step_id: String,
    child_scenario_id: String,
    child_version: String,
}

/// Resolve a version string ("latest", "current", or numeric) against a scenario's metadata.
fn resolve_child_version(
    version_str: &str,
    scenario_data: &serde_json::Value,
) -> Option<i32> {
    match version_str {
        "latest" => scenario_data
            .pointer("/data/latestVersion")
            .or_else(|| scenario_data.pointer("/data/latest_version"))
            .and_then(|v| v.as_i64())
            .map(|v| v as i32),
        "current" => scenario_data
            .pointer("/data/currentVersion")
            .or_else(|| scenario_data.pointer("/data/current_version"))
            .and_then(|v| v.as_i64())
            .map(|v| v as i32),
        _ => version_str.parse::<i32>().ok(),
    }
}

pub async fn deploy_scenario(
    server: &SmoMcpServer,
    params: DeployScenarioParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    validate_path_param("scenario_id", &params.scenario_id)?;

    // Step 0: Detect and validate child scenario references (StartScenario steps)
    let child_refs = extract_child_refs(&params.execution_graph);
    let mut child_compilations = Vec::new();

    if !child_refs.is_empty() {
        // Deduplicate by child_scenario_id (multiple steps may reference the same child)
        let mut seen_children = std::collections::HashSet::new();

        for child_ref in &child_refs {
            if !seen_children.insert(child_ref.child_scenario_id.clone()) {
                continue; // Already handled this child
            }

            // Validate child scenario exists
            let child_result = api_get(
                server,
                &format!(
                    "/api/runtime/scenarios/{}",
                    child_ref.child_scenario_id
                ),
            )
            .await
            .map_err(|_| {
                err(format!(
                    "Child scenario '{}' (referenced by StartScenario step '{}') not found. \
                     Deploy the child scenario first, then retry deploying the parent.",
                    child_ref.child_scenario_id, child_ref.step_id
                ))
            })?;

            // Resolve version
            let resolved_version = resolve_child_version(
                &child_ref.child_version,
                &child_result,
            )
            .ok_or_else(|| {
                err(format!(
                    "Cannot resolve version '{}' for child scenario '{}'. \
                     The child scenario may not have a '{}' version set.",
                    child_ref.child_version,
                    child_ref.child_scenario_id,
                    child_ref.child_version
                ))
            })?;

            // Compile child scenario (skips if already compiled)
            let child_compile = api_post(
                server,
                &format!(
                    "/api/runtime/scenarios/{}/versions/{}/compile",
                    child_ref.child_scenario_id, resolved_version
                ),
                None,
            )
            .await
            .map_err(|e| {
                err(format!(
                    "Failed to compile child scenario '{}' version {}: {}. \
                     Fix the child scenario first, then retry deploying the parent.",
                    child_ref.child_scenario_id, resolved_version, e
                ))
            })?;

            child_compilations.push(serde_json::json!({
                "childScenarioId": child_ref.child_scenario_id,
                "version": resolved_version,
                "status": if child_compile.get("imageId").is_some() { "compiled" } else { "unknown" },
            }));
        }
    }

    // Step 1: Update parent scenario
    let update_body = serde_json::json!({
        "executionGraph": params.execution_graph,
    });
    let update_result = api_post(
        server,
        &format!("/api/runtime/scenarios/{}/update", params.scenario_id),
        Some(update_body),
    )
    .await?;

    let version = update_result
        .get("version")
        .and_then(|v| v.as_str())
        .ok_or_else(|| err("Update succeeded but no version returned"))?;

    // Step 2: Compile parent
    let compile_result = api_post(
        server,
        &format!(
            "/api/runtime/scenarios/{}/versions/{}/compile",
            params.scenario_id, version
        ),
        None,
    )
    .await?;

    // Step 3: Set current version
    let _ = api_post(
        server,
        &format!(
            "/api/runtime/scenarios/{}/versions/{}/set-current",
            params.scenario_id, version
        ),
        None,
    )
    .await?;

    let mut response = serde_json::json!({
        "success": true,
        "message": format!("Scenario deployed successfully (version {})", version),
        "scenarioId": params.scenario_id,
        "version": version,
        "compilation": {
            "binarySize": compile_result.get("binarySize"),
            "binaryChecksum": compile_result.get("binaryChecksum"),
        },
        "warnings": update_result.get("warnings"),
    });

    if !child_compilations.is_empty() {
        response["childScenarios"] = serde_json::json!({
            "count": child_compilations.len(),
            "compilations": child_compilations,
        });
    }

    json_result(response)
}

pub async fn diff_scenario_versions(
    server: &SmoMcpServer,
    params: DiffScenarioVersionsParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    validate_path_param("scenario_id", &params.scenario_id)?;
    // Fetch both versions
    let result_a = api_get(
        server,
        &format!(
            "/api/runtime/scenarios/{}?versionNumber={}",
            params.scenario_id, params.version_a
        ),
    )
    .await?;
    let result_b = api_get(
        server,
        &format!(
            "/api/runtime/scenarios/{}?versionNumber={}",
            params.scenario_id, params.version_b
        ),
    )
    .await?;

    let graph_a = result_a
        .pointer("/data/definition/executionGraph")
        .or_else(|| result_a.pointer("/data/executionGraph"))
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    let graph_b = result_b
        .pointer("/data/definition/executionGraph")
        .or_else(|| result_b.pointer("/data/executionGraph"))
        .cloned()
        .unwrap_or(serde_json::Value::Null);

    // Extract steps from both graphs
    let steps_a = extract_steps(&graph_a);
    let steps_b = extract_steps(&graph_b);

    let mut added = Vec::new();
    let mut removed = Vec::new();
    let mut changed = Vec::new();

    // Find added and changed steps
    for (id, step_b) in &steps_b {
        match steps_a.get(id.as_str()) {
            None => added.push(serde_json::json!({
                "stepId": id,
                "stepName": step_b.get("name").or_else(|| step_b.get("stepName")),
                "stepType": step_b.get("type").or_else(|| step_b.get("stepType")),
            })),
            Some(step_a) => {
                if step_a != step_b {
                    let diffs = diff_step(step_a, step_b);
                    if !diffs.is_empty() {
                        changed.push(serde_json::json!({
                            "stepId": id,
                            "stepName": step_b.get("name").or_else(|| step_b.get("stepName")),
                            "changedFields": diffs,
                        }));
                    }
                }
            }
        }
    }

    // Find removed steps
    for (id, step_a) in &steps_a {
        if !steps_b.contains_key(id.as_str()) {
            removed.push(serde_json::json!({
                "stepId": id,
                "stepName": step_a.get("name").or_else(|| step_a.get("stepName")),
                "stepType": step_a.get("type").or_else(|| step_a.get("stepType")),
            }));
        }
    }

    // Check for top-level graph changes (inputSchema, outputSchema, name, etc.)
    let mut graph_changes = Vec::new();
    for key in ["name", "description", "inputSchema", "outputSchema"] {
        let val_a = graph_a.get(key);
        let val_b = graph_b.get(key);
        if val_a != val_b {
            graph_changes.push(key);
        }
    }

    let response = serde_json::json!({
        "success": true,
        "scenarioId": params.scenario_id,
        "versionA": params.version_a,
        "versionB": params.version_b,
        "summary": format!(
            "{} added, {} removed, {} changed",
            added.len(), removed.len(), changed.len()
        ),
        "graphChanges": graph_changes,
        "addedSteps": added,
        "removedSteps": removed,
        "changedSteps": changed,
    });
    json_result(response)
}

/// Extract steps from an execution graph as a map of stepId -> step JSON.
fn extract_steps(
    graph: &serde_json::Value,
) -> std::collections::HashMap<String, &serde_json::Value> {
    let mut map = std::collections::HashMap::new();
    if let Some(steps) = graph.get("steps").and_then(|s| s.as_object()) {
        for (id, step) in steps {
            map.insert(id.clone(), step);
        }
    }
    map
}

/// Compare two step JSON objects and return a list of changed top-level field names.
fn diff_step(a: &serde_json::Value, b: &serde_json::Value) -> Vec<String> {
    let mut changed = Vec::new();
    let empty = serde_json::Map::new();
    let obj_a = a.as_object().unwrap_or(&empty);
    let obj_b = b.as_object().unwrap_or(&empty);

    let all_keys: std::collections::HashSet<&String> = obj_a.keys().chain(obj_b.keys()).collect();

    for key in all_keys {
        if obj_a.get(key) != obj_b.get(key) {
            changed.push(key.clone());
        }
    }
    changed
}
