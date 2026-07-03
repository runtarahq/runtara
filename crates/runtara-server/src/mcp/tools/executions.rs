use rmcp::model::{CallToolResult, Content};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

use super::super::server::SmoMcpServer;
use super::internal_api::{api_get, api_post, normalize_json_arg, validate_path_param};

const DEBUG_STRING_TRUNCATE_THRESHOLD_BYTES: usize = 4000;
const DEBUG_STRING_PREVIEW_BYTES: usize = 2000;
const RUNTIME_NESTED_REFERENCE_NOTE: &str =
    "Nested condition references are resolved by workflow runtime before agent dispatch.";

fn json_result(value: serde_json::Value) -> Result<CallToolResult, rmcp::ErrorData> {
    Ok(CallToolResult::success(vec![Content::text(
        serde_json::to_string_pretty(&value).unwrap_or_default(),
    )]))
}

fn push_query_param(query: &mut Vec<String>, key: &str, value: &str) {
    query.push(format!("{}={}", key, urlencoding::encode(value)));
}

// ===== Parameter Structs =====

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
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
#[serde(deny_unknown_fields)]
pub struct GetExecutionParams {
    #[schemars(description = "Execution instance UUID")]
    pub instance_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
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
#[serde(deny_unknown_fields)]
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
#[serde(deny_unknown_fields)]
pub struct StopExecutionParams {
    #[schemars(description = "Execution instance UUID")]
    pub instance_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExecuteWorkflowWaitParams {
    #[schemars(description = "Workflow ID")]
    pub workflow_id: String,
    #[schemars(
        description = "Input data as JSON (format: {\"data\": {...}, \"variables\": {...}})"
    )]
    #[schemars(schema_with = "crate::mcp::tools::internal_api::workflow_inputs_schema")]
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
    let qs = list_executions_query_string(&params);
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

fn list_executions_query_string(params: &ListExecutionsParams) -> String {
    let mut query = Vec::new();
    if let Some(sid) = &params.workflow_id {
        push_query_param(&mut query, "workflowId", sid);
    }
    if let Some(status) = &params.status {
        push_query_param(&mut query, "status", status);
    }
    if let Some(p) = params.page {
        query.push(format!("page={}", p));
    }
    if let Some(s) = params.size {
        query.push(format!("size={}", s));
    }
    if let Some(sb) = &params.sort_by {
        push_query_param(&mut query, "sortBy", sb);
    }
    if let Some(so) = &params.sort_order {
        push_query_param(&mut query, "sortOrder", so);
    }
    if query.is_empty() {
        String::new()
    } else {
        format!("?{}", query.join("&"))
    }
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
                    let mut cut = 2000;
                    while cut > 0 && !s.is_char_boundary(cut) {
                        cut -= 1;
                    }
                    data.insert(
                        key.to_string(),
                        serde_json::json!({
                            "_truncated": true,
                            "_originalSize": s.len(),
                            "_preview": &s[..cut]
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
#[serde(deny_unknown_fields)]
pub struct PauseExecutionParams {
    #[schemars(description = "Execution instance UUID")]
    pub instance_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
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
#[serde(deny_unknown_fields)]
pub struct InspectStepParams {
    #[schemars(description = "Workflow ID")]
    pub workflow_id: String,
    #[schemars(description = "Execution instance UUID")]
    pub instance_id: String,
    #[schemars(description = "Step ID to inspect")]
    pub step_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
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
#[serde(deny_unknown_fields)]
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
    let inputs = match params.inputs {
        Some(inputs) => normalize_json_arg(inputs, "inputs")?,
        None => serde_json::json!({"data": {}, "variables": {}}),
    };
    let body = serde_json::json!({
        "inputs": inputs,
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
///
/// Array segments support Python-style negative suffix indexing (`-1` is the last
/// element), matching the workflow reference resolver so diagnostics agree with
/// runtime resolution.
fn resolve_json_path(value: &serde_json::Value, path: &str) -> Option<serde_json::Value> {
    let mut current = value;
    for segment in path.split('.') {
        if let serde_json::Value::Array(items) = current {
            current = items.get(signed_array_index(segment, items.len())?)?;
        } else if let Ok(idx) = segment.parse::<usize>() {
            current = current.get(idx)?;
        } else {
            current = current.get(segment)?;
        }
    }
    Some(current.clone())
}

/// Resolve a path segment to a concrete array index, supporting Python-style
/// negative suffix indexing (`-1` is the last element). Non-numeric segments and
/// out-of-range negatives return `None`.
fn signed_array_index(segment: &str, len: usize) -> Option<usize> {
    let raw: i64 = segment.parse().ok()?;
    if raw >= 0 {
        usize::try_from(raw).ok()
    } else {
        len.checked_sub(usize::try_from(raw.unsigned_abs()).ok()?)
    }
}

/// Helper: recursively replace large strings with an explicit truncation envelope.
fn truncate_large_strings(value: &serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::String(s) if s.len() > DEBUG_STRING_TRUNCATE_THRESHOLD_BYTES => {
            let mut cut = DEBUG_STRING_PREVIEW_BYTES.min(s.len());
            while cut > 0 && !s.is_char_boundary(cut) {
                cut -= 1;
            }
            json!({
                "_truncated": true,
                "_originalSize": s.len(),
                "_preview": &s[..cut],
            })
        }
        serde_json::Value::Array(items) => {
            serde_json::Value::Array(items.iter().map(truncate_large_strings).collect())
        }
        serde_json::Value::Object(map) => serde_json::Value::Object(
            map.iter()
                .map(|(key, child)| (key.clone(), truncate_large_strings(child)))
                .collect(),
        ),
        _ => value.clone(),
    }
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

/// Resolve a `steps.<id>.<path>`, `data.<path>`, `variables.<path>`, or
/// `loop.<path>` reference against step summaries / execution input.
///
/// This is the single source of truth for reference resolution in the MCP debug
/// tools — `inspect_step`, `why_execution_failed`, and `trace_reference` all route
/// step-output resolution through it so the diagnostic can never diverge from how
/// the workflow runtime resolves the same path. That divergence is exactly what
/// silently returned `null` for nested step-output references for two months: a
/// step summary's `outputs` field is the full step *envelope*
/// (`{ "outputs": <actual>, "stepId", "stepType", ... }`) — the runtime
/// `steps.<id>` value — so the path after the step id (the leading `outputs`
/// segment included) is walked against it directly.
///
/// `scope_id` is the scope of the step whose mapping is being resolved (only
/// `loop.*` needs it, to recover the iteration index — see
/// `loop_index_from_scope_id`).
fn resolve_reference_value(
    ref_path: &str,
    summaries: &serde_json::Value,
    execution: &serde_json::Value,
    scope_id: Option<&str>,
) -> Option<serde_json::Value> {
    let parts: Vec<&str> = ref_path.splitn(3, '.').collect();
    match parts.first().copied() {
        Some("steps") if parts.len() >= 2 => {
            let source_step_id = parts[1];
            if source_step_id == "__error" || source_step_id == "error" {
                // `__error`/`error` aren't real steps — the runtime injects the
                // captured onError envelope under this synthetic id when routing
                // to a failure handler (see `error_steps` in
                // runtara-workflow-stdlib). The MCP tools only see historical
                // step summaries, not which specific failure triggered a given
                // onError edge, so surface the first failed step's error as a
                // best-effort match — the same "primary failure" step
                // `why_execution_failed` already reports.
                let envelope = find_error_envelope(summaries)?;
                return match parts.get(2) {
                    Some(field_path) => resolve_json_path(&envelope, field_path),
                    None => Some(envelope),
                };
            }
            // A step summary's `outputs` field is the full step *envelope*
            // (`{ "outputs": <actual>, "stepId", "stepType", ... }`) — exactly the
            // runtime `steps.<id>` value. Resolve the remainder after the step id
            // (the leading `outputs` segment included) against that envelope, the
            // same way the workflow runtime resolves `steps.<id>.<path>`.
            let source = find_step_in_summaries(summaries, source_step_id)?;
            let envelope = source.get("outputs")?;
            match parts.get(2) {
                Some(field_path) => resolve_json_path(envelope, field_path),
                None => Some(envelope.clone()),
            }
        }
        Some("data") if parts.len() >= 2 => {
            let field = parts[1..].join(".");
            let inputs = execution
                .pointer("/data/inputs/data")
                .or_else(|| execution.pointer("/data/inputs"))?;
            resolve_json_path(inputs, &field)
        }
        Some("variables") if parts.len() >= 2 => {
            let field = parts[1..].join(".");
            execution
                .pointer("/data/inputs/variables")
                .or_else(|| execution.pointer("/data/variables"))
                .and_then(|variables| resolve_json_path(variables, &field))
        }
        Some("loop") => {
            // The iteration index is recoverable from the step's scope id (see
            // `loop_index_from_scope_id`). `loop.outputs` is not: it only ever
            // lived in the ephemeral per-iteration variables bag and was never
            // persisted, so it's intentionally absent here rather than
            // fabricated — the lookup below just returns `None` for it.
            let loop_context = json!({ "index": loop_index_from_scope_id(scope_id?)? });
            match parts.get(1..).filter(|p| !p.is_empty()) {
                Some(field_parts) => resolve_json_path(&loop_context, &field_parts.join(".")),
                None => Some(loop_context),
            }
        }
        _ => None,
    }
}

/// Recover the iteration index the runtime encoded into a Split/While scope id
/// (`sc_<stepId>_<index>` at the top level, `<parentScope>_<stepId>_<index>`
/// nested — see the Split/While iteration-variable builders in
/// runtara-workflow-stdlib's `direct_json.rs`). The trailing `_`-delimited
/// segment is always the numeric iteration index.
fn loop_index_from_scope_id(scope_id: &str) -> Option<u64> {
    scope_id.rsplit('_').next()?.parse().ok()
}

/// Locate the error envelope for a `steps.__error.*` / `steps.error.*`
/// reference: the first step in the summaries with a non-null error, mirroring
/// the "primary failure" convention `why_execution_failed` already uses
/// (`failed_steps.first()`).
fn find_error_envelope(summaries: &serde_json::Value) -> Option<serde_json::Value> {
    summaries
        .pointer("/data/steps")
        .and_then(|s| s.as_array())
        .and_then(|steps| steps.iter().find_map(step_error_envelope))
}

/// Recover the richest structured error envelope available for one step,
/// rather than whatever `step_error` collapsed it to (that helper exists to
/// answer "did this step fail", not to expose `.message`/`.category` fields).
///
/// The persisted shape genuinely varies by step type — confirmed against a
/// live server rather than assumed: an `Error` step's structured fields land
/// *flat* on `outputs` (`{_error, category, code, message, severity}`, no
/// nested `error` key — see the `"Error"` arm of `debug_end_output` in
/// runtara-workflow-stdlib), while an Agent failure's `outputs.error` is a raw
/// string, often wrapping a JSON envelope after a `Step <id> failed: Agent
/// <a>::<c>: ` prefix (`DirectJsonManifest::agent_error`). Recover both.
fn step_error_envelope(step: &serde_json::Value) -> Option<serde_json::Value> {
    if let Some(outputs) = step.get("outputs")
        && outputs.get("_error").and_then(|v| v.as_bool()) == Some(true)
    {
        return match outputs.get("error") {
            Some(serde_json::Value::Object(_)) => outputs.get("error").cloned(),
            Some(serde_json::Value::String(text)) => Some(recover_error_envelope(text)),
            // No nested `error` key (e.g. the Error step type): the structured
            // fields already sit flat on `outputs` alongside `_error`.
            _ => Some(outputs.clone()),
        };
    }

    match step.get("error") {
        Some(serde_json::Value::Object(_)) => step.get("error").cloned(),
        Some(serde_json::Value::String(text)) => Some(recover_error_envelope(text)),
        _ => None,
    }
}

/// Recover a structured error envelope from a raw error string, mirroring the
/// runtime's own recovery in `parse_error_envelope` (runtara-workflow-stdlib):
/// try the whole string as JSON first, then a `{...}` embedded after a
/// wrapping prefix. Falls back to wrapping the raw text as `{"message": ...}`
/// so `.message` still resolves to *something* instead of nothing.
fn recover_error_envelope(text: &str) -> serde_json::Value {
    if let Ok(parsed @ serde_json::Value::Object(_)) = serde_json::from_str(text) {
        return parsed;
    }
    if let Some(brace) = text.find('{')
        && let Ok(parsed @ serde_json::Value::Object(_)) =
            serde_json::from_str(text[brace..].trim())
    {
        return parsed;
    }
    json!({ "message": text })
}

fn resolve_nested_reference_envelopes(
    value: &serde_json::Value,
    summaries: &serde_json::Value,
    execution: &serde_json::Value,
    scope_id: Option<&str>,
    unresolved_refs: &mut Vec<String>,
) -> serde_json::Value {
    match value {
        serde_json::Value::Object(map) => {
            let fn_call = map.get("fn").and_then(|v| v.as_str());
            if fn_call.is_some()
                && let Some(arguments) = map.get("arguments").and_then(|v| v.as_array())
            {
                let mut resolved = serde_json::Map::new();
                for (key, child) in map {
                    if key == "arguments" {
                        let resolved_args: Vec<serde_json::Value> = arguments
                            .iter()
                            .map(|arg| {
                                if is_unqualified_reference_envelope(arg) {
                                    arg.clone()
                                } else {
                                    resolve_nested_reference_envelopes(
                                        arg,
                                        summaries,
                                        execution,
                                        scope_id,
                                        unresolved_refs,
                                    )
                                }
                            })
                            .collect();
                        resolved.insert(key.clone(), serde_json::Value::Array(resolved_args));
                    } else {
                        resolved.insert(
                            key.clone(),
                            resolve_nested_reference_envelopes(
                                child,
                                summaries,
                                execution,
                                scope_id,
                                unresolved_refs,
                            ),
                        );
                    }
                }
                return serde_json::Value::Object(resolved);
            }

            let condition_op = map.get("op").and_then(|v| v.as_str()).map(str::to_owned);
            if let Some(op) = condition_op.as_deref()
                && let Some(arguments) = map.get("arguments").and_then(|v| v.as_array())
            {
                let mut resolved = serde_json::Map::new();
                for (key, child) in map {
                    if key == "arguments" {
                        let resolved_args: Vec<serde_json::Value> = arguments
                            .iter()
                            .enumerate()
                            .map(|(index, arg)| {
                                if index == 0
                                    && is_field_argument_operator(op)
                                    && is_reference_envelope(arg)
                                {
                                    arg.clone()
                                } else {
                                    resolve_nested_reference_envelopes(
                                        arg,
                                        summaries,
                                        execution,
                                        scope_id,
                                        unresolved_refs,
                                    )
                                }
                            })
                            .collect();
                        resolved.insert(key.clone(), serde_json::Value::Array(resolved_args));
                    } else {
                        resolved.insert(
                            key.clone(),
                            resolve_nested_reference_envelopes(
                                child,
                                summaries,
                                execution,
                                scope_id,
                                unresolved_refs,
                            ),
                        );
                    }
                }
                return serde_json::Value::Object(resolved);
            }

            let is_reference_envelope =
                matches!(
                    map.get("valueType"),
                    Some(serde_json::Value::String(s)) if s == "reference"
                ) && matches!(map.get("value"), Some(serde_json::Value::String(_)));

            if is_reference_envelope {
                let ref_path = map
                    .get("value")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                let resolved = resolve_reference_value(ref_path, summaries, execution, scope_id)
                    .or_else(|| map.get("default").cloned());
                if let Some(resolved) = resolved {
                    return json!({
                        "valueType": "immediate",
                        "value": resolve_nested_reference_envelopes(
                            &resolved,
                            summaries,
                            execution,
                            scope_id,
                            unresolved_refs
                        ),
                    });
                }

                unresolved_refs.push(ref_path.to_string());
                return value.clone();
            }

            serde_json::Value::Object(
                map.iter()
                    .map(|(key, child)| {
                        (
                            key.clone(),
                            resolve_nested_reference_envelopes(
                                child,
                                summaries,
                                execution,
                                scope_id,
                                unresolved_refs,
                            ),
                        )
                    })
                    .collect(),
            )
        }
        serde_json::Value::Array(items) => serde_json::Value::Array(
            items
                .iter()
                .map(|item| {
                    resolve_nested_reference_envelopes(
                        item,
                        summaries,
                        execution,
                        scope_id,
                        unresolved_refs,
                    )
                })
                .collect(),
        ),
        _ => value.clone(),
    }
}

/// Mirror the runtime's `apply_composite`: a composite payload is an object or
/// array whose leaves are themselves MappingValue envelopes. Resolve each child
/// to its materialized value so inspect_step can surface the final JSON a
/// composite mapping sends to the agent (SYN-450).
fn resolve_composite_payload(
    payload: &serde_json::Value,
    summaries: &serde_json::Value,
    execution: &serde_json::Value,
    scope_id: Option<&str>,
    unresolved_refs: &mut Vec<String>,
) -> serde_json::Value {
    match payload {
        serde_json::Value::Object(map) => serde_json::Value::Object(
            map.iter()
                .map(|(key, child)| {
                    (
                        key.clone(),
                        resolve_mapping_envelope(
                            child,
                            summaries,
                            execution,
                            scope_id,
                            unresolved_refs,
                        ),
                    )
                })
                .collect(),
        ),
        serde_json::Value::Array(items) => serde_json::Value::Array(
            items
                .iter()
                .map(|item| {
                    resolve_mapping_envelope(item, summaries, execution, scope_id, unresolved_refs)
                })
                .collect(),
        ),
        // A composite payload should be an object/array, but degrade gracefully:
        // resolve any embedded reference envelopes rather than erroring.
        other => resolve_nested_reference_envelopes(
            other,
            summaries,
            execution,
            scope_id,
            unresolved_refs,
        ),
    }
}

/// Resolve a single MappingValue envelope (`{valueType, value, ...}`) to its
/// materialized value, mirroring the runtime's `apply_mapping_value`. Used for
/// composite children. `template` and unknown valueTypes degrade gracefully
/// (inspect_step is a best-effort reconstruction, not the runtime).
fn resolve_mapping_envelope(
    envelope: &serde_json::Value,
    summaries: &serde_json::Value,
    execution: &serde_json::Value,
    scope_id: Option<&str>,
    unresolved_refs: &mut Vec<String>,
) -> serde_json::Value {
    match envelope.get("valueType").and_then(|v| v.as_str()) {
        Some("reference") => {
            let path = envelope
                .get("value")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            match resolve_reference_value(path, summaries, execution, scope_id)
                .or_else(|| envelope.get("default").cloned())
            {
                Some(resolved) => resolve_nested_reference_envelopes(
                    &resolved,
                    summaries,
                    execution,
                    scope_id,
                    unresolved_refs,
                ),
                None => {
                    if !path.is_empty() {
                        unresolved_refs.push(path.to_string());
                    }
                    serde_json::Value::Null
                }
            }
        }
        Some("immediate") => {
            let inner = envelope
                .get("value")
                .cloned()
                .unwrap_or(serde_json::Value::Null);
            resolve_nested_reference_envelopes(
                &inner,
                summaries,
                execution,
                scope_id,
                unresolved_refs,
            )
        }
        Some("composite") => {
            let inner = envelope
                .get("value")
                .cloned()
                .unwrap_or(serde_json::Value::Null);
            resolve_composite_payload(&inner, summaries, execution, scope_id, unresolved_refs)
        }
        Some("template") => json!({
            "__runtimeTemplate": envelope.get("value").cloned().unwrap_or(serde_json::Value::Null),
        }),
        // Condition-like (`op`+`arguments`) or unknown shapes: best-effort
        // resolve any embedded references in place.
        _ => resolve_nested_reference_envelopes(
            envelope,
            summaries,
            execution,
            scope_id,
            unresolved_refs,
        ),
    }
}

fn is_reference_envelope(value: &serde_json::Value) -> bool {
    matches!(
        value.get("valueType"),
        Some(serde_json::Value::String(s)) if s == "reference"
    ) && matches!(value.get("value"), Some(serde_json::Value::String(_)))
}

fn is_unqualified_reference_envelope(value: &serde_json::Value) -> bool {
    let Some(path) = value.get("value").and_then(|v| v.as_str()) else {
        return false;
    };
    is_reference_envelope(value) && !is_qualified_workflow_path(path)
}

fn is_qualified_workflow_path(path: &str) -> bool {
    matches!(
        path.split('.').next(),
        Some("data" | "variables" | "workflow" | "steps" | "loop")
    )
}

fn is_field_argument_operator(op: &str) -> bool {
    matches!(
        op.to_ascii_uppercase().as_str(),
        "EQ" | "NE"
            | "GT"
            | "GTE"
            | "LT"
            | "LTE"
            | "STARTS_WITH"
            | "ENDS_WITH"
            | "CONTAINS"
            | "IN"
            | "NOT_IN"
            | "IS_DEFINED"
            | "IS_EMPTY"
            | "IS_NOT_EMPTY"
            | "SIMILARITY_GTE"
            | "MATCH"
            | "COSINE_DISTANCE_LTE"
            | "L2_DISTANCE_LTE"
    )
}

fn is_condition_like(value: &serde_json::Value) -> bool {
    value
        .get("op")
        .and_then(|op| op.as_str())
        .is_some_and(|op| !op.is_empty())
        && value
            .get("arguments")
            .and_then(|arguments| arguments.as_array())
            .is_some()
}

fn output_error_from_step(step: &serde_json::Value) -> Option<serde_json::Value> {
    let outputs = step.get("outputs")?;
    if outputs.get("_error").and_then(|v| v.as_bool()) != Some(true) {
        return None;
    }

    Some(
        outputs
            .get("error")
            .cloned()
            .unwrap_or_else(|| json!("Step output reported _error=true")),
    )
}

fn effective_step_status(step: &serde_json::Value) -> Option<&str> {
    match step.get("status").and_then(|v| v.as_str()) {
        Some("completed") if output_error_from_step(step).is_some() => Some("failed"),
        status => status,
    }
}

fn step_error(step: &serde_json::Value) -> serde_json::Value {
    step.get("error")
        .cloned()
        .or_else(|| output_error_from_step(step))
        .unwrap_or(serde_json::Value::Null)
}

/// Helper: resolve inputMapping references against step summaries. `scope_id`
/// is the scope of the step this input mapping belongs to (needed to resolve
/// `loop.*` references — see `resolve_reference_value`).
fn resolve_input_mappings(
    input_mapping: &serde_json::Value,
    summaries: &serde_json::Value,
    execution: &serde_json::Value,
    scope_id: Option<&str>,
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
                    let parts: Vec<&str> = ref_path.splitn(3, '.').collect();
                    match parts.first().copied() {
                        Some("steps") if parts.len() >= 2 => {
                            let source_step_id = parts[1];

                            if source_step_id == "__error" || source_step_id == "error" {
                                // Not a real step — see the matching special-case
                                // in resolve_reference_value for why `__error`
                                // never shows up in find_step_in_summaries.
                                entry["source"] = json!("error_context");
                                entry["resolvedValue"] = resolve_reference_value(
                                    ref_path, summaries, execution, scope_id,
                                )
                                .unwrap_or(json!(null));
                            } else if let Some(source) =
                                find_step_in_summaries(summaries, source_step_id)
                            {
                                entry["sourceStep"] = json!(source_step_id);
                                entry["sourceStatus"] =
                                    source.get("status").cloned().unwrap_or(json!("unknown"));

                                // Route the value through the shared resolver so this
                                // can never diverge from the runtime (see
                                // resolve_reference_value). Only surface a value once
                                // the source step actually has an output to resolve.
                                if source.get("outputs").is_some() {
                                    entry["resolvedValue"] = resolve_reference_value(
                                        ref_path, summaries, execution, scope_id,
                                    )
                                    .unwrap_or(json!(null));
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
                            // Route through the shared resolver, same as the
                            // steps/data arms above — this used to be dropped,
                            // silently reporting resolvedValue:null even though
                            // the runtime resolves workflow variables fine.
                            entry["resolvedValue"] =
                                resolve_reference_value(ref_path, summaries, execution, scope_id)
                                    .unwrap_or(json!(null));
                        }
                        Some("loop") => {
                            entry["source"] = json!("loop");
                            entry["resolvedValue"] =
                                resolve_reference_value(ref_path, summaries, execution, scope_id)
                                    .unwrap_or(json!(null));
                        }
                        _ => {}
                    }
                }
            }
            "immediate" => {
                let mut unresolved_refs = Vec::new();
                let resolved_value = resolve_nested_reference_envelopes(
                    &value,
                    summaries,
                    execution,
                    scope_id,
                    &mut unresolved_refs,
                );
                entry["resolvedValue"] = resolved_value;
                if !unresolved_refs.is_empty() {
                    entry["resolutionNote"] = json!(RUNTIME_NESTED_REFERENCE_NOTE);
                    entry["unresolvedNestedReferences"] = json!(unresolved_refs);
                }
            }
            "composite" => {
                // Mirror the runtime's `apply_composite`: a composite payload is an
                // object/array whose leaves are themselves MappingValue envelopes.
                // Materialize each so the author sees the final JSON the composite
                // sends to the agent (SYN-450).
                let mut unresolved_refs = Vec::new();
                let resolved_value = resolve_composite_payload(
                    &value,
                    summaries,
                    execution,
                    scope_id,
                    &mut unresolved_refs,
                );
                entry["resolvedValue"] = resolved_value;
                if !unresolved_refs.is_empty() {
                    entry["resolutionNote"] = json!(RUNTIME_NESTED_REFERENCE_NOTE);
                    entry["unresolvedNestedReferences"] = json!(unresolved_refs);
                }
            }
            "template" => {
                entry["template"] = value;
                entry["resolutionNote"] = json!(
                    "Template rendering is runtime-only and is not evaluated by inspect_step."
                );
            }
            _ if is_condition_like(mapping_value) => {
                let mut unresolved_refs = Vec::new();
                let resolved_value = resolve_nested_reference_envelopes(
                    mapping_value,
                    summaries,
                    execution,
                    scope_id,
                    &mut unresolved_refs,
                );
                entry["resolvedValue"] = resolved_value;
                if !unresolved_refs.is_empty() {
                    entry["resolutionNote"] = json!(RUNTIME_NESTED_REFERENCE_NOTE);
                    entry["unresolvedNestedReferences"] = json!(unresolved_refs);
                }
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

    // Fetch execution for input data. `?full=true`: step input mappings may
    // reference `data.X` fields the default detail fetch elides; resolve against
    // the complete value (the MCP re-truncates its own response).
    let execution = api_get(
        server,
        &format!(
            "/api/runtime/workflows/instances/{}?full=true",
            params.instance_id
        ),
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

    let scope_id = target.get("scopeId").and_then(|v| v.as_str());
    let resolved_inputs = resolve_input_mappings(&input_mapping, &summaries, &execution, scope_id);

    let response = json!({
        "step": {
            "stepId": target.get("stepId"),
            "stepName": target.get("stepName"),
            "stepType": target.get("stepType"),
            "status": target.get("status"),
            "durationMs": target.get("durationMs"),
            "error": target.get("error"),
        },
        "resolvedInputs": truncate_large_strings(&resolved_inputs),
        "outputs": target
            .get("outputs")
            .map(truncate_large_strings)
            .unwrap_or(serde_json::Value::Null),
    });

    json_result(response)
}

pub async fn trace_reference(
    server: &SmoMcpServer,
    params: TraceReferenceParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    validate_path_param("workflow_id", &params.workflow_id)?;
    validate_path_param("instance_id", &params.instance_id)?;

    let parts: Vec<&str> = params.reference.splitn(3, '.').collect();
    if parts.is_empty() {
        return Err(rmcp::ErrorData::invalid_params(
            "Reference path must not be empty".to_string(),
            None,
        ));
    }

    match parts[0] {
        "steps" => {
            if parts.len() < 2 {
                return Err(rmcp::ErrorData::invalid_params(
                    "Step reference must be 'steps.<stepId>[.outputs.<field>]'".to_string(),
                    None,
                ));
            }
            let step_id = parts[1];

            let summaries =
                fetch_full_step_summaries(server, &params.workflow_id, &params.instance_id).await?;

            if step_id == "__error" || step_id == "error" {
                // Not a real step — the runtime injects the captured onError
                // envelope under this synthetic id (see `error_steps` in
                // runtara-workflow-stdlib). Resolve it via the shared resolver's
                // "first failed step" fallback instead of requiring a literal
                // step named `__error` to exist in the summaries.
                let resolved = resolve_reference_value(
                    &params.reference,
                    &summaries,
                    &serde_json::Value::Null,
                    None,
                )
                .unwrap_or(json!(null));

                return json_result(json!({
                    "reference": params.reference,
                    "resolved": !resolved.is_null(),
                    "value": resolved,
                    "source": {
                        "type": "error_context",
                        "stepId": step_id,
                    }
                }));
            }

            let step = find_step_in_summaries(&summaries, step_id).ok_or_else(|| {
                rmcp::ErrorData::internal_error(
                    format!(
                        "Source step '{}' not found in execution {}",
                        step_id, params.instance_id
                    ),
                    None,
                )
            })?;

            // Resolve through the shared resolver (the single source of truth that
            // mirrors the runtime); `fullOutputs` still exposes the raw step
            // envelope for context.
            let outputs = step.get("outputs").cloned().unwrap_or(json!(null));
            let resolved = resolve_reference_value(
                &params.reference,
                &summaries,
                &serde_json::Value::Null,
                None,
            )
            .unwrap_or(json!(null));

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
        "loop" => {
            // `trace_reference` has no step_id param, so there's no scope to
            // resolve `loop.index` against here — this only stops the hard
            // "Unknown reference root" rejection; the resolver-level fix (and
            // its test coverage) is what actually proves loop.* resolves given
            // a scope id, via inspect_step / resolve_reference_value directly.
            let summaries =
                fetch_full_step_summaries(server, &params.workflow_id, &params.instance_id).await?;
            let resolved = resolve_reference_value(
                &params.reference,
                &summaries,
                &serde_json::Value::Null,
                None,
            )
            .unwrap_or(json!(null));

            json_result(json!({
                "reference": params.reference,
                "resolved": !resolved.is_null(),
                "value": resolved,
                "source": {
                    "type": "loop_context",
                }
            }))
        }
        "data" => {
            let field = parts[1..].join(".");
            // `?full=true`: the reference may point *into* a large input field
            // that the default detail fetch elides; resolve against the complete
            // value. The MCP response is re-truncated downstream, so the wire
            // stays bounded.
            let execution = api_get(
                server,
                &format!(
                    "/api/runtime/workflows/instances/{}?full=true",
                    params.instance_id
                ),
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
    let mut running_steps = Vec::new();
    for step in &steps {
        match effective_step_status(step) {
            Some("completed") => completed += 1,
            Some("failed") => {
                failed_steps.push(step.clone());
            }
            Some("running") => running_steps.push(step.clone()),
            _ => {}
        }
    }
    let running = running_steps.len();

    if status != "failed" && failed_steps.is_empty() {
        return json_result(json!({
            "execution": {
                "instanceId": params.instance_id,
                "status": status,
            },
            "message": format!("Execution is not failed (status: {})", status),
        }));
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

        let scope_id = first_failed.get("scopeId").and_then(|v| v.as_str());
        let resolved_inputs =
            resolve_input_mappings(&input_mapping, &summaries, &execution, scope_id);

        json!({
            "stepId": first_failed.get("stepId"),
            "stepName": first_failed.get("stepName"),
            "stepType": first_failed.get("stepType"),
            "status": effective_step_status(first_failed).unwrap_or("unknown"),
            "error": step_error(first_failed),
            "durationMs": first_failed.get("durationMs"),
            "resolvedInputs": resolved_inputs,
        })
    } else if status == "failed" && !running_steps.is_empty() {
        // The execution failed but no step recorded an error, yet one or more
        // steps were still in flight (a `step_debug_start` with no matching
        // `step_debug_end`). The run was terminated abruptly — e.g. a guest trap
        // such as the per-instance memory limit being exceeded — before the step
        // could record its outcome. Attribute the instance-level failure reason
        // to the in-flight step(s) so the failure isn't a silent
        // running/null-error record.
        let in_flight: Vec<serde_json::Value> = running_steps
            .iter()
            .map(|s| {
                json!({
                    "stepId": s.get("stepId"),
                    "stepName": s.get("stepName"),
                    "stepType": s.get("stepType"),
                    "scopeId": s.get("scopeId"),
                })
            })
            .collect();
        let first = &running_steps[0];
        json!({
            "stepId": first.get("stepId"),
            "stepName": first.get("stepName"),
            "stepType": first.get("stepType"),
            "scopeId": first.get("scopeId"),
            "status": "interrupted",
            "error": execution.pointer("/data/error"),
            "durationMs": first.get("durationMs"),
            "note": "Step was in flight when the execution terminated abnormally; \
                     no step-level error was recorded. The error shown is the \
                     instance-level failure reason.",
            "inFlightSteps": in_flight,
        })
    } else {
        json!(null)
    };

    json_result(json!({
        "execution": {
            "instanceId": params.instance_id,
            "status": status,
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

#[cfg(test)]
mod tests {
    use super::*;
    use schemars::JsonSchema;
    use serde_json::json;

    fn generated_property_schema<T: JsonSchema>(property: &str) -> serde_json::Value {
        let schema = serde_json::to_value(schemars::schema_for!(T)).unwrap();
        schema
            .get("properties")
            .and_then(|properties| properties.get(property))
            .cloned()
            .unwrap_or_else(|| panic!("missing property schema for {property}: {schema:#}"))
    }

    /// SYN-448: the diagnostic path resolver must honor Python-style negative
    /// array indices so `inspect_step`/`trace_reference` agree with runtime
    /// reference resolution instead of reporting `-1` as null.
    #[test]
    fn resolve_json_path_supports_negative_indices() {
        let value = json!({ "items": ["a", "b", "c"] });

        assert_eq!(resolve_json_path(&value, "items.-1"), Some(json!("c")));
        assert_eq!(resolve_json_path(&value, "items.-3"), Some(json!("a")));
        assert_eq!(resolve_json_path(&value, "items.0"), Some(json!("a")));
        assert_eq!(resolve_json_path(&value, "items.-4"), None);
        assert_eq!(resolve_json_path(&value, "items.5"), None);
    }

    fn summaries() -> serde_json::Value {
        // A step summary's `outputs` is the full step *envelope* produced by the
        // runtime — `{ "outputs": <actual>, "stepId", "stepType", ... }` — not the
        // bare capability output. Reference resolution must walk that envelope the
        // same way the runtime resolves `steps.<id>.outputs.<path>`.
        json!({
            "data": {
                "steps": [
                    {
                        "stepId": "build",
                        "status": "completed",
                        "outputs": {
                            "outputs": {
                                "status": "active",
                                "nested": {"name": "from-step"}
                            },
                            "stepId": "build",
                            "stepName": "Build",
                            "stepType": "Agent"
                        }
                    },
                    {
                        "stepId": "embed",
                        "status": "completed",
                        "outputs": {
                            "outputs": {
                                "embeddings": [[0.1, 0.2, 0.3]]
                            },
                            "stepId": "embed",
                            "stepName": "Embed",
                            "stepType": "Agent"
                        }
                    }
                ]
            }
        })
    }

    fn execution() -> serde_json::Value {
        json!({
            "data": {
                "inputs": {
                    "data": {
                        "customer": {"name": "Ada"},
                        "threshold": 7
                    },
                    "variables": {
                        "limit": 10
                    }
                }
            }
        })
    }

    #[test]
    fn execute_workflow_wait_inputs_schema_declares_object() {
        let inputs = generated_property_schema::<ExecuteWorkflowWaitParams>("inputs");
        assert_eq!(inputs["type"], "object");
        assert_eq!(inputs["required"], serde_json::json!(["data"]));
        assert_eq!(inputs["properties"]["variables"]["type"], "object");
    }

    #[test]
    fn list_executions_query_uses_api_parameter_names() {
        let query = list_executions_query_string(&ListExecutionsParams {
            workflow_id: Some("workflow/needs encoding".to_string()),
            status: Some("running,queued".to_string()),
            page: Some(2),
            size: Some(50),
            sort_by: Some("createdAt".to_string()),
            sort_order: Some("desc".to_string()),
        });

        assert!(query.contains("workflowId=workflow%2Fneeds%20encoding"));
        assert!(query.contains("status=running%2Cqueued"));
        assert!(query.contains("page=2"));
        assert!(query.contains("size=50"));
        assert!(query.contains("sortBy=createdAt"));
        assert!(query.contains("sortOrder=desc"));
        assert!(!query.contains("workflow_id="));
        assert!(!query.contains("sort_by="));
    }

    #[test]
    fn truncate_large_strings_recurses_with_explicit_envelope() {
        let large = format!(
            "{}{}",
            "a".repeat(DEBUG_STRING_TRUNCATE_THRESHOLD_BYTES),
            "é"
        );
        let value = json!({
            "small": "unchanged",
            "items": [{"body": large}]
        });

        let truncated = truncate_large_strings(&value);

        assert_eq!(truncated["small"], json!("unchanged"));
        assert_eq!(truncated["items"][0]["body"]["_truncated"], json!(true));
        assert_eq!(
            truncated["items"][0]["body"]["_originalSize"],
            json!(DEBUG_STRING_TRUNCATE_THRESHOLD_BYTES + 2)
        );
        assert_eq!(
            truncated["items"][0]["body"]["_preview"]
                .as_str()
                .unwrap()
                .len(),
            DEBUG_STRING_PREVIEW_BYTES
        );
    }

    #[test]
    fn immediate_condition_resolves_nested_references_where_available() {
        let input_mapping = json!({
            "condition": {
                "valueType": "immediate",
                "value": {
                    "type": "operation",
                    "op": "EQ",
                    "arguments": [
                        {"valueType": "reference", "value": "customer_name"},
                        {"valueType": "reference", "value": "steps.build.outputs.nested.name"}
                    ]
                }
            }
        });

        let resolved = resolve_input_mappings(&input_mapping, &summaries(), &execution(), None);

        assert_eq!(
            resolved["condition"]["resolvedValue"],
            json!({
                "type": "operation",
                "op": "EQ",
                "arguments": [
                    {"valueType": "reference", "value": "customer_name"},
                    {"valueType": "immediate", "value": "from-step"}
                ]
            })
        );
        assert!(resolved["condition"].get("resolutionNote").is_none());
    }

    /// SYN-450: composite mappings now expose a fully materialized `resolvedValue`
    /// (nested immediate/reference/composite envelopes resolved), not just the raw
    /// `mapping` tree.
    #[test]
    fn composite_mapping_resolves_nested_envelopes_to_final_value() {
        let input_mapping = json!({
            "value": {
                "valueType": "composite",
                "value": {
                    "lit": {"valueType": "immediate", "value": "literal"},
                    "from_input": {"valueType": "reference", "value": "data.customer.name"},
                    "from_step": {"valueType": "reference", "value": "steps.build.outputs.status"},
                    "nested": {
                        "valueType": "composite",
                        "value": [
                            {"valueType": "immediate", "value": 1},
                            {"valueType": "reference", "value": "variables.limit"}
                        ]
                    }
                }
            }
        });

        let resolved = resolve_input_mappings(&input_mapping, &summaries(), &execution(), None);
        let entry = &resolved["value"];

        assert_eq!(
            entry["resolvedValue"],
            json!({
                "lit": "literal",
                "from_input": "Ada",
                "from_step": "active",
                "nested": [1, 10]
            }),
            "composite resolvedValue should materialize nested envelopes: {entry:#}"
        );
        // The raw mapping tree is still surfaced alongside the resolved value.
        assert!(entry.get("mapping").is_some());
        assert!(entry.get("resolutionNote").is_none());
    }

    /// SYN-450: an unresolvable reference inside a composite degrades to null and
    /// is reported under `unresolvedNestedReferences`.
    #[test]
    fn composite_mapping_tracks_unresolved_reference() {
        let input_mapping = json!({
            "value": {
                "valueType": "composite",
                "value": {
                    "ok": {"valueType": "reference", "value": "data.customer.name"},
                    "missing": {"valueType": "reference", "value": "steps.nope.outputs.x"}
                }
            }
        });

        let resolved = resolve_input_mappings(&input_mapping, &summaries(), &execution(), None);
        let entry = &resolved["value"];

        assert_eq!(entry["resolvedValue"]["ok"], json!("Ada"));
        assert_eq!(entry["resolvedValue"]["missing"], json!(null));
        assert_eq!(
            entry["unresolvedNestedReferences"],
            json!(["steps.nope.outputs.x"])
        );
        assert!(entry.get("resolutionNote").is_some());
    }

    #[test]
    fn condition_resolution_preserves_field_arg_and_resolves_value_arg() {
        let input_mapping = json!({
            "condition": {
                "type": "operation",
                "op": "EQ",
                "arguments": [
                    {"valueType": "reference", "value": "item.status"},
                    {"valueType": "reference", "value": "steps.build.outputs.status"}
                ]
            }
        });

        let resolved = resolve_input_mappings(&input_mapping, &summaries(), &execution(), None);

        assert_eq!(
            resolved["condition"]["resolvedValue"]["arguments"][0],
            json!({"valueType": "reference", "value": "item.status"})
        );
        assert_eq!(
            resolved["condition"]["resolvedValue"]["arguments"][1],
            json!({"valueType": "immediate", "value": "active"})
        );
        assert!(resolved["condition"].get("resolutionNote").is_none());
        assert!(
            resolved["condition"]
                .get("unresolvedNestedReferences")
                .is_none()
        );
    }

    #[test]
    fn score_expression_resolution_preserves_column_ref_and_resolves_query_vector() {
        let input_mapping = json!({
            "score_expression": {
                "valueType": "immediate",
                "value": {
                    "alias": "distance",
                    "expression": {
                        "fn": "COSINE_DISTANCE",
                        "arguments": [
                            {"valueType": "reference", "value": "embedding"},
                            {"valueType": "reference", "value": "steps.embed.outputs.embeddings.0"}
                        ]
                    }
                }
            }
        });

        let resolved = resolve_input_mappings(&input_mapping, &summaries(), &execution(), None);

        assert_eq!(
            resolved["score_expression"]["resolvedValue"]["expression"]["arguments"],
            json!([
                {"valueType": "reference", "value": "embedding"},
                {"valueType": "immediate", "value": [0.1, 0.2, 0.3]}
            ])
        );
        assert!(
            resolved["score_expression"]
                .get("unresolvedNestedReferences")
                .is_none()
        );
    }

    #[test]
    fn reference_resolves_array_index_path_through_output_envelope() {
        // Regression for the misreported "EmbedWorkflow nested-path" bug. The
        // diagnostic must resolve `steps.<id>.outputs.<obj>.<idx>.<field>` the same
        // way the runtime does. A step summary's `outputs` is the *envelope*
        // (`{ outputs: <actual>, stepId, ... }`); the resolver previously stripped
        // the literal `outputs` segment and indexed the envelope directly, so any
        // path through a step output came back `null` — e.g. inspect_step /
        // why_execution_failed reported every embed input as null even though the
        // runtime delivered the real value to the child.
        let summaries = json!({
            "data": { "steps": [{
                "stepId": "lookup_file",
                "status": "completed",
                "outputs": {
                    "outputs": { "instances": [{ "customer_id": "53883889550" }] },
                    "stepId": "lookup_file",
                    "stepType": "Agent"
                }
            }]}
        });
        let input_mapping = json!({
            "customer_id": {
                "valueType": "reference",
                "value": "steps.lookup_file.outputs.instances.0.customer_id"
            }
        });

        let resolved = resolve_input_mappings(&input_mapping, &summaries, &execution(), None);

        assert_eq!(
            resolved["customer_id"]["resolvedValue"],
            json!("53883889550")
        );
        assert_eq!(resolved["customer_id"]["sourceStep"], json!("lookup_file"));
        assert_eq!(resolved["customer_id"]["sourceStatus"], json!("completed"));
    }

    #[test]
    fn reference_resolves_bare_step_and_outputs_envelope() {
        // `steps.<id>` resolves to the runtime envelope and `steps.<id>.outputs` to
        // the actual output — matching what the workflow runtime exposes.
        let resolved_bare =
            resolve_reference_value("steps.build", &summaries(), &execution(), None);
        assert_eq!(
            resolved_bare.and_then(|v| v.get("stepType").cloned()),
            Some(json!("Agent"))
        );

        let resolved_outputs =
            resolve_reference_value("steps.build.outputs", &summaries(), &execution(), None);
        assert_eq!(
            resolved_outputs,
            Some(json!({ "status": "active", "nested": { "name": "from-step" } }))
        );
    }

    /// A summaries fixture with one additional failed step, for `steps.__error.*`
    /// coverage — the runtime injects the captured envelope from whichever step
    /// failed, so tests need at least one failed step present.
    fn summaries_with_failed_step() -> serde_json::Value {
        let mut summaries = summaries();
        summaries["data"]["steps"]
            .as_array_mut()
            .unwrap()
            .push(json!({
                "stepId": "notify",
                "status": "failed",
                "error": {
                    "message": "Delivery failed",
                    "code": "SMTP_TIMEOUT",
                    "category": "transient",
                    "severity": "error",
                    "stepId": "notify"
                }
            }));
        summaries
    }

    #[test]
    fn input_mapping_step_value_matches_shared_resolver() {
        // Drift guard: resolve_input_mappings (inspect_step / why_execution_failed)
        // must resolve step-output references to the same value as the shared
        // resolve_reference_value. If a future change touches one resolver and not
        // the other, this fails instead of silently returning null like the
        // original bug.
        let summaries = summaries_with_failed_step();
        let execution = execution();
        for (path, scope_id) in [
            ("steps.build.outputs.nested.name", None),
            ("steps.build.outputs.status", None),
            ("steps.build.outputs", None),
            ("steps.embed.outputs.embeddings.0", None),
            ("steps.missing.outputs.x", None),
            ("steps.__error.message", None),
            ("variables.limit", None),
            ("loop.index", Some("sc_whileStep_3")),
        ] {
            let mapping = json!({ "field": { "valueType": "reference", "value": path } });
            let resolved = resolve_input_mappings(&mapping, &summaries, &execution, scope_id);
            let via_input_mapping = resolved["field"]
                .get("resolvedValue")
                .cloned()
                .unwrap_or(serde_json::Value::Null);
            let via_shared = resolve_reference_value(path, &summaries, &execution, scope_id)
                .unwrap_or(json!(null));
            assert_eq!(
                via_input_mapping, via_shared,
                "resolve_input_mappings diverged from resolve_reference_value for {path}"
            );
        }
    }

    #[test]
    fn loop_index_resolves_from_scope_id() {
        // SYN-467: `loop.index` is recoverable from the step's scope id even
        // though it's never persisted as its own field — the runtime always
        // encodes it as the trailing `_`-delimited segment.
        assert_eq!(
            resolve_reference_value("loop.index", &summaries(), &execution(), Some("sc_while_3")),
            Some(json!(3))
        );
        assert_eq!(
            resolve_reference_value(
                "loop.index",
                &summaries(),
                &execution(),
                Some("parentScope_while_2")
            ),
            Some(json!(2))
        );
        assert_eq!(
            resolve_reference_value("loop", &summaries(), &execution(), Some("sc_while_0")),
            Some(json!({ "index": 0 }))
        );
    }

    #[test]
    fn loop_outputs_is_not_reconstructable_from_persisted_state() {
        // SYN-467: unlike `loop.index`, the accumulated `loop.outputs` value only
        // ever lived in the ephemeral per-iteration variables bag and was never
        // persisted anywhere retrievable — this must stay `None` rather than
        // silently fabricate a value.
        assert_eq!(
            resolve_reference_value(
                "loop.outputs",
                &summaries(),
                &execution(),
                Some("sc_while_3")
            ),
            None
        );
    }

    #[test]
    fn loop_reference_without_scope_is_unresolved() {
        // No step context (e.g. trace_reference has no step_id param) means no
        // scope to resolve the iteration index against.
        assert_eq!(
            resolve_reference_value("loop.index", &summaries(), &execution(), None),
            None
        );
    }

    #[test]
    fn steps_dunder_error_resolves_to_first_failed_steps_envelope() {
        // SYN-467: `steps.__error.*` (and its `steps.error.*` alias) aren't a real
        // step — the runtime injects the onError envelope under this synthetic
        // id. The MCP tools only see historical summaries, so this resolves to
        // the first failed step's error, mirroring why_execution_failed's
        // "primary failure" convention.
        let summaries = summaries_with_failed_step();
        assert_eq!(
            resolve_reference_value("steps.__error.message", &summaries, &execution(), None),
            Some(json!("Delivery failed"))
        );
        assert_eq!(
            resolve_reference_value("steps.error.category", &summaries, &execution(), None),
            Some(json!("transient"))
        );
    }

    #[test]
    fn steps_dunder_error_resolves_flat_error_step_outputs() {
        // SYN-467: verified against a live server — an `Error`-step-type failure
        // has no nested `outputs.error` object at all; the structured fields
        // (`category`/`code`/`message`/`severity`) sit flat on `outputs`
        // alongside `_error`, and the top-level `error` field collapses to the
        // generic "Step output reported _error=true" string. Must still recover
        // the real fields from `outputs` directly.
        let summaries = json!({
            "data": {
                "steps": [{
                    "stepId": "boom",
                    "stepType": "Error",
                    "status": "failed",
                    "error": "Step output reported _error=true",
                    "outputs": {
                        "_error": true,
                        "category": "transient",
                        "code": "SMTP_TIMEOUT",
                        "message": "Delivery failed",
                        "severity": "error"
                    }
                }]
            }
        });

        assert_eq!(
            resolve_reference_value("steps.__error.message", &summaries, &execution(), None),
            Some(json!("Delivery failed"))
        );
        assert_eq!(
            resolve_reference_value("steps.__error.code", &summaries, &execution(), None),
            Some(json!("SMTP_TIMEOUT"))
        );
    }

    #[test]
    fn steps_dunder_error_recovers_wrapped_agent_failure_string() {
        // SYN-467: verified against a live server — an Agent capability failure
        // persists `outputs.error` (and the top-level `error` field) as a raw
        // string wrapping a JSON envelope after a `Step <id> failed: Agent
        // <a>::<c>: ` prefix, not a JSON object. Must recover the embedded JSON
        // rather than treating the whole string as unresolvable.
        let summaries = json!({
            "data": {
                "steps": [{
                    "stepId": "call",
                    "stepType": "Agent",
                    "status": "failed",
                    "error": "Step call failed: Agent http::http-request: {\"attributes\":{\"url\":\"http://127.0.0.1:1/\"},\"category\":\"transient\",\"code\":\"NETWORK_ERROR\",\"message\":\"request to http://127.0.0.1:1/ failed: Transport error: HTTP error: ErrorCode::ConnectionRefused\",\"retryable\":true,\"severity\":\"warning\"}",
                    "outputs": {
                        "_error": true,
                        "error": "Step call failed: Agent http::http-request: {\"attributes\":{\"url\":\"http://127.0.0.1:1/\"},\"category\":\"transient\",\"code\":\"NETWORK_ERROR\",\"message\":\"request to http://127.0.0.1:1/ failed: Transport error: HTTP error: ErrorCode::ConnectionRefused\",\"retryable\":true,\"severity\":\"warning\"}"
                    }
                }]
            }
        });

        assert_eq!(
            resolve_reference_value("steps.__error.category", &summaries, &execution(), None),
            Some(json!("transient"))
        );
        assert_eq!(
            resolve_reference_value("steps.__error.code", &summaries, &execution(), None),
            Some(json!("NETWORK_ERROR"))
        );
        assert_eq!(
            resolve_reference_value("steps.__error.message", &summaries, &execution(), None),
            Some(json!(
                "request to http://127.0.0.1:1/ failed: Transport error: HTTP error: ErrorCode::ConnectionRefused"
            ))
        );
    }

    #[test]
    fn error_envelope_recovery_falls_back_to_wrapping_unparseable_text() {
        // SYN-467: a raw error string with no embedded JSON at all (e.g. the
        // generic "_error" fallback text) still yields a `.message` instead of
        // resolving to nothing.
        let summaries = json!({
            "data": {
                "steps": [{
                    "stepId": "weird",
                    "status": "failed",
                    "error": "totally unstructured failure text"
                }]
            }
        });

        assert_eq!(
            resolve_reference_value("steps.__error.message", &summaries, &execution(), None),
            Some(json!("totally unstructured failure text"))
        );
    }

    #[test]
    fn variables_reference_now_sets_resolved_value() {
        // SYN-467: this arm used to only set `source`/`variableName` metadata and
        // never called the resolver, so inspect_step always reported
        // resolvedValue:null for variable-mapped inputs even though the runtime
        // resolves workflow variables fine.
        let input_mapping = json!({
            "limit": { "valueType": "reference", "value": "variables.limit" }
        });

        let resolved = resolve_input_mappings(&input_mapping, &summaries(), &execution(), None);

        assert_eq!(resolved["limit"]["resolvedValue"], json!(10));
        assert_eq!(resolved["limit"]["source"], json!("variable"));
        assert_eq!(resolved["limit"]["variableName"], json!("limit"));
    }

    #[test]
    fn template_mapping_reports_runtime_only_rendering() {
        let input_mapping = json!({
            "message": {
                "valueType": "template",
                "value": "Hello {{ data.customer.name }}"
            }
        });

        let resolved = resolve_input_mappings(&input_mapping, &summaries(), &execution(), None);

        assert_eq!(
            resolved["message"]["resolutionNote"],
            json!("Template rendering is runtime-only and is not evaluated by inspect_step.")
        );
        assert!(resolved["message"].get("resolvedValue").is_none());
    }

    #[test]
    fn completed_step_with_output_error_has_failed_effective_status() {
        let step = json!({
            "status": "completed",
            "outputs": {
                "_error": true,
                "error": {"message": "Capability failed"}
            }
        });

        assert_eq!(effective_step_status(&step), Some("failed"));
        assert_eq!(step_error(&step), json!({"message": "Capability failed"}));
    }

    #[test]
    fn completed_step_without_output_error_keeps_status() {
        let step = json!({
            "status": "completed",
            "outputs": {"ok": true}
        });

        assert_eq!(effective_step_status(&step), Some("completed"));
        assert_eq!(step_error(&step), serde_json::Value::Null);
    }
}
