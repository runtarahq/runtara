use rmcp::model::{CallToolResult, Content};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

use super::super::server::SmoMcpServer;
use super::internal_api::{api_get, api_post, validate_path_param};

fn json_result(value: serde_json::Value) -> Result<CallToolResult, rmcp::ErrorData> {
    Ok(CallToolResult::success(vec![Content::text(
        serde_json::to_string_pretty(&value).unwrap_or_default(),
    )]))
}

// ===== Parameter Structs =====

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListExecutionsParams {
    #[schemars(description = "Filter by workflow ID")]
    pub workflow_id: Option<String>,
    #[schemars(
        description = "Comma-separated statuses: queued,compiling,running,completed,failed,timeout,cancelled"
    )]
    pub status: Option<String>,
    #[schemars(description = "Page number (0-based)")]
    pub page: Option<i64>,
    #[schemars(description = "Page size")]
    pub size: Option<i64>,
    #[schemars(description = "Sort field (e.g., 'completedAt', 'createdAt')")]
    pub sort_by: Option<String>,
    #[schemars(description = "Sort order: 'asc' or 'desc'")]
    pub sort_order: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetExecutionParams {
    #[schemars(description = "Execution instance UUID")]
    pub instance_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetStepEventsParams {
    #[schemars(description = "Workflow ID")]
    pub workflow_id: String,
    #[schemars(description = "Execution instance UUID")]
    pub instance_id: String,
    #[schemars(
        description = "Filter by event subtype (e.g., 'step_debug_start', 'step_debug_end', 'workflow_log')"
    )]
    pub subtype: Option<String>,
    #[schemars(description = "Max results (default 100)")]
    pub limit: Option<i64>,
    #[schemars(description = "Only return root-level events")]
    pub root_scopes_only: Option<bool>,
    #[schemars(description = "Sort order: 'asc' or 'desc'")]
    pub sort_order: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetStepSummariesParams {
    #[schemars(description = "Workflow ID")]
    pub workflow_id: String,
    #[schemars(description = "Execution instance UUID")]
    pub instance_id: String,
    #[schemars(description = "Filter by status (running, completed, failed)")]
    pub status: Option<String>,
    #[schemars(description = "Max results (default 100)")]
    pub limit: Option<i64>,
    #[schemars(description = "Only return root-level steps")]
    pub root_scopes_only: Option<bool>,
    #[schemars(
        description = "If false, include full inputs/outputs per step (default: true = compact, omits inputs/outputs)"
    )]
    pub compact: Option<bool>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct StopExecutionParams {
    #[schemars(description = "Execution instance UUID")]
    pub instance_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ExecuteWorkflowWaitParams {
    #[schemars(description = "Workflow ID")]
    pub workflow_id: String,
    #[schemars(
        description = "Input data as JSON (format: {\"data\": {...}, \"variables\": {...}})"
    )]
    pub inputs: Option<serde_json::Value>,
    #[schemars(description = "Specific version to execute (default: current)")]
    pub version: Option<i32>,
    #[schemars(description = "Max seconds to wait for completion (default: 120, max: 300)")]
    pub timeout_seconds: Option<u32>,
}

// ===== Tool Implementations =====

pub async fn list_executions(
    server: &SmoMcpServer,
    params: ListExecutionsParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    let mut query = Vec::new();
    if let Some(sid) = &params.workflow_id {
        query.push(format!("workflow_id={}", sid));
    }
    if let Some(status) = &params.status {
        query.push(format!("status={}", status));
    }
    if let Some(p) = params.page {
        query.push(format!("page={}", p));
    }
    if let Some(s) = params.size {
        query.push(format!("size={}", s));
    }
    if let Some(sb) = &params.sort_by {
        query.push(format!("sort_by={}", sb));
    }
    if let Some(so) = &params.sort_order {
        query.push(format!("sort_order={}", so));
    }
    let qs = if query.is_empty() {
        String::new()
    } else {
        format!("?{}", query.join("&"))
    };
    let mut result = api_get(server, &format!("/api/runtime/executions{}", qs)).await?;

    // Strip verbose fields from execution listings to keep responses compact.
    // Use get_execution for full details on a specific instance.
    if let Some(content) = result
        .pointer_mut("/data/content")
        .and_then(|v| v.as_array_mut())
    {
        for item in content {
            if let Some(obj) = item.as_object_mut() {
                obj.remove("inputs");
                obj.remove("outputs");
                obj.remove("steps");
            }
        }
    }

    json_result(result)
}

pub async fn get_execution(
    server: &SmoMcpServer,
    params: GetExecutionParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    validate_path_param("instance_id", &params.instance_id)?;
    let mut result = api_get(
        server,
        &format!("/api/runtime/workflows/instances/{}", params.instance_id),
    )
    .await?;

    // Strip steps array — use get_step_summaries for step-level detail.
    if let Some(data) = result.pointer_mut("/data").and_then(|v| v.as_object_mut()) {
        data.remove("steps");
        // Truncate large inputs/outputs to keep response manageable
        for key in &["inputs", "outputs"] {
            if let Some(val) = data.get(key.to_owned()) {
                let s = serde_json::to_string(val).unwrap_or_default();
                if s.len() > 4000 {
                    data.insert(
                        key.to_string(),
                        serde_json::json!({
                            "_truncated": true,
                            "_originalSize": s.len(),
                            "_preview": &s[..2000]
                        }),
                    );
                }
            }
        }
    }

    json_result(result)
}

pub async fn get_step_events(
    server: &SmoMcpServer,
    params: GetStepEventsParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    validate_path_param("workflow_id", &params.workflow_id)?;
    validate_path_param("instance_id", &params.instance_id)?;
    let mut query = Vec::new();
    if let Some(subtype) = &params.subtype {
        query.push(format!("subtype={}", subtype));
    }
    if let Some(limit) = params.limit {
        query.push(format!("limit={}", limit));
    }
    if let Some(rso) = params.root_scopes_only {
        query.push(format!("rootScopesOnly={}", rso));
    }
    if let Some(so) = &params.sort_order {
        query.push(format!("sortOrder={}", so));
    }
    let qs = if query.is_empty() {
        String::new()
    } else {
        format!("?{}", query.join("&"))
    };
    let result = api_get(
        server,
        &format!(
            "/api/runtime/workflows/{}/instances/{}/step-events{}",
            params.workflow_id, params.instance_id, qs
        ),
    )
    .await?;
    json_result(result)
}

pub async fn get_step_summaries(
    server: &SmoMcpServer,
    params: GetStepSummariesParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    validate_path_param("workflow_id", &params.workflow_id)?;
    validate_path_param("instance_id", &params.instance_id)?;
    let mut query = Vec::new();
    if let Some(status) = &params.status {
        query.push(format!("status={}", status));
    }
    if let Some(limit) = params.limit {
        query.push(format!("limit={}", limit));
    }
    if let Some(rso) = params.root_scopes_only {
        query.push(format!("rootScopesOnly={}", rso));
    }
    let qs = if query.is_empty() {
        String::new()
    } else {
        format!("?{}", query.join("&"))
    };
    let mut result = api_get(
        server,
        &format!(
            "/api/runtime/workflows/{}/instances/{}/steps{}",
            params.workflow_id, params.instance_id, qs
        ),
    )
    .await?;

    // Compact mode (default): strip inputs and outputs from each step.
    // Pass compact=false to include full data.
    if params.compact != Some(false)
        && let Some(steps) = result
            .pointer_mut("/data/steps")
            .and_then(|s| s.as_array_mut())
    {
        for step in steps {
            if let Some(obj) = step.as_object_mut() {
                obj.remove("inputs");
                obj.remove("outputs");
            }
        }
    }

    json_result(result)
}

pub async fn stop_execution(
    server: &SmoMcpServer,
    params: StopExecutionParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    validate_path_param("instance_id", &params.instance_id)?;
    let result = api_post(
        server,
        &format!(
            "/api/runtime/workflows/instances/{}/stop",
            params.instance_id
        ),
        None,
    )
    .await?;
    json_result(result)
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct PauseExecutionParams {
    #[schemars(description = "Execution instance UUID")]
    pub instance_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ResumeExecutionParams {
    #[schemars(description = "Execution instance UUID")]
    pub instance_id: String,
}

pub async fn pause_execution(
    server: &SmoMcpServer,
    params: PauseExecutionParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    validate_path_param("instance_id", &params.instance_id)?;
    let result = api_post(
        server,
        &format!(
            "/api/runtime/workflows/instances/{}/pause",
            params.instance_id
        ),
        None,
    )
    .await?;
    json_result(result)
}

pub async fn resume_execution(
    server: &SmoMcpServer,
    params: ResumeExecutionParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    validate_path_param("instance_id", &params.instance_id)?;
    let result = api_post(
        server,
        &format!(
            "/api/runtime/workflows/instances/{}/resume",
            params.instance_id
        ),
        None,
    )
    .await?;
    json_result(result)
}

// ===== Debugging Tool Parameter Structs =====

#[derive(Debug, Deserialize, JsonSchema)]
pub struct InspectStepParams {
    #[schemars(description = "Workflow ID")]
    pub workflow_id: String,
    #[schemars(description = "Execution instance UUID")]
    pub instance_id: String,
    #[schemars(description = "Step ID to inspect")]
    pub step_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct TraceReferenceParams {
    #[schemars(description = "Workflow ID")]
    pub workflow_id: String,
    #[schemars(description = "Execution instance UUID")]
    pub instance_id: String,
    #[schemars(
        description = "Reference path to resolve (e.g., 'steps.getVariant.outputs.price', 'data.orderId', 'variables.counter')"
    )]
    pub reference: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct WhyExecutionFailedParams {
    #[schemars(description = "Workflow ID")]
    pub workflow_id: String,
    #[schemars(description = "Execution instance UUID")]
    pub instance_id: String,
}

pub async fn execute_workflow_wait(
    server: &SmoMcpServer,
    params: ExecuteWorkflowWaitParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    validate_path_param("workflow_id", &params.workflow_id)?;
    let timeout = params.timeout_seconds.unwrap_or(120).min(300);

    // Step 1: Queue execution
    let qs = match params.version {
        Some(v) => format!("?version={}", v),
        None => String::new(),
    };
    let body = serde_json::json!({
        "inputs": params.inputs.unwrap_or(serde_json::json!({"data": {}, "variables": {}})),
    });
    let exec_result = api_post(
        server,
        &format!(
            "/api/runtime/workflows/{}/execute{}",
            params.workflow_id, qs
        ),
        Some(body),
    )
    .await?;

    let instance_id = exec_result
        .pointer("/data/instanceId")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            rmcp::ErrorData::internal_error(
                "Execute succeeded but no instanceId returned".to_string(),
                None,
            )
        })?
        .to_string();

    // Step 2: Poll until terminal state or timeout
    let start = std::time::Instant::now();
    let poll_interval = std::time::Duration::from_secs(2);
    let timeout_duration = std::time::Duration::from_secs(timeout as u64);

    loop {
        tokio::time::sleep(poll_interval).await;

        let result = api_get(
            server,
            &format!("/api/runtime/workflows/instances/{}", instance_id),
        )
        .await?;

        let status = result
            .pointer("/data/status")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        match status {
            "completed" | "failed" | "timeout" | "cancelled" => {
                return json_result(result);
            }
            _ => {
                if start.elapsed() >= timeout_duration {
                    return json_result(serde_json::json!({
                        "success": false,
                        "message": format!(
                            "Timed out after {}s waiting for execution to complete",
                            timeout
                        ),
                        "instanceId": instance_id,
                        "lastStatus": status,
                        "data": result.get("data"),
                    }));
                }
            }
        }
    }
}

// ===== Debugging Tools =====

/// Helper: fetch all step summaries with full inputs/outputs for an instance.
async fn fetch_full_step_summaries(
    server: &SmoMcpServer,
    workflow_id: &str,
    instance_id: &str,
) -> Result<serde_json::Value, rmcp::ErrorData> {
    api_get(
        server,
        &format!(
            "/api/runtime/workflows/{}/instances/{}/steps?compact=false&limit=500",
            workflow_id, instance_id
        ),
    )
    .await
}

/// Helper: resolve a JSON path like "field.nested.0.name" against a Value.
fn resolve_json_path(value: &serde_json::Value, path: &str) -> Option<serde_json::Value> {
    let mut current = value;
    for segment in path.split('.') {
        if let Ok(idx) = segment.parse::<usize>() {
            current = current.get(idx)?;
        } else {
            current = current.get(segment)?;
        }
    }
    Some(current.clone())
}

/// Helper: find a step by ID in the step summaries response.
fn find_step_in_summaries<'a>(
    summaries: &'a serde_json::Value,
    step_id: &str,
) -> Option<&'a serde_json::Value> {
    summaries
        .pointer("/data/steps")
        .and_then(|s| s.as_array())
        .and_then(|steps| {
            steps
                .iter()
                .find(|s| s.get("stepId").and_then(|v| v.as_str()) == Some(step_id))
        })
}

/// Helper: resolve inputMapping references against step summaries.
fn resolve_input_mappings(
    input_mapping: &serde_json::Value,
    summaries: &serde_json::Value,
    execution: &serde_json::Value,
) -> serde_json::Value {
    let Some(mapping_obj) = input_mapping.as_object() else {
        return json!({});
    };

    let mut resolved = serde_json::Map::new();
    for (input_name, mapping_value) in mapping_obj {
        let value_type = mapping_value
            .get("valueType")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let value = mapping_value
            .get("value")
            .cloned()
            .unwrap_or(serde_json::Value::Null);

        let mut entry = json!({
            "mapping": mapping_value,
        });

        match value_type {
            "reference" => {
                if let Some(ref_path) = value.as_str() {
                    let parts: Vec<&str> = ref_path.splitn(4, '.').collect();
                    match parts.first().copied() {
                        Some("steps") if parts.len() >= 3 => {
                            let source_step_id = parts[1];
                            // parts[2] should be "outputs"
                            let field_path = if parts.len() >= 4 {
                                Some(parts[3])
                            } else {
                                None
                            };

                            if let Some(source) = find_step_in_summaries(summaries, source_step_id)
                            {
                                entry["sourceStep"] = json!(source_step_id);
                                entry["sourceStatus"] =
                                    source.get("status").cloned().unwrap_or(json!("unknown"));

                                if let Some(outputs) = source.get("outputs") {
                                    if let Some(fp) = field_path {
                                        entry["resolvedValue"] =
                                            resolve_json_path(outputs, fp).unwrap_or(json!(null));
                                    } else {
                                        entry["resolvedValue"] = outputs.clone();
                                    }
                                }
                            } else {
                                entry["sourceStep"] = json!(source_step_id);
                                entry["sourceStatus"] = json!("not_found");
                            }
                        }
                        Some("data") if parts.len() >= 2 => {
                            let field = parts[1..].join(".");
                            if let Some(inputs) = execution
                                .pointer("/data/inputs/data")
                                .or_else(|| execution.pointer("/data/inputs"))
                            {
                                entry["resolvedValue"] =
                                    resolve_json_path(inputs, &field).unwrap_or(json!(null));
                            }
                            entry["source"] = json!("workflow_input");
                        }
                        Some("variables") if parts.len() >= 2 => {
                            entry["source"] = json!("variable");
                            entry["variableName"] = json!(parts[1]);
                        }
                        _ => {}
                    }
                }
            }
            "immediate" => {
                entry["resolvedValue"] = value;
            }
            "template" => {
                entry["template"] = value;
            }
            _ => {}
        }

        resolved.insert(input_name.clone(), entry);
    }

    serde_json::Value::Object(resolved)
}

pub async fn inspect_step(
    server: &SmoMcpServer,
    params: InspectStepParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    validate_path_param("workflow_id", &params.workflow_id)?;
    validate_path_param("instance_id", &params.instance_id)?;

    // Fetch step summaries (full, not compact) and workflow definition in parallel
    let summaries =
        fetch_full_step_summaries(server, &params.workflow_id, &params.instance_id).await?;

    let target = find_step_in_summaries(&summaries, &params.step_id).ok_or_else(|| {
        rmcp::ErrorData::internal_error(
            format!(
                "Step '{}' not found in execution {}",
                params.step_id, params.instance_id
            ),
            None,
        )
    })?;

    // Fetch workflow definition to get inputMapping
    let workflow = api_get(
        server,
        &format!("/api/runtime/workflows/{}", params.workflow_id),
    )
    .await?;

    // Fetch execution for input data
    let execution = api_get(
        server,
        &format!("/api/runtime/workflows/instances/{}", params.instance_id),
    )
    .await?;

    // Extract step definition from workflow graph
    let input_mapping = workflow
        .pointer("/data/definition/executionGraph/steps")
        .or_else(|| workflow.pointer("/data/executionGraph/steps"))
        .and_then(|steps| steps.get(&params.step_id))
        .and_then(|step| step.get("inputMapping"))
        .cloned()
        .unwrap_or(json!({}));

    let resolved_inputs = resolve_input_mappings(&input_mapping, &summaries, &execution);

    let response = json!({
        "step": {
            "stepId": target.get("stepId"),
            "stepName": target.get("stepName"),
            "stepType": target.get("stepType"),
            "status": target.get("status"),
            "durationMs": target.get("durationMs"),
            "error": target.get("error"),
        },
        "resolvedInputs": resolved_inputs,
        "outputs": target.get("outputs"),
    });

    json_result(response)
}

pub async fn trace_reference(
    server: &SmoMcpServer,
    params: TraceReferenceParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    validate_path_param("workflow_id", &params.workflow_id)?;
    validate_path_param("instance_id", &params.instance_id)?;

    let parts: Vec<&str> = params.reference.splitn(4, '.').collect();
    if parts.is_empty() {
        return Err(rmcp::ErrorData::invalid_params(
            "Reference path must not be empty".to_string(),
            None,
        ));
    }

    match parts[0] {
        "steps" => {
            if parts.len() < 3 {
                return Err(rmcp::ErrorData::invalid_params(
                    "Step reference must be 'steps.<stepId>.outputs[.<field>]'".to_string(),
                    None,
                ));
            }
            let step_id = parts[1];
            let field_path = if parts.len() >= 4 {
                Some(parts[3])
            } else {
                None
            };

            let summaries =
                fetch_full_step_summaries(server, &params.workflow_id, &params.instance_id).await?;

            let step = find_step_in_summaries(&summaries, step_id).ok_or_else(|| {
                rmcp::ErrorData::internal_error(
                    format!(
                        "Source step '{}' not found in execution {}",
                        step_id, params.instance_id
                    ),
                    None,
                )
            })?;

            let outputs = step.get("outputs").cloned().unwrap_or(json!(null));
            let resolved = if let Some(fp) = field_path {
                resolve_json_path(&outputs, fp).unwrap_or(json!(null))
            } else {
                outputs.clone()
            };

            json_result(json!({
                "reference": params.reference,
                "resolved": !resolved.is_null(),
                "value": resolved,
                "source": {
                    "type": "step_output",
                    "stepId": step_id,
                    "stepStatus": step.get("status"),
                    "fullOutputs": outputs,
                }
            }))
        }
        "data" => {
            let field = parts[1..].join(".");
            let execution = api_get(
                server,
                &format!("/api/runtime/workflows/instances/{}", params.instance_id),
            )
            .await?;

            let inputs = execution
                .pointer("/data/inputs/data")
                .or_else(|| execution.pointer("/data/inputs"))
                .cloned()
                .unwrap_or(json!(null));

            let resolved = resolve_json_path(&inputs, &field).unwrap_or(json!(null));

            json_result(json!({
                "reference": params.reference,
                "resolved": !resolved.is_null(),
                "value": resolved,
                "source": {
                    "type": "workflow_input",
                    "fullInputs": inputs,
                }
            }))
        }
        "variables" => {
            let var_name = parts[1..].join(".");
            let workflow = api_get(
                server,
                &format!("/api/runtime/workflows/{}", params.workflow_id),
            )
            .await?;

            let variables = workflow
                .pointer("/data/definition/executionGraph/variables")
                .or_else(|| workflow.pointer("/data/executionGraph/variables"))
                .cloned()
                .unwrap_or(json!({}));

            let resolved = resolve_json_path(&variables, &var_name).unwrap_or(json!(null));

            json_result(json!({
                "reference": params.reference,
                "resolved": !resolved.is_null(),
                "value": resolved,
                "source": {
                    "type": "variable",
                    "allVariables": variables,
                }
            }))
        }
        _ => Err(rmcp::ErrorData::invalid_params(
            format!(
                "Unknown reference root '{}'. Must be 'steps', 'data', or 'variables'.",
                parts[0]
            ),
            None,
        )),
    }
}

pub async fn why_execution_failed(
    server: &SmoMcpServer,
    params: WhyExecutionFailedParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    validate_path_param("workflow_id", &params.workflow_id)?;
    validate_path_param("instance_id", &params.instance_id)?;

    // Fetch execution status
    let execution = api_get(
        server,
        &format!("/api/runtime/workflows/instances/{}", params.instance_id),
    )
    .await?;

    let status = execution
        .pointer("/data/status")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");

    if status != "failed" {
        return json_result(json!({
            "execution": {
                "instanceId": params.instance_id,
                "status": status,
            },
            "message": format!("Execution is not failed (status: {})", status),
        }));
    }

    // Fetch all step summaries (full)
    let summaries =
        fetch_full_step_summaries(server, &params.workflow_id, &params.instance_id).await?;

    let steps = summaries
        .pointer("/data/steps")
        .and_then(|s| s.as_array())
        .cloned()
        .unwrap_or_default();

    let mut completed = 0;
    let mut failed_steps = Vec::new();
    let mut running = 0;
    for step in &steps {
        match step.get("status").and_then(|v| v.as_str()) {
            Some("completed") => completed += 1,
            Some("failed") => {
                failed_steps.push(step.clone());
            }
            Some("running") => running += 1,
            _ => {}
        }
    }
    let not_reached = steps.len() - completed - failed_steps.len() - running;

    // Build failure diagnosis for the first (primary) failing step
    let failing_step = if let Some(first_failed) = failed_steps.first() {
        let step_id = first_failed
            .get("stepId")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        // Try to resolve inputs for the failing step
        let workflow = api_get(
            server,
            &format!("/api/runtime/workflows/{}", params.workflow_id),
        )
        .await
        .ok();

        let input_mapping = workflow
            .as_ref()
            .and_then(|s| s.pointer("/data/definition/executionGraph/steps"))
            .or_else(|| {
                workflow
                    .as_ref()
                    .and_then(|s| s.pointer("/data/executionGraph/steps"))
            })
            .and_then(|steps| steps.get(step_id))
            .and_then(|step| step.get("inputMapping"))
            .cloned()
            .unwrap_or(json!({}));

        let resolved_inputs = resolve_input_mappings(&input_mapping, &summaries, &execution);

        json!({
            "stepId": first_failed.get("stepId"),
            "stepName": first_failed.get("stepName"),
            "stepType": first_failed.get("stepType"),
            "error": first_failed.get("error"),
            "durationMs": first_failed.get("durationMs"),
            "resolvedInputs": resolved_inputs,
        })
    } else {
        json!(null)
    };

    json_result(json!({
        "execution": {
            "instanceId": params.instance_id,
            "status": "failed",
            "error": execution.pointer("/data/error"),
        },
        "failingStep": failing_step,
        "executionSummary": {
            "totalSteps": steps.len(),
            "completed": completed,
            "failed": failed_steps.len(),
            "running": running,
            "notReached": not_reached,
        },
    }))
}
