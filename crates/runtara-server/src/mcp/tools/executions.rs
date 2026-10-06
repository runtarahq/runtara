use rmcp::model::{CallToolResult, ContentBlock};
use runtara_workflow_stdlib::reference_path::{
    array_index, is_array_index_token, is_workflow_reference, reference_segments,
};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;
use std::collections::BTreeSet;

use super::super::server::SmoMcpServer;
use super::internal_api::{api_get, api_post, normalize_json_arg, validate_path_param};

const DEBUG_STRING_TRUNCATE_THRESHOLD_BYTES: usize = 4000;
const DEBUG_STRING_PREVIEW_BYTES: usize = 2000;
const RUNTIME_NESTED_REFERENCE_NOTE: &str =
    "Nested condition references are resolved by workflow runtime before agent dispatch.";

fn json_result(value: serde_json::Value) -> Result<CallToolResult, rmcp::ErrorData> {
    Ok(CallToolResult::success(vec![ContentBlock::text(
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
    /// Case-insensitive literal substring search across run labels, workflow names/IDs,
    /// execution IDs, and statuses. Applied before pagination and total counting;
    /// punctuation is literal, not a wildcard pattern.
    pub search: Option<String>,
    /// Exact, case-sensitive match on the stored runLabel. Labels are not unique;
    /// all matches contribute to the filtered total before pagination. Use the
    /// normalized, possibly truncated label returned by list_executions/get_execution.
    pub run_label: Option<String>,
    /// Only the children of this execution: the runs its control:start steps
    /// started (each child reports its parent as parentInstanceId).
    pub parent_instance_id: Option<String>,
    #[schemars(description = "Filter by workflow ID")]
    pub workflow_id: Option<String>,
    #[schemars(
        description = "Comma-separated statuses — matches executions holding any one of them: queued,compiling,running,suspended,completed,failed,timeout,cancelled"
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
    /// Keep only executions whose published state (what their SetState steps
    /// wrote) matches every filter: a JSON array of {field, op, value}, op
    /// eq, ne, in, lt, lte, gt, gte or exists. A run without the field does
    /// not match; date-times compare in UTC. State is never returned here;
    /// get_execution shows one run's state.
    #[schemars(schema_with = "crate::mcp::tools::internal_api::optional_json_array_schema")]
    pub state: Option<serde_json::Value>,
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
    #[schemars(
        description = "Filter by status (running, suspended, completed, failed). An unfinished step reads suspended while its run is suspended."
    )]
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
    /// Optional exact execution reference (1–1024 printable ASCII bytes).
    pub run_label: Option<String>,
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
    let mut result = match &params.state {
        // State filters need the POST query form: they do not fit in a query
        // string.
        Some(state) => {
            let state = normalize_json_arg(state.clone(), "state")?;
            api_post(
                server,
                "/api/runtime/executions/query",
                Some(json!({
                    "search": params.search,
                    "runLabel": params.run_label,
                    "parentInstanceId": params.parent_instance_id,
                    "workflowId": params.workflow_id,
                    "status": params.status,
                    "page": params.page,
                    "size": params.size,
                    "sortBy": params.sort_by,
                    "sortOrder": params.sort_order,
                    "state": state,
                })),
            )
            .await?
        }
        None => {
            let qs = list_executions_query_string(&params);
            api_get(server, &format!("/api/runtime/executions{}", qs)).await?
        }
    };

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
    if let Some(search) = &params.search {
        push_query_param(&mut query, "search", search);
    }
    if let Some(label) = &params.run_label {
        push_query_param(&mut query, "runLabel", label);
    }
    if let Some(parent) = &params.parent_instance_id {
        push_query_param(&mut query, "parentInstanceId", parent);
    }
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
        description = "Reference path to resolve (e.g., 'steps.getVariant.outputs.price', 'data.orderId', 'variables.counter', 'workflow.inputs.data.orderId', 'steps.__error.message'). loop/iteration/item references need a step's scope; use inspect_step for those."
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
        "runLabel": params.run_label,
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

/// Cap on step records per targeted summaries fetch. Matches the REST
/// endpoint's own hard maximum; fetches that hit it report a truncation flag
/// instead of silently dropping records.
const STEP_SUMMARY_FETCH_LIMIT: u32 = 500;
/// Cap on in-flight steps listed when diagnosing an abnormally terminated
/// execution.
const IN_FLIGHT_FETCH_LIMIT: u32 = 50;

/// Filters for a targeted step-summaries fetch. The REST endpoint always
/// returns full inputs/outputs per record, so narrowing *which* records are
/// fetched (by step id and/or status) is what keeps debugging loop-heavy
/// instances bounded — an unfiltered fetch drags every step's full payload
/// across the wire only to discard most of it.
struct StepSummariesFetch<'a> {
    /// Restrict to these step ids (empty = no step-id filter). Sent
    /// comma-separated, so ids containing commas cannot be filtered on.
    step_ids: &'a [String],
    /// Restrict to a status ("running", "completed", "failed").
    status: Option<&'a str>,
    /// Maximum records returned; the response's `totalCount` still reflects
    /// everything matching the filter. `0` fetches counts only.
    limit: u32,
}

/// Helper: fetch step summaries (full inputs/outputs) matching the filter.
/// Records are newest-first, so `limit: 1` yields a step id's most recent
/// record — the same one `find_step_in_summaries` picks from a full listing.
async fn fetch_step_summaries(
    server: &SmoMcpServer,
    workflow_id: &str,
    instance_id: &str,
    fetch: &StepSummariesFetch<'_>,
) -> Result<serde_json::Value, rmcp::ErrorData> {
    let mut query = Vec::new();
    if !fetch.step_ids.is_empty() {
        push_query_param(&mut query, "stepIds", &fetch.step_ids.join(","));
    }
    if let Some(status) = fetch.status {
        push_query_param(&mut query, "status", status);
    }
    query.push(format!("limit={}", fetch.limit));
    api_get(
        server,
        &format!(
            "/api/runtime/workflows/{}/instances/{}/steps?{}",
            workflow_id,
            instance_id,
            query.join("&")
        ),
    )
    .await
}

/// Extract the step records from a summaries response.
fn steps_from_summaries(summaries: &serde_json::Value) -> Vec<serde_json::Value> {
    summaries
        .pointer("/data/steps")
        .and_then(|s| s.as_array())
        .cloned()
        .unwrap_or_default()
}

/// Extract the exact number of records matching the fetch's filter (the
/// endpoint counts beyond the page limit).
fn total_count_from_summaries(summaries: &serde_json::Value) -> u64 {
    summaries
        .pointer("/data/totalCount")
        .and_then(|v| v.as_u64())
        .unwrap_or(0)
}

/// Truncation flag for a capped fetch: `Some({returned, totalMatching})`
/// when records beyond the page limit exist, `None` when the page is
/// complete.
fn truncation_flag(returned: usize, total_matching: u64) -> Option<serde_json::Value> {
    (total_matching > returned as u64).then(|| {
        json!({
            "returned": returned,
            "totalMatching": total_matching,
        })
    })
}

/// Wrap targeted-fetch results in the same envelope shape the `/steps`
/// listing returns, so the shared resolvers (`find_step_in_summaries`,
/// `resolve_reference_value`, `resolve_input_mappings`) work unchanged on a
/// reduced step set.
fn synthetic_summaries(steps: Vec<serde_json::Value>) -> serde_json::Value {
    json!({ "data": { "steps": steps } })
}

/// Collect the step ids referenced as `steps.<id>...` anywhere in a step's
/// input mapping, plus whether the synthetic `__error`/`error` step (or its
/// bare `__error.*`/`error.*` alias) is referenced (not a real step — it
/// resolves to the newest failed step's error envelope). Direct references, composite payloads, condition/fn
/// arguments, and immediate values all embed the same
/// `{valueType: "reference", value: "steps..."}` envelope shape, so one
/// uniform walk over the mapping tree covers every place a reference can
/// hide. Over-collecting is harmless (an extra step is fetched); the ids
/// found here decide which steps' payloads are worth fetching at all.
fn referenced_step_ids(mapping: &serde_json::Value) -> (std::collections::BTreeSet<String>, bool) {
    fn walk(
        value: &serde_json::Value,
        ids: &mut std::collections::BTreeSet<String>,
        wants_error: &mut bool,
    ) {
        match value {
            serde_json::Value::Object(map) => {
                if matches!(
                    map.get("valueType"),
                    Some(serde_json::Value::String(kind)) if kind == "reference"
                ) && let Some(path) = map.get("value").and_then(|v| v.as_str())
                {
                    // Tokenized like the runtime reads it, so a bracketed id
                    // (`steps['fetch'].outputs`) is collected too.
                    let segments = reference_segments(path);
                    match segments.as_slice() {
                        [root, id, ..] if root == "steps" => match id.as_str() {
                            "__error" | "error" => *wants_error = true,
                            id => {
                                ids.insert(id.to_string());
                            }
                        },
                        // The bare onError aliases read the same envelope.
                        [root, ..] if is_error_alias(root) => *wants_error = true,
                        _ => {}
                    }
                }
                for child in map.values() {
                    walk(child, ids, wants_error);
                }
            }
            serde_json::Value::Array(items) => {
                for item in items {
                    walk(item, ids, wants_error);
                }
            }
            _ => {}
        }
    }

    let mut ids = std::collections::BTreeSet::new();
    let mut wants_error = false;
    walk(mapping, &mut ids, &mut wants_error);
    (ids, wants_error)
}

/// Helper: resolve path `segments` (see [`reference_segments`]) against a Value.
///
/// Walks them the way the workflow runtime does, so diagnostics agree with
/// runtime resolution: an object segment is always a key lookup (a key named
/// `"0"` included), and an array segment must be an index token, with
/// Python-style negative suffix indexing (`-1` is the last element).
fn resolve_json_path(value: &serde_json::Value, segments: &[String]) -> Option<serde_json::Value> {
    let mut current = value;
    for segment in segments {
        current = match current {
            serde_json::Value::Object(map) => map.get(segment)?,
            serde_json::Value::Array(items) if is_array_index_token(segment) => {
                items.get(array_index(segment, items.len())?)?
            }
            _ => return None,
        };
    }
    Some(current.clone())
}

/// Explain why the `tail` path segments failed to resolve against `value`,
/// telling the one *shape mismatch* the runtime rejects — a named key indexed
/// into an array, the reporter's `steps.split.outputs.result` — apart from the
/// misses it lets through: a missing field, an out-of-range index, or a
/// segment reaching into a scalar or `null`. Mirrors the workflow runtime's
/// `descend`: only the array case fails the run (unless the reference declares
/// a `default`); every other miss quietly resolves to `null` or the default.
///
/// Returns `None` when the path fully resolves (to any value, including a
/// genuine `null`) — a real null leaf is not a mismatch and gets no reason.
/// `base` is the human prefix the tail hangs off (e.g. `steps.split_users`).
fn explain_unresolved_path(
    value: &serde_json::Value,
    base: &str,
    tail: &[String],
) -> Option<String> {
    use serde_json::Value;
    let mut current = value;
    let mut walked = base.to_string();
    for segment in tail {
        let segment = segment.as_str();
        match current {
            Value::Object(map) => match map.get(segment) {
                Some(child) => current = child,
                None => {
                    let fields: Vec<&str> = map.keys().map(String::as_str).collect();
                    return Some(format!(
                        "'{walked}' has no field '{segment}' (available: {})",
                        if fields.is_empty() {
                            "(none)".to_string()
                        } else {
                            fields.join(", ")
                        }
                    ));
                }
            },
            Value::Array(items) if is_array_index_token(segment) => {
                match array_index(segment, items.len()).and_then(|index| items.get(index)) {
                    Some(child) => current = child,
                    None => {
                        return Some(format!(
                            "'{walked}' is an array of length {} — index {segment} is out of range",
                            items.len()
                        ));
                    }
                }
            }
            Value::Array(_) => {
                return Some(format!(
                    "'{walked}' is an array, so '{segment}' is not a valid field — address \
                     elements by numeric index (e.g. '{walked}.0'), or reference '{walked}' \
                     itself for the whole array"
                ));
            }
            scalar => {
                let kind = match scalar {
                    Value::String(_) => "a string",
                    Value::Number(_) => "a number",
                    Value::Bool(_) => "a boolean",
                    Value::Null => "null",
                    _ => "a scalar",
                };
                return Some(format!(
                    "'{walked}' is {kind}, so '{segment}' resolves to null (or the \
                     reference's default) at run time — the runtime does not \
                     traverse into scalars or null"
                ));
            }
        }
        if !walked.is_empty() {
            walked.push('.');
        }
        walked.push_str(segment);
    }
    None
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

/// Where a referencing step sits in the workflow graph, as far as reference
/// resolution cares. The runtime gives every Split/While iteration and every
/// WaitForSignal `onWait` run a source of its own — a Split binds `data` to
/// its current element, each run starts with an empty `steps` map, and a loop
/// binds its own variables — so a reference only resolves the way the runtime
/// does once the tools know what surrounds the step. `StepScope::default()`
/// places nothing (`trace_reference` has no step): references then resolve
/// against the top-level run.
#[derive(Debug, Clone, Default)]
struct StepScope {
    /// The step's scope id, from its step record (`sc_<loop>_<i>…`; absent at
    /// the top level).
    scope_id: Option<String>,
    /// Where the step sits; `None` when it wasn't found in the definition.
    placement: Option<Placement>,
}

#[derive(Debug, Clone, Default)]
struct Placement {
    /// The Split/While steps whose subgraph holds the step, outermost first.
    loops: Vec<EnclosingLoop>,
    /// Whether the step runs in a WaitForSignal `onWait` graph.
    in_on_wait: bool,
    /// Ids of the steps in the step's own graph — the only ones its `steps.*`
    /// can see, since every nested graph run starts with an empty `steps` map
    /// and the top level never sees the steps inside a loop.
    siblings: BTreeSet<String>,
}

#[derive(Debug, Clone)]
struct EnclosingLoop {
    step_id: String,
    is_split: bool,
    /// Names the loop binds for its body on each iteration: its
    /// `config.variables` mapping, plus the subgraph's declared variables.
    variables: BTreeSet<String>,
}

impl Placement {
    fn in_loop(&self) -> bool {
        !self.loops.is_empty()
    }

    fn in_split(&self) -> bool {
        self.loops.iter().any(|enclosing| enclosing.is_split)
    }

    fn in_nested_graph(&self) -> bool {
        self.in_loop() || self.in_on_wait
    }
}

impl StepScope {
    fn new(scope_id: Option<&str>, placement: Option<Placement>) -> Self {
        Self {
            scope_id: scope_id.map(str::to_string),
            placement,
        }
    }

    /// Each enclosing loop's iteration index, outermost first. The runtime
    /// builds the scope id as `sc_<loop>_<i>` and appends `_<loop>_<i>` per
    /// nested loop; reading it against the known loop ids keeps the parse
    /// exact even when an id itself ends in `_<digits>`.
    fn iteration_indices(&self) -> Option<Vec<u64>> {
        let placement = self.placement.as_ref()?;
        if !placement.in_loop() {
            return Some(Vec::new());
        }
        let mut rest = self.scope_id.as_deref()?.strip_prefix("sc")?;
        let mut indices = Vec::with_capacity(placement.loops.len());
        for enclosing in &placement.loops {
            rest = rest
                .strip_prefix('_')?
                .strip_prefix(enclosing.step_id.as_str())?
                .strip_prefix('_')?;
            let end = rest.find('_').unwrap_or(rest.len());
            indices.push(rest[..end].parse().ok()?);
            rest = &rest[end..];
        }
        rest.is_empty().then_some(indices)
    }

    /// The iteration index of the enclosing loop at `position` (outermost
    /// first). The innermost one is also the scope id's trailing segment,
    /// which stands in when the full parse fails.
    fn loop_index(&self, position: usize) -> Option<u64> {
        if let Some(indices) = self.iteration_indices() {
            return indices.get(position).copied();
        }
        let innermost = self.placement.as_ref()?.loops.len().checked_sub(1)?;
        (position == innermost)
            .then(|| loop_index_from_scope_id(self.scope_id.as_deref()?))
            .flatten()
    }
}

/// Find `step_id`'s definition in a fetched workflow — at the top level or in
/// a Split/While `subgraph` or WaitForSignal `onWait` graph, at any depth —
/// together with where it sits.
fn locate_step<'a>(
    workflow: &'a serde_json::Value,
    step_id: &str,
) -> Option<(&'a serde_json::Value, Placement)> {
    let steps = workflow
        .pointer("/data/definition/executionGraph/steps")
        .or_else(|| workflow.pointer("/data/executionGraph/steps"))?;
    let mut placement = Placement::default();
    let definition = locate_in_graph(steps, step_id, &mut placement)?;
    Some((definition, placement))
}

/// Depth-first search of a graph's `steps` map and the graphs nested in it,
/// recording on the way down which containers the step sits in.
fn locate_in_graph<'a>(
    steps: &'a serde_json::Value,
    step_id: &str,
    placement: &mut Placement,
) -> Option<&'a serde_json::Value> {
    let steps = steps.as_object()?;
    if let Some(definition) = steps.get(step_id) {
        placement.siblings = steps.keys().cloned().collect();
        return Some(definition);
    }
    for (id, step) in steps {
        if let Some(subgraph) = step.pointer("/subgraph/steps") {
            let is_split = match step.get("stepType").and_then(|t| t.as_str()) {
                Some("Split") => true,
                Some("While") => false,
                _ => continue,
            };
            placement.loops.push(EnclosingLoop {
                step_id: id.clone(),
                is_split,
                variables: ["/config/variables", "/subgraph/variables"]
                    .into_iter()
                    .filter_map(|pointer| step.pointer(pointer)?.as_object())
                    .flat_map(|variables| variables.keys().cloned())
                    .collect(),
            });
            if let Some(definition) = locate_in_graph(subgraph, step_id, placement) {
                return Some(definition);
            }
            placement.loops.pop();
        }
        if let Some(on_wait) = step.pointer("/onWait/steps") {
            let outer = std::mem::replace(&mut placement.in_on_wait, true);
            if let Some(definition) = locate_in_graph(on_wait, step_id, placement) {
                return Some(definition);
            }
            placement.in_on_wait = outer;
        }
    }
    None
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

/// The step record a `steps.<id>` reference reads for a step at `scope`. Once
/// the step is placed that is the record from its own iteration — the same
/// scope id, both absent at the top level — since each iteration has its own
/// `steps` map; otherwise the newest record of that id.
fn scoped_step_record<'a>(
    summaries: &'a serde_json::Value,
    step_id: &str,
    scope: &StepScope,
) -> Option<&'a serde_json::Value> {
    if scope.placement.is_none() {
        return find_step_in_summaries(summaries, step_id);
    }
    summaries
        .pointer("/data/steps")?
        .as_array()?
        .iter()
        .find(|s| {
            s.get("stepId").and_then(|v| v.as_str()) == Some(step_id)
                && s.get("scopeId").and_then(|v| v.as_str()) == scope.scope_id.as_deref()
        })
}

/// What a reference resolves to for one step, as far as the tools can tell.
#[derive(Debug, Clone, PartialEq)]
enum Resolution {
    /// The value the step sees.
    Found(serde_json::Value),
    /// Nothing there: at run time the reference resolves to null, or to its
    /// declared default. Carries the reason when it isn't plain absence.
    Missing(Option<&'static str>),
    /// There is a value, but only the running workflow has it: it is never
    /// stored, so the tools can't show it.
    RuntimeOnly(&'static str),
}

impl Resolution {
    fn at(value: &serde_json::Value, path: &[String]) -> Self {
        resolve_json_path(value, path).map_or(Self::Missing(None), Self::Found)
    }

    fn found(self) -> Option<serde_json::Value> {
        match self {
            Self::Found(value) => Some(value),
            _ => None,
        }
    }
}

const DATA_IN_SPLIT: &str = "Inside a Split, data is the current element of the nearest Split, \
     not the workflow input. It is never stored, so its value is known only at run time.";
const ITEM_IN_SPLIT: &str = "item is the current element of the nearest Split. It is never \
     stored, so its value is known only at run time.";
const LOOP_OUTPUTS: &str = "loop.outputs is the previous While iteration's output. It is never \
     stored, so its value is known only at run time.";
const INDEX_UNKNOWN: &str = "The iteration indices could not be read from this step's scope id, \
     so they are known only at run time.";
const STEP_OUTSIDE_GRAPH: &str = "Only steps in the referencing step's own graph are visible: \
     every Split/While iteration and onWait run starts with an empty steps map, and the top \
     level never sees the steps inside a loop. This reference resolves to null (or its \
     default) at run time.";
const INTERNAL_VARIABLE: &str = "Set by the runtime for its own bookkeeping and never stored \
     with the instance, so its value is known only at run time.";
const LOOP_VARIABLE: &str = "Bound by an enclosing Split/While on each iteration (its variables \
     mapping), so its value is known only at run time.";
const ITERATION_VARIABLE: &str = "Set by the runtime for each loop iteration and never stored, \
     so its value is known only at run time.";
const SIGNAL_ID_VARIABLE: &str = "Set by the runtime for each onWait run and never stored, so \
     its value is known only at run time.";
const VARIABLES_IN_NESTED_GRAPH: &str = "Inside a Split/While iteration or an onWait graph, \
     variables also holds values the runtime binds for that run, so the whole object is known \
     only at run time. Reference a single variable instead.";

/// Resolve a `steps.<id>.<path>`, `data.<path>`, `variables.<path>`,
/// `workflow.<path>`, `loop.<path>`, `iteration.<path>`, `item.<path>` or
/// bare `__error.<path>`/`error.<path>` reference for the step at `scope`,
/// against step summaries / execution input.
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
fn resolve_reference(
    ref_path: &str,
    summaries: &serde_json::Value,
    execution: &serde_json::Value,
    scope: &StepScope,
) -> Resolution {
    // Tokenized exactly as the runtime reads the path, so a bracketed spelling
    // (`steps['fetch'].outputs`, `data["a.b"]`) resolves like the dotted one.
    let segments = reference_segments(ref_path);
    match segments.split_first() {
        Some((root, rest)) => resolve_root(root, rest, summaries, execution, scope),
        None => Resolution::Missing(None),
    }
}

/// The value [`resolve_reference`] finds, if it finds one.
fn resolve_reference_value(
    ref_path: &str,
    summaries: &serde_json::Value,
    execution: &serde_json::Value,
    scope: &StepScope,
) -> Option<serde_json::Value> {
    resolve_reference(ref_path, summaries, execution, scope).found()
}

/// The per-root half of [`resolve_reference`], over already-tokenized
/// segments, so `workflow.inputs.<root>.*` can hand its tail to the same arm.
fn resolve_root(
    root: &str,
    rest: &[String],
    summaries: &serde_json::Value,
    execution: &serde_json::Value,
    scope: &StepScope,
) -> Resolution {
    use Resolution::{Found, Missing, RuntimeOnly};
    let placement = scope.placement.as_ref();
    match root {
        "steps" => {
            let Some((source_step_id, field_path)) = rest.split_first() else {
                return Missing(None);
            };
            if is_error_alias(source_step_id) {
                // `__error`/`error` aren't real steps — the runtime injects the
                // captured onError envelope under this synthetic id when routing
                // to a failure handler (see `error_steps` in
                // runtara-workflow-stdlib). The MCP tools only see historical
                // step summaries, not which specific failure triggered a given
                // onError edge, so surface the first failed step's error as a
                // best-effort match — the same "primary failure" step
                // `why_execution_failed` already reports.
                return find_error_envelope(summaries).map_or(Missing(None), |envelope| {
                    Resolution::at(&envelope, field_path)
                });
            }
            if placement.is_some_and(|p| !p.siblings.contains(source_step_id.as_str())) {
                return Missing(Some(STEP_OUTSIDE_GRAPH));
            }
            // A step summary's `outputs` field is the full step *envelope*
            // (`{ "outputs": <actual>, "stepId", "stepType", ... }`) — exactly the
            // runtime `steps.<id>` value. Resolve the remainder after the step id
            // (the leading `outputs` segment included) against that envelope, the
            // same way the workflow runtime resolves `steps.<id>.<path>`.
            match scoped_step_record(summaries, source_step_id, scope)
                .and_then(|record| record.get("outputs"))
            {
                Some(envelope) => Resolution::at(envelope, field_path),
                None => Missing(None),
            }
        }
        // The bare onError aliases: `build_source` mirrors `steps.__error` to
        // the source root during onError dispatch, so they read the same
        // envelope as the `steps.__error` arm above.
        "__error" | "error" => find_error_envelope(summaries)
            .map_or(Missing(None), |envelope| Resolution::at(&envelope, rest)),
        // A bare `data` / `variables` is the whole object, as at runtime. A
        // Split binds `data` to its current element, and a While or onWait
        // graph inside it keeps that binding.
        "data" if placement.is_some_and(Placement::in_split) => RuntimeOnly(DATA_IN_SPLIT),
        "data" => instance_data(execution).map_or(Missing(None), |data| Resolution::at(data, rest)),
        "variables" => {
            if let Some(reason) = runtime_only_variable(rest.first().map(String::as_str), placement)
            {
                return RuntimeOnly(reason);
            }
            instance_variables(execution)
                .map_or(Missing(None), |variables| Resolution::at(variables, rest))
        }
        // `build_source` sets `workflow` to `{inputs: {data, variables}}` from
        // the same `data` and `variables` the step sees, so its two subtrees
        // resolve exactly like those roots.
        "workflow" => match rest {
            [inputs, part, tail @ ..]
                if inputs == "inputs" && (part == "data" || part == "variables") =>
            {
                resolve_root(part, tail, summaries, execution, scope)
            }
            [] => workflow_inputs(rest, summaries, execution, scope),
            [inputs] if inputs == "inputs" => workflow_inputs(rest, summaries, execution, scope),
            _ => Missing(None),
        },
        // `loop` is the innermost enclosing While's `{index, outputs}`; a Split
        // keeps its parent's, so outside any While there is none. Its index is
        // recoverable from the scope id, its outputs never are.
        "loop" => {
            let Some(position) =
                placement.and_then(|p| p.loops.iter().rposition(|enclosing| !enclosing.is_split))
            else {
                return Missing(None);
            };
            match rest.split_first() {
                Some((field, tail)) if field == "index" => scope
                    .loop_index(position)
                    .map_or(RuntimeOnly(INDEX_UNKNOWN), |index| {
                        Resolution::at(&json!(index), tail)
                    }),
                Some((field, _)) if field == "outputs" => RuntimeOnly(LOOP_OUTPUTS),
                Some(_) => Missing(None),
                None => RuntimeOnly(LOOP_OUTPUTS),
            }
        }
        // `iteration` is `{index, indices, item}` for the innermost Split/While
        // around the step; `item` is the nearest Split's element (null in a
        // While with no Split around it).
        "iteration" => {
            let Some(placement) = placement.filter(|p| p.in_loop()) else {
                return Missing(None);
            };
            let innermost = placement.loops.len() - 1;
            match rest.split_first() {
                Some((field, tail)) if field == "index" => scope
                    .loop_index(innermost)
                    .map_or(RuntimeOnly(INDEX_UNKNOWN), |index| {
                        Resolution::at(&json!(index), tail)
                    }),
                Some((field, tail)) if field == "indices" => scope
                    .iteration_indices()
                    .map_or(RuntimeOnly(INDEX_UNKNOWN), |indices| {
                        Resolution::at(&json!(indices), tail)
                    }),
                Some((field, _)) if field == "item" && placement.in_split() => {
                    RuntimeOnly(ITEM_IN_SPLIT)
                }
                Some((field, tail)) if field == "item" => {
                    Resolution::at(&serde_json::Value::Null, tail)
                }
                Some(_) => Missing(None),
                None if placement.in_split() => RuntimeOnly(ITEM_IN_SPLIT),
                None => scope
                    .iteration_indices()
                    .map_or(RuntimeOnly(INDEX_UNKNOWN), |indices| {
                        Found(json!({
                            "index": indices.last(),
                            "indices": indices,
                            "item": null,
                        }))
                    }),
            }
        }
        "item" if placement.is_some_and(Placement::in_split) => RuntimeOnly(ITEM_IN_SPLIT),
        _ => Missing(None),
    }
}

/// A bare `workflow` / `workflow.inputs`: known only when both of its halves,
/// the step's `data` and `variables`, are.
fn workflow_inputs(
    rest: &[String],
    summaries: &serde_json::Value,
    execution: &serde_json::Value,
    scope: &StepScope,
) -> Resolution {
    let data = resolve_root("data", &[], summaries, execution, scope);
    let variables = resolve_root("variables", &[], summaries, execution, scope);
    match (data, variables) {
        (Resolution::RuntimeOnly(reason), _) | (_, Resolution::RuntimeOnly(reason)) => {
            Resolution::RuntimeOnly(reason)
        }
        (Resolution::Found(data), variables) => {
            let workflow = json!({
                "inputs": {
                    "data": data,
                    "variables": variables.found().unwrap_or_else(|| json!({})),
                }
            });
            Resolution::at(&workflow, rest)
        }
        _ => Resolution::Missing(None),
    }
}

/// Why `variables.<name>` (or a bare `variables` when `name` is `None`) has a
/// value only at run time for a step at `placement`, if it does.
/// `with_runtime_variables` rebuilds the rest: declared defaults, stored
/// input, and the runtime's `_instance_id` / `_tenant_id` / `_workflow_id`.
fn runtime_only_variable(
    name: Option<&str>,
    placement: Option<&Placement>,
) -> Option<&'static str> {
    let Some(name) = name else {
        return placement
            .is_some_and(Placement::in_nested_graph)
            .then_some(VARIABLES_IN_NESTED_GRAPH);
    };
    let in_loop = placement.is_some_and(Placement::in_loop);
    let in_split = placement.is_some_and(Placement::in_split);
    let in_while = placement.is_some_and(|p| p.loops.iter().any(|l| !l.is_split));
    let bound_by_loop =
        placement.is_some_and(|p| p.loops.iter().any(|l| l.variables.contains(name)));
    match name {
        "_durable_key_version" | "_loop_path" | "_manifest_graph_path" => Some(INTERNAL_VARIABLE),
        _ if bound_by_loop => Some(LOOP_VARIABLE),
        "_scope_id" | "_parent_scope_id" | "_loop_indices" | "_index" if in_loop => {
            Some(ITERATION_VARIABLE)
        }
        "_item" if in_split => Some(ITERATION_VARIABLE),
        "_loop" | "_previousOutputs" if in_while => Some(ITERATION_VARIABLE),
        "_signal_id" if placement.is_some_and(|p| p.in_on_wait) => Some(SIGNAL_ID_VARIABLE),
        _ => None,
    }
}

/// The workflow input the runtime binds to `data`: the `data` half of the
/// stored `{data, variables}` envelope, or the whole input for an invocation
/// that wasn't wrapped in one.
fn instance_data(execution: &serde_json::Value) -> Option<&serde_json::Value> {
    execution
        .pointer("/data/inputs/data")
        .or_else(|| execution.pointer("/data/inputs"))
}

/// The pointer to the instance's variables in its execution record: inside
/// the `{data, variables}` input envelope when there is one, else beside it.
fn instance_variables_pointer(execution: &serde_json::Value) -> &'static str {
    if execution.pointer("/data/inputs/variables").is_some()
        || execution.pointer("/data/inputs/data").is_some()
    {
        "/data/inputs/variables"
    } else {
        "/data/variables"
    }
}

/// The instance's variables as stored with its execution record. Run them
/// through [`with_runtime_variables`] first to see the runtime's `variables`.
fn instance_variables(execution: &serde_json::Value) -> Option<&serde_json::Value> {
    execution.pointer(instance_variables_pointer(execution))
}

/// True for the synthetic onError step id / bare root (`__error`, `error`).
fn is_error_alias(segment: &str) -> bool {
    segment == "__error" || segment == "error"
}

/// The workflow's declared variables with their default values, flattened the
/// way the compiler bakes them into the workflow (`{name: {type, value}}`
/// becomes `{name: value}`); every run's `variables` starts from these.
fn declared_variables(workflow: &serde_json::Value) -> serde_json::Map<String, serde_json::Value> {
    let Some(declared) = workflow
        .pointer("/data/definition/executionGraph/variables")
        .or_else(|| workflow.pointer("/data/executionGraph/variables"))
        .and_then(|variables| variables.as_object())
    else {
        return serde_json::Map::new();
    };
    declared
        .iter()
        .map(|(name, declaration)| {
            let value = match declaration {
                serde_json::Value::Object(fields)
                    if fields.contains_key("type") && fields.contains_key("value") =>
                {
                    fields["value"].clone()
                }
                other => other.clone(),
            };
            (name.clone(), value)
        })
        .collect()
}

/// The instance record with its variables replaced by the set the runtime
/// resolves a top-level `variables.*` against, as far as the tools can rebuild
/// it: the declared defaults from `workflow` (the definition the instance
/// ran), the stored input variables over them — minus `_`-prefixed ones but
/// `_cache_key_prefix`, which `build_source` drops — and the `_instance_id`,
/// `_tenant_id` and `_workflow_id` the runtime adds (this instance's id, the
/// server's tenant, the record's workflow id). What the runtime keeps for its
/// own bookkeeping, or binds per loop iteration, stays out (see
/// [`runtime_only_variable`]).
fn with_runtime_variables(
    mut execution: serde_json::Value,
    workflow: Option<&serde_json::Value>,
    instance_id: &str,
    workflow_id: &str,
    tenant_id: &str,
) -> serde_json::Value {
    let workflow_id = execution
        .pointer("/data/workflowId")
        .and_then(|v| v.as_str())
        .unwrap_or(workflow_id)
        .to_string();
    let pointer = instance_variables_pointer(&execution);
    let mut variables = workflow.map(declared_variables).unwrap_or_default();
    if let Some(serde_json::Value::Object(stored)) =
        execution.pointer_mut(pointer).map(serde_json::Value::take)
    {
        variables.extend(
            stored
                .into_iter()
                .filter(|(name, _)| !name.starts_with('_') || name == "_cache_key_prefix"),
        );
    }
    variables.insert("_instance_id".into(), json!(instance_id));
    variables.insert("_tenant_id".into(), json!(tenant_id));
    variables.insert("_workflow_id".into(), json!(workflow_id));

    let (parent, _) = pointer.rsplit_once('/').unwrap_or_default();
    if let Some(parent) = execution
        .pointer_mut(parent)
        .and_then(|parent| parent.as_object_mut())
    {
        parent.insert("variables".into(), serde_json::Value::Object(variables));
    }
    execution
}

/// Recover the innermost iteration index the runtime encoded into a Split/While
/// scope id (`sc_<stepId>_<index>` at the top level, `<parentScope>_<stepId>_<index>`
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

/// What a reference envelope (`{valueType: "reference", value, default?}`)
/// hands the step, applying its `default` the way the runtime's
/// `resolve_lookup` does: a found `null` or a miss falls back to it. `None`
/// when the value isn't known here — missing with no default, or runtime-only,
/// where a default would only stand in for a value the runtime does have.
fn resolve_envelope_reference(
    envelope: &serde_json::Value,
    ref_path: &str,
    summaries: &serde_json::Value,
    execution: &serde_json::Value,
    scope: &StepScope,
) -> Option<serde_json::Value> {
    let default = envelope.get("default").cloned();
    match resolve_reference(ref_path, summaries, execution, scope) {
        Resolution::Found(serde_json::Value::Null) => {
            Some(default.unwrap_or(serde_json::Value::Null))
        }
        Resolution::Found(value) => Some(value),
        Resolution::Missing(_) => default,
        Resolution::RuntimeOnly(_) => None,
    }
}

fn resolve_nested_reference_envelopes(
    value: &serde_json::Value,
    summaries: &serde_json::Value,
    execution: &serde_json::Value,
    scope: &StepScope,
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
                                        scope,
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
                                scope,
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
                                        scope,
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
                                scope,
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
                let resolved =
                    resolve_envelope_reference(value, ref_path, summaries, execution, scope);
                if let Some(resolved) = resolved {
                    return json!({
                        "valueType": "immediate",
                        "value": resolve_nested_reference_envelopes(
                            &resolved,
                            summaries,
                            execution,
                            scope,
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
                                scope,
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
                        scope,
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
    scope: &StepScope,
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
                            scope,
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
                    resolve_mapping_envelope(item, summaries, execution, scope, unresolved_refs)
                })
                .collect(),
        ),
        // A composite payload should be an object/array, but degrade gracefully:
        // resolve any embedded reference envelopes rather than erroring.
        other => {
            resolve_nested_reference_envelopes(other, summaries, execution, scope, unresolved_refs)
        }
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
    scope: &StepScope,
    unresolved_refs: &mut Vec<String>,
) -> serde_json::Value {
    match envelope.get("valueType").and_then(|v| v.as_str()) {
        Some("reference") => {
            let path = envelope
                .get("value")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            match resolve_envelope_reference(envelope, path, summaries, execution, scope) {
                Some(resolved) => resolve_nested_reference_envelopes(
                    &resolved,
                    summaries,
                    execution,
                    scope,
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
            resolve_nested_reference_envelopes(&inner, summaries, execution, scope, unresolved_refs)
        }
        Some("composite") => {
            let inner = envelope
                .get("value")
                .cloned()
                .unwrap_or(serde_json::Value::Null);
            resolve_composite_payload(&inner, summaries, execution, scope, unresolved_refs)
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
            scope,
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
    is_reference_envelope(value) && !is_workflow_reference(path)
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
            // Kept identical to runtara-core's own envelope fallback
            // (`persistence::common::row::error_from_output_envelope`). Core's
            // value wins where both run, so a divergence here would surface as
            // one failure reported two different ways.
            .unwrap_or_else(|| json!("Output reported _error=true")),
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

/// Record the references a nested mapping left unresolved on its
/// `resolve_input_mappings` entry, keeping the ones that have a value only at
/// run time (see [`Resolution::RuntimeOnly`]) apart from the ones that are
/// simply missing.
fn note_unresolved_references(
    entry: &mut serde_json::Value,
    unresolved_refs: Vec<String>,
    summaries: &serde_json::Value,
    execution: &serde_json::Value,
    scope: &StepScope,
) {
    if unresolved_refs.is_empty() {
        return;
    }
    let mut missing = Vec::new();
    let mut runtime_only = Vec::new();
    for reference in unresolved_refs {
        match resolve_reference(&reference, summaries, execution, scope) {
            Resolution::RuntimeOnly(reason) => {
                runtime_only.push(json!({ "reference": reference, "reason": reason }))
            }
            _ => missing.push(reference),
        }
    }
    entry["resolutionNote"] = json!(RUNTIME_NESTED_REFERENCE_NOTE);
    if !missing.is_empty() {
        entry["unresolvedNestedReferences"] = json!(missing);
    }
    if !runtime_only.is_empty() {
        entry["runtimeOnlyReferences"] = json!(runtime_only);
    }
}

/// Helper: resolve inputMapping references against step summaries. `scope`
/// places the step this input mapping belongs to (see [`StepScope`]).
fn resolve_input_mappings(
    input_mapping: &serde_json::Value,
    summaries: &serde_json::Value,
    execution: &serde_json::Value,
    scope: &StepScope,
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
                    // Tokenized as the runtime reads it, like the shared
                    // resolver, so a bracketed root or step id
                    // (`steps['fetch'].outputs`) takes the same arm.
                    let segments = reference_segments(ref_path);
                    // A source step with no record or no outputs yet gets no
                    // `resolvedValue` at all rather than a null.
                    let mut has_value = true;
                    match segments.first().map(String::as_str).unwrap_or("") {
                        "steps" if segments.len() > 1 => {
                            let source_step_id = segments[1].as_str();
                            if is_error_alias(source_step_id) {
                                // Not a real step — see the matching special-case
                                // in resolve_root for why `__error` never shows
                                // up as a step record.
                                entry["source"] = json!("error_context");
                            } else {
                                entry["sourceStep"] = json!(source_step_id);
                                // The record the resolver reads: once the step
                                // is placed, the one from its own iteration.
                                match scoped_step_record(summaries, source_step_id, scope) {
                                    Some(source) => {
                                        entry["sourceStatus"] = source
                                            .get("status")
                                            .cloned()
                                            .unwrap_or(json!("unknown"));
                                        has_value = source.get("outputs").is_some();
                                    }
                                    None => {
                                        entry["sourceStatus"] = json!("not_found");
                                        has_value = false;
                                    }
                                }
                            }
                        }
                        "__error" | "error" => entry["source"] = json!("error_context"),
                        "data" | "workflow" => entry["source"] = json!("workflow_input"),
                        "variables" => {
                            entry["source"] = json!("variable");
                            if let Some(name) = segments.get(1) {
                                entry["variableName"] = json!(name);
                            }
                        }
                        "loop" => entry["source"] = json!("loop"),
                        "iteration" | "item" => entry["source"] = json!("iteration"),
                        _ => {}
                    }
                    // Every root's value comes from the shared resolver, so
                    // this can never diverge from the runtime (see
                    // resolve_reference).
                    match resolve_reference(ref_path, summaries, execution, scope) {
                        Resolution::Found(resolved) => {
                            if has_value {
                                entry["resolvedValue"] = resolved;
                            }
                        }
                        Resolution::Missing(reason) => {
                            if has_value {
                                entry["resolvedValue"] = json!(null);
                            }
                            if let Some(reason) = reason {
                                entry["resolutionNote"] = json!(reason);
                            }
                        }
                        Resolution::RuntimeOnly(reason) => {
                            entry["resolvedValue"] = json!(null);
                            entry["runtimeOnly"] = json!(true);
                            entry["resolutionNote"] = json!(reason);
                        }
                    }
                }
            }
            "immediate" => {
                let mut unresolved_refs = Vec::new();
                let resolved_value = resolve_nested_reference_envelopes(
                    &value,
                    summaries,
                    execution,
                    scope,
                    &mut unresolved_refs,
                );
                entry["resolvedValue"] = resolved_value;
                note_unresolved_references(
                    &mut entry,
                    unresolved_refs,
                    summaries,
                    execution,
                    scope,
                );
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
                    scope,
                    &mut unresolved_refs,
                );
                entry["resolvedValue"] = resolved_value;
                note_unresolved_references(
                    &mut entry,
                    unresolved_refs,
                    summaries,
                    execution,
                    scope,
                );
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
                    scope,
                    &mut unresolved_refs,
                );
                entry["resolvedValue"] = resolved_value;
                note_unresolved_references(
                    &mut entry,
                    unresolved_refs,
                    summaries,
                    execution,
                    scope,
                );
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

    // Fetch the execution and the definition it ran first: the target's
    // inputMapping decides which other steps are needed for reference
    // resolution, so it must be known before any step payloads are fetched.
    let (execution, workflow) =
        fetch_execution_for_references(server, &params.workflow_id, &params.instance_id).await?;

    // Extract the step's definition — at any depth — and where it sits.
    let located = locate_step(&workflow, &params.step_id);
    let input_mapping = located
        .as_ref()
        .and_then(|(step, _)| step.get("inputMapping"))
        .cloned()
        .unwrap_or(json!({}));

    // The target step is fetched alone with `limit: 1` (newest record — the
    // same one a full listing would resolve to), so a loop-heavy instance can
    // neither push it out of a capped page nor drag unrelated payloads along.
    let target_page = fetch_step_summaries(
        server,
        &params.workflow_id,
        &params.instance_id,
        &StepSummariesFetch {
            step_ids: std::slice::from_ref(&params.step_id),
            status: None,
            limit: 1,
        },
    )
    .await?;
    let target = steps_from_summaries(&target_page)
        .into_iter()
        .next()
        .ok_or_else(|| {
            rmcp::ErrorData::internal_error(
                format!(
                    "Step '{}' not found in execution {}",
                    params.step_id, params.instance_id
                ),
                None,
            )
        })?;

    // Steps referenced by the mapping are fetched as one batch; anything the
    // mapping doesn't reference never crosses the wire.
    let (mut ref_ids, wants_error) = referenced_step_ids(&input_mapping);
    ref_ids.remove(&params.step_id);
    let mut steps = vec![target.clone()];
    let mut referenced_truncation = None;
    if !ref_ids.is_empty() {
        let ref_ids: Vec<String> = ref_ids.into_iter().collect();
        let refs_page = fetch_step_summaries(
            server,
            &params.workflow_id,
            &params.instance_id,
            &StepSummariesFetch {
                step_ids: &ref_ids,
                status: None,
                limit: STEP_SUMMARY_FETCH_LIMIT,
            },
        )
        .await?;
        let ref_steps = steps_from_summaries(&refs_page);
        referenced_truncation =
            truncation_flag(ref_steps.len(), total_count_from_summaries(&refs_page));
        steps.extend(ref_steps);
    }
    if wants_error {
        // `steps.__error` resolves to the newest failed step's error envelope.
        let failed_page = fetch_step_summaries(
            server,
            &params.workflow_id,
            &params.instance_id,
            &StepSummariesFetch {
                step_ids: &[],
                status: Some("failed"),
                limit: 1,
            },
        )
        .await?;
        steps.extend(steps_from_summaries(&failed_page));
    }
    let summaries = synthetic_summaries(steps);

    let scope = StepScope::new(
        target.get("scopeId").and_then(|v| v.as_str()),
        located.map(|(_, placement)| placement),
    );
    let resolved_inputs = resolve_input_mappings(&input_mapping, &summaries, &execution, &scope);

    let mut response = json!({
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
    if let Some(truncated) = referenced_truncation {
        response["referencedStepsTruncated"] = truncated;
    }

    json_result(response)
}

/// Build the `trace_reference` response for a `variables.*` reference from a
/// fetched instance execution record (already through
/// [`with_runtime_variables`]). Split out of the tool arm so the resolution
/// semantics stay unit-testable: the value must come from the instance's
/// runtime variable state via the shared resolver — its stored values over
/// the declared defaults, as at runtime — never from the defaults alone.
fn trace_variables_response(reference: &str, execution: &serde_json::Value) -> serde_json::Value {
    let resolution = resolve_reference(
        reference,
        &synthetic_summaries(Vec::new()),
        execution,
        &StepScope::default(),
    );

    // Same lookup as the resolver's variables arm, surfaced whole for context
    // alongside the resolved value.
    let variables = instance_variables(execution).cloned().unwrap_or(json!({}));

    trace_response(
        reference,
        resolution,
        json!({
            "type": "variable",
            "allVariables": variables,
            "note": "allVariables is the instance's variables as a top-level step sees them: \
                     the declared defaults, the values the run was started with, and the \
                     _instance_id, _tenant_id and _workflow_id the runtime adds. Variables the \
                     runtime keeps for its own bookkeeping (e.g. _durable_key_version, \
                     _loop_path, _manifest_graph_path) are never stored and are not shown.",
        }),
    )
}

/// A `trace_reference` response for `resolution`: the value when there is one,
/// and `runtimeOnly` + the reason, or just the reason, when there isn't.
fn trace_response(
    reference: &str,
    resolution: Resolution,
    source: serde_json::Value,
) -> serde_json::Value {
    let mut response = json!({
        "reference": reference,
        "resolved": false,
        "value": null,
        "source": source,
    });
    match resolution {
        Resolution::Found(value) => {
            response["resolved"] = json!(!value.is_null());
            response["value"] = value;
        }
        Resolution::Missing(reason) => {
            if let Some(reason) = reason {
                response["reason"] = json!(reason);
            }
        }
        Resolution::RuntimeOnly(reason) => {
            response["runtimeOnly"] = json!(true);
            response["reason"] = json!(reason);
        }
    }
    response
}

/// The `trace_reference` response for a `loop`/`iteration`/`item` reference.
/// These resolve per loop iteration and `trace_reference` has no step to place
/// them in, so nothing is fetched: the response points at `inspect_step`,
/// which resolves them for a given step (or says why it can't).
fn trace_iteration_response(reference: &str, root: &str) -> serde_json::Value {
    json!({
        "reference": reference,
        "resolved": false,
        "value": null,
        "source": {
            "type": if root == "loop" { "loop_context" } else { "iteration" },
        },
        "reason": "loop, iteration and item resolve per loop iteration, and trace_reference \
                   has no step to place them in. Use inspect_step on a step inside the loop.",
    })
}

/// Fetch an instance record. `full` asks for large inputs unelided: a
/// reference may point into one (the MCP response is re-truncated
/// downstream, so the wire stays bounded).
async fn fetch_instance(
    server: &SmoMcpServer,
    instance_id: &str,
    full: bool,
) -> Result<serde_json::Value, rmcp::ErrorData> {
    let query = if full { "?full=true" } else { "" };
    api_get(
        server,
        &format!("/api/runtime/workflows/instances/{instance_id}{query}"),
    )
    .await
}

/// Fetch the workflow definition `execution` ran — the version recorded on it
/// (`usedVersion`), whose step mappings and declared variable defaults are the
/// ones that applied — or the latest when the record names none.
async fn fetch_definition_for(
    server: &SmoMcpServer,
    workflow_id: &str,
    execution: &serde_json::Value,
) -> Result<serde_json::Value, rmcp::ErrorData> {
    let version = execution
        .pointer("/data/usedVersion")
        .and_then(|v| v.as_i64())
        .filter(|version| *version > 0);
    let path = match version {
        Some(version) => format!("/api/runtime/workflows/{workflow_id}?versionNumber={version}"),
        None => format!("/api/runtime/workflows/{workflow_id}"),
    };
    api_get(server, &path).await
}

/// Fetch what reference resolution needs: the instance record in full, with
/// its variables as a top-level step sees them ([`with_runtime_variables`]),
/// and the definition it ran.
async fn fetch_execution_for_references(
    server: &SmoMcpServer,
    workflow_id: &str,
    instance_id: &str,
) -> Result<(serde_json::Value, serde_json::Value), rmcp::ErrorData> {
    let execution = fetch_instance(server, instance_id, true).await?;
    let workflow = fetch_definition_for(server, workflow_id, &execution).await?;
    let execution = with_runtime_variables(
        execution,
        Some(&workflow),
        instance_id,
        workflow_id,
        &server.tenant_id,
    );
    Ok((execution, workflow))
}

/// Trace an onError reference — `steps.__error.*` or its bare `__error.*` /
/// `error.*` alias. Not a real step: the runtime injects the captured onError
/// envelope under this synthetic id (see `error_steps` in
/// runtara-workflow-stdlib). It resolves to the newest failed step's error
/// envelope, so only that one record is fetched. `alias` is the `__error` /
/// `error` segment as written.
async fn trace_error_context(
    server: &SmoMcpServer,
    params: &TraceReferenceParams,
    alias: &str,
) -> Result<CallToolResult, rmcp::ErrorData> {
    let failed_page = fetch_step_summaries(
        server,
        &params.workflow_id,
        &params.instance_id,
        &StepSummariesFetch {
            step_ids: &[],
            status: Some("failed"),
            limit: 1,
        },
    )
    .await?;
    let summaries = synthetic_summaries(steps_from_summaries(&failed_page));
    let resolution = resolve_reference(
        &params.reference,
        &summaries,
        &serde_json::Value::Null,
        &StepScope::default(),
    );

    json_result(trace_response(
        &params.reference,
        resolution,
        json!({
            "type": "error_context",
            "stepId": alias,
        }),
    ))
}

pub async fn trace_reference(
    server: &SmoMcpServer,
    params: TraceReferenceParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    validate_path_param("workflow_id", &params.workflow_id)?;
    validate_path_param("instance_id", &params.instance_id)?;

    // Tokenized exactly as the runtime reads the path, so a bracketed spelling
    // (`steps['fetch'].outputs.q`, `data["a.b"]`) traces like the dotted one.
    let segments = reference_segments(&params.reference);
    let Some((root, rest)) = segments.split_first() else {
        return Err(rmcp::ErrorData::invalid_params(
            "Reference path must not be empty".to_string(),
            None,
        ));
    };

    match root.as_str() {
        "steps" => {
            let Some((step_id, tail)) = rest.split_first() else {
                return Err(rmcp::ErrorData::invalid_params(
                    "Step reference must be 'steps.<stepId>[.outputs.<field>]'".to_string(),
                    None,
                ));
            };
            let step_id = step_id.as_str();

            if is_error_alias(step_id) {
                return trace_error_context(server, &params, step_id).await;
            }

            // Only the referenced step's newest record is needed to resolve
            // the path — fetching it alone keeps this bounded on loop-heavy
            // instances.
            let step_page = fetch_step_summaries(
                server,
                &params.workflow_id,
                &params.instance_id,
                &StepSummariesFetch {
                    step_ids: &[step_id.to_string()],
                    status: None,
                    limit: 1,
                },
            )
            .await?;
            let summaries = synthetic_summaries(steps_from_summaries(&step_page));

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
                &StepScope::default(),
            )
            .unwrap_or(json!(null));

            let mut response = json!({
                "reference": params.reference,
                "resolved": !resolved.is_null(),
                "value": resolved,
                "source": {
                    "type": "step_output",
                    "stepId": step_id,
                    "stepStatus": step.get("status"),
                    "fullOutputs": outputs,
                }
            });
            // When the path didn't resolve, say WHY instead of just `null`: a
            // named key indexed into an array (e.g. `steps.split.outputs.result`)
            // is a shape mismatch that fails the run (and preflight), so the
            // diagnostic must not imply the value is simply absent; a missing
            // field, an out-of-range index or a scalar traversal resolves to
            // null (or the default) at run time, and the reason says so.
            // `tail` is the path after `steps.<id>`.
            if resolved.is_null()
                && !tail.is_empty()
                && let Some(reason) =
                    explain_unresolved_path(&outputs, &format!("steps.{step_id}"), tail)
            {
                response["reason"] = json!(reason);
            }
            json_result(response)
        }
        // The bare onError aliases read the same envelope as `steps.__error`.
        "__error" | "error" => trace_error_context(server, &params, root).await,
        // These resolve per loop iteration, and `trace_reference` has no
        // step_id param to place one in: point at inspect_step, which
        // resolves them for a given step, instead of rejecting the root.
        "loop" | "iteration" | "item" => {
            json_result(trace_iteration_response(&params.reference, root))
        }
        "data" => {
            let execution = fetch_instance(server, &params.instance_id, true).await?;

            let inputs = instance_data(&execution).cloned().unwrap_or(json!(null));

            let resolved = resolve_json_path(&inputs, rest).unwrap_or(json!(null));

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
            // Workflow variables are per-instance runtime state, not the
            // static defaults in the definition graph — an instance launched
            // with variable overrides diverges from the definition
            // immediately. Resolve against the instance record through the
            // shared resolver so this can never disagree with inspect_step or
            // the runtime (see resolve_reference).
            let (execution, _) =
                fetch_execution_for_references(server, &params.workflow_id, &params.instance_id)
                    .await?;

            json_result(trace_variables_response(&params.reference, &execution))
        }
        "workflow" => {
            // `workflow.inputs.data` / `.variables` are the same values as the
            // `data` / `variables` roots; the shared resolver hands them to
            // those arms.
            let (execution, _) =
                fetch_execution_for_references(server, &params.workflow_id, &params.instance_id)
                    .await?;
            let resolution = resolve_reference(
                &params.reference,
                &synthetic_summaries(Vec::new()),
                &execution,
                &StepScope::default(),
            );

            json_result(trace_response(
                &params.reference,
                resolution,
                json!({ "type": "workflow_input" }),
            ))
        }
        _ => Err(rmcp::ErrorData::invalid_params(
            format!(
                "Unknown reference root '{root}'. Must be 'steps', 'data', 'variables', \
                 'workflow', 'loop', 'iteration', 'item', or the onError alias '__error' / \
                 'error'."
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

    // Fetch execution status — in full, since the failing step's references
    // may point into a large input field the default detail fetch elides.
    let mut execution = fetch_instance(server, &params.instance_id, true).await?;

    let status = execution
        .pointer("/data/status")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();

    // Three targeted fetches replace one full-payload listing: the failed
    // page carries the primary failing step's payload, the running page the
    // in-flight steps, and a count-only fetch the exact record total — each
    // `totalCount` is exact beyond the page limit, so the summary can no
    // longer be silently distorted by a capped listing.
    let failed_page = fetch_step_summaries(
        server,
        &params.workflow_id,
        &params.instance_id,
        &StepSummariesFetch {
            step_ids: &[],
            status: Some("failed"),
            limit: 1,
        },
    )
    .await?;
    let failed_steps = steps_from_summaries(&failed_page);
    let failed_total = total_count_from_summaries(&failed_page);

    let running_page = fetch_step_summaries(
        server,
        &params.workflow_id,
        &params.instance_id,
        &StepSummariesFetch {
            step_ids: &[],
            status: Some("running"),
            limit: IN_FLIGHT_FETCH_LIMIT,
        },
    )
    .await?;
    let running_steps = steps_from_summaries(&running_page);
    let running_total = total_count_from_summaries(&running_page);

    let total_page = fetch_step_summaries(
        server,
        &params.workflow_id,
        &params.instance_id,
        &StepSummariesFetch {
            step_ids: &[],
            status: None,
            limit: 0,
        },
    )
    .await?;
    let total_steps = total_count_from_summaries(&total_page);

    if status != "failed" && failed_total == 0 {
        return json_result(json!({
            "execution": {
                "instanceId": params.instance_id,
                "status": status,
            },
            "message": format!("Execution is not failed (status: {})", status),
        }));
    }

    // Every step record is exactly one of running/completed/failed, so the
    // completed count follows from the other two without a fourth fetch.
    let completed_total = total_steps.saturating_sub(failed_total + running_total);

    // Build failure diagnosis for the primary (newest) failing step
    let mut in_flight_truncation = None;
    let failing_step = if let Some(first_failed) = failed_steps.first() {
        let step_id = first_failed
            .get("stepId")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        // Try to resolve inputs for the failing step, against the definition
        // the instance ran and its variables as a top-level step sees them.
        let workflow = fetch_definition_for(server, &params.workflow_id, &execution)
            .await
            .ok();
        execution = with_runtime_variables(
            std::mem::take(&mut execution),
            workflow.as_ref(),
            &params.instance_id,
            &params.workflow_id,
            &server.tenant_id,
        );

        let located = workflow
            .as_ref()
            .and_then(|workflow| locate_step(workflow, step_id));
        let input_mapping = located
            .as_ref()
            .and_then(|(step, _)| step.get("inputMapping"))
            .cloned()
            .unwrap_or(json!({}));

        // Fetch only the steps the failing step's mapping references; its
        // own record already carries the error, so a `steps.__error`
        // reference resolves against it without another fetch.
        let (mut ref_ids, _wants_error) = referenced_step_ids(&input_mapping);
        ref_ids.remove(step_id);
        let mut steps = vec![first_failed.clone()];
        if !ref_ids.is_empty() {
            let ref_ids: Vec<String> = ref_ids.into_iter().collect();
            let refs_page = fetch_step_summaries(
                server,
                &params.workflow_id,
                &params.instance_id,
                &StepSummariesFetch {
                    step_ids: &ref_ids,
                    status: None,
                    limit: STEP_SUMMARY_FETCH_LIMIT,
                },
            )
            .await?;
            steps.extend(steps_from_summaries(&refs_page));
        }
        let summaries = synthetic_summaries(steps);

        let scope = StepScope::new(
            first_failed.get("scopeId").and_then(|v| v.as_str()),
            located.map(|(_, placement)| placement),
        );
        let resolved_inputs =
            resolve_input_mappings(&input_mapping, &summaries, &execution, &scope);

        json!({
            "stepId": first_failed.get("stepId"),
            "stepName": first_failed.get("stepName"),
            "stepType": first_failed.get("stepType"),
            "status": effective_step_status(first_failed).unwrap_or("unknown"),
            "error": step_error(first_failed),
            "durationMs": first_failed.get("durationMs"),
            "resolvedInputs": truncate_large_strings(&resolved_inputs),
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
        in_flight_truncation = truncation_flag(in_flight.len(), running_total);
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

    let mut response = json!({
        "execution": {
            "instanceId": params.instance_id,
            "status": status,
            "error": execution.pointer("/data/error"),
        },
        "failingStep": failing_step,
        "executionSummary": {
            "totalSteps": total_steps,
            "completed": completed_total,
            "failed": failed_total,
            "running": running_total,
        },
    });
    if let Some(truncated) = in_flight_truncation {
        response["inFlightStepsTruncated"] = truncated;
    }
    json_result(response)
}

#[cfg(test)]
mod tests {
    use super::*;
    use schemars::JsonSchema;
    use serde_json::json;

    /// A reference path tokenized as the runtime reads it.
    fn segments(path: &str) -> Vec<String> {
        reference_segments(path)
    }

    /// No step to place: references resolve against the top-level run.
    fn top() -> StepScope {
        StepScope::default()
    }

    /// A step at `scope_id` inside `loops` (`(step id, is Split)`, outermost
    /// first), sharing its graph with `siblings`.
    fn in_loops(scope_id: &str, loops: &[(&str, bool)], siblings: &[&str]) -> StepScope {
        StepScope::new(
            Some(scope_id),
            Some(Placement {
                loops: loops
                    .iter()
                    .map(|(step_id, is_split)| EnclosingLoop {
                        step_id: step_id.to_string(),
                        is_split: *is_split,
                        variables: BTreeSet::new(),
                    })
                    .collect(),
                in_on_wait: false,
                siblings: siblings.iter().map(|id| id.to_string()).collect(),
            }),
        )
    }

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

        assert_eq!(
            resolve_json_path(&value, &segments("items.-1")),
            Some(json!("c"))
        );
        assert_eq!(
            resolve_json_path(&value, &segments("items.-3")),
            Some(json!("a"))
        );
        assert_eq!(
            resolve_json_path(&value, &segments("items.0")),
            Some(json!("a"))
        );
        assert_eq!(resolve_json_path(&value, &segments("items.-4")), None);
        assert_eq!(resolve_json_path(&value, &segments("items.5")), None);
    }

    /// trace_reference must explain a shape mismatch (the reporter's
    /// `steps.split.outputs.result`) instead of implying the value is absent.
    #[test]
    fn explain_unresolved_path_distinguishes_mismatch_from_missing() {
        // A Split step envelope: `outputs` is the collected array.
        let envelope = json!({
            "stepId": "split_users",
            "stepType": "Split",
            "outputs": [{"id": 1}, {"id": 2}],
        });

        // Named key into the array -> shape mismatch, with a numeric-index hint.
        let reason =
            explain_unresolved_path(&envelope, "steps.split_users", &segments("outputs.result"))
                .expect("named key into array must produce a reason");
        assert!(
            reason.contains("is an array")
                && reason.contains("'result'")
                && reason.contains("numeric index"),
            "unhelpful reason: {reason}"
        );

        // Unknown top-level field -> missing (not a mismatch), lists available.
        let reason = explain_unresolved_path(&envelope, "steps.split_users", &segments("bogus"))
            .expect("missing field must produce a reason");
        assert!(reason.contains("has no field 'bogus'"), "reason: {reason}");

        // Traversing into a scalar -> not a failure: the runtime's `descend`
        // treats it as an absent value, so the reason must say it resolves to
        // null rather than claim the run fails.
        let reason =
            explain_unresolved_path(&envelope, "steps.split_users", &segments("stepType.first"))
                .expect("scalar traversal must produce a reason");
        assert!(
            reason.contains("is a string") && reason.contains("resolves to null"),
            "reason: {reason}"
        );
        assert!(!reason.contains("cannot be traversed"), "reason: {reason}");

        // A path that fully resolves (to the array, or into an element) yields no
        // reason — a real value, including a genuine null leaf, is not a mismatch.
        assert!(
            explain_unresolved_path(&envelope, "steps.split_users", &segments("outputs")).is_none()
        );
        assert!(
            explain_unresolved_path(&envelope, "steps.split_users", &segments("outputs.0.id"))
                .is_none()
        );
    }

    /// The referenced-id walk decides which steps' payloads are fetched at
    /// all, so it must find `steps.<id>` references in every envelope
    /// position a mapping can hide one: direct references, composite
    /// payloads, condition arguments, fn-call arguments, and envelopes
    /// nested inside immediate values.
    #[test]
    fn referenced_step_ids_finds_references_in_all_mapping_shapes() {
        let mapping = json!({
            "direct": { "valueType": "reference", "value": "steps.fetch.outputs.items" },
            "composite": {
                "valueType": "composite",
                "value": {
                    "nested": { "valueType": "reference", "value": "steps.enrich.outputs" },
                    "list": [
                        { "valueType": "reference", "value": "steps.split_users.outputs.0" }
                    ],
                }
            },
            "condition": {
                "op": "EQ",
                "arguments": [
                    { "valueType": "reference", "value": "steps.check.outputs.flag" },
                    { "valueType": "immediate", "value": true },
                ]
            },
            "fnCall": {
                "fn": "concat",
                "arguments": [
                    { "valueType": "reference", "value": "steps.build.outputs.name" }
                ]
            },
            "immediateWithEnvelope": {
                "valueType": "immediate",
                "value": { "valueType": "reference", "value": "steps.inner.outputs" }
            },
            "notAStep": { "valueType": "reference", "value": "data.orders.0" },
            "errorRef": { "valueType": "reference", "value": "steps.__error.message" },
        });

        let (ids, wants_error) = referenced_step_ids(&mapping);

        let ids: Vec<&str> = ids.iter().map(String::as_str).collect();
        assert_eq!(
            ids,
            ["build", "check", "enrich", "fetch", "inner", "split_users"]
        );
        assert!(wants_error);

        // `data.*`/`variables.*` roots and plain values contribute nothing.
        let (ids, wants_error) = referenced_step_ids(&json!({
            "a": { "valueType": "reference", "value": "data.x" },
            "b": { "valueType": "immediate", "value": "steps.not_a_reference" },
        }));
        assert!(ids.is_empty());
        assert!(!wants_error);
    }

    /// A capped fetch must say so: flag present (with exact totals) only when
    /// records beyond the page limit exist.
    #[test]
    fn truncation_flag_reports_only_actual_truncation() {
        assert_eq!(truncation_flag(2, 2), None);
        assert_eq!(truncation_flag(0, 0), None);
        assert_eq!(
            truncation_flag(500, 731),
            Some(json!({"returned": 500, "totalMatching": 731}))
        );
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
            search: None,
            run_label: None,
            parent_instance_id: Some("parent 1".to_string()),
            workflow_id: Some("workflow/needs encoding".to_string()),
            status: Some("running,queued".to_string()),
            page: Some(2),
            size: Some(50),
            sort_by: Some("createdAt".to_string()),
            sort_order: Some("desc".to_string()),
            state: None,
        });

        assert!(query.contains("workflowId=workflow%2Fneeds%20encoding"));
        assert!(query.contains("parentInstanceId=parent%201"));
        assert!(!query.contains("parent_instance_id="));
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

        let resolved = resolve_input_mappings(&input_mapping, &summaries(), &execution(), &top());

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

        let resolved = resolve_input_mappings(&input_mapping, &summaries(), &execution(), &top());
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

        let resolved = resolve_input_mappings(&input_mapping, &summaries(), &execution(), &top());
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

        let resolved = resolve_input_mappings(&input_mapping, &summaries(), &execution(), &top());

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

        let resolved = resolve_input_mappings(&input_mapping, &summaries(), &execution(), &top());

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

        let resolved = resolve_input_mappings(&input_mapping, &summaries, &execution(), &top());

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
            resolve_reference_value("steps.build", &summaries(), &execution(), &top());
        assert_eq!(
            resolved_bare.and_then(|v| v.get("stepType").cloned()),
            Some(json!("Agent"))
        );

        let resolved_outputs =
            resolve_reference_value("steps.build.outputs", &summaries(), &execution(), &top());
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
        let in_while = in_loops("sc_whileStep_3", &[("whileStep", false)], &["field"]);
        for (path, scope) in [
            ("steps.build.outputs.nested.name", top()),
            ("steps.build.outputs.status", top()),
            ("steps.build.outputs", top()),
            ("steps.embed.outputs.embeddings.0", top()),
            ("steps.missing.outputs.x", top()),
            ("steps.__error.message", top()),
            ("variables.limit", top()),
            ("loop.index", in_while.clone()),
            ("steps.build.outputs", in_while),
        ] {
            let mapping = json!({ "field": { "valueType": "reference", "value": path } });
            let resolved = resolve_input_mappings(&mapping, &summaries, &execution, &scope);
            let via_input_mapping = resolved["field"]
                .get("resolvedValue")
                .cloned()
                .unwrap_or(serde_json::Value::Null);
            let via_shared = resolve_reference_value(path, &summaries, &execution, &scope)
                .unwrap_or(json!(null));
            assert_eq!(
                via_input_mapping, via_shared,
                "resolve_input_mappings diverged from resolve_reference_value for {path}"
            );
        }
    }

    /// `loop` is the innermost enclosing While's context — a Split keeps its
    /// parent's — and `iteration` the innermost loop's, Split or While. Both
    /// indices come from the scope id, read against the known loop ids.
    #[test]
    fn loop_and_iteration_indices_follow_the_enclosing_loops() {
        let resolve = |path: &str, scope: &StepScope| {
            resolve_reference(path, &summaries(), &execution(), scope)
        };
        let found = |value: serde_json::Value| Resolution::Found(value);

        let while_only = in_loops("sc_while_3", &[("while", false)], &[]);
        assert_eq!(resolve("loop.index", &while_only), found(json!(3)));
        assert_eq!(resolve("iteration.index", &while_only), found(json!(3)));
        assert_eq!(resolve("iteration.indices", &while_only), found(json!([3])));
        // In a While with no Split around it, `iteration` is fully known.
        assert_eq!(
            resolve("iteration", &while_only),
            found(json!({ "index": 3, "indices": [3], "item": null }))
        );
        assert_eq!(resolve("iteration.item", &while_only), found(json!(null)));

        // A Split inside a While keeps the While's `loop`.
        let split_in_while = in_loops("sc_w_2_s_5", &[("w", false), ("s", true)], &[]);
        assert_eq!(resolve("loop.index", &split_in_while), found(json!(2)));
        assert_eq!(resolve("iteration.index", &split_in_while), found(json!(5)));
        assert_eq!(
            resolve("iteration.indices", &split_in_while),
            found(json!([2, 5]))
        );

        // A While inside a Split has its own.
        let while_in_split = in_loops("sc_s_1_w_2", &[("s", true), ("w", false)], &[]);
        assert_eq!(resolve("loop.index", &while_in_split), found(json!(2)));
        assert_eq!(
            resolve("iteration.indices", &while_in_split),
            found(json!([1, 2]))
        );

        // No While around a Split step: the runtime has no `loop` at all.
        let split_only = in_loops("sc_split_4", &[("split", true)], &[]);
        assert_eq!(
            resolve("loop.index", &split_only),
            Resolution::Missing(None)
        );
        assert_eq!(resolve("iteration.index", &split_only), found(json!(4)));

        // Loop ids ending in `_<digits>` still parse exactly.
        let tricky = in_loops("sc_while_2_7", &[("while_2", false)], &[]);
        assert_eq!(resolve("loop.index", &tricky), found(json!(7)));
        assert_eq!(resolve("iteration.indices", &tricky), found(json!([7])));

        // Outside any loop neither root exists at run time.
        let top_level = StepScope::new(None, Some(Placement::default()));
        assert_eq!(resolve("loop.index", &top_level), Resolution::Missing(None));
        assert_eq!(
            resolve("iteration.index", &top_level),
            Resolution::Missing(None)
        );
    }

    #[test]
    fn loop_outputs_is_not_reconstructable_from_persisted_state() {
        // Unlike `loop.index`, `loop.outputs` only ever lived in the running
        // iteration and was never persisted — it must be reported as
        // runtime-only rather than fabricated, and so must the bare `loop`
        // object that contains it.
        let in_while = in_loops("sc_while_3", &[("while", false)], &[]);
        for path in ["loop.outputs", "loop.outputs.total", "loop"] {
            assert_eq!(
                resolve_reference(path, &summaries(), &execution(), &in_while),
                Resolution::RuntimeOnly(LOOP_OUTPUTS),
                "{path}"
            );
        }
    }

    #[test]
    fn loop_reference_without_scope_is_unresolved() {
        // No step context (e.g. trace_reference has no step_id param) means no
        // scope to resolve the iteration index against.
        assert_eq!(
            resolve_reference_value("loop.index", &summaries(), &execution(), &top()),
            None
        );
    }

    #[test]
    fn trace_variables_resolves_from_instance_state() {
        // The variables arm must report the running instance's value (an
        // instance launched with overrides diverges from the definition
        // immediately), never the static default from the definition graph —
        // reading the definition is exactly what made this arm disagree with
        // inspect_step for the same reference.
        let response = trace_variables_response("variables.limit", &execution());
        assert_eq!(response["resolved"], json!(true));
        assert_eq!(response["value"], json!(10));
        assert_eq!(response["source"]["allVariables"], json!({ "limit": 10 }));
    }

    #[test]
    fn trace_variables_matches_shared_resolver() {
        // Drift guard: the tool arm routes through resolve_reference_value, so
        // the two can never report different values for the same reference.
        let execution = execution();
        for path in ["variables.limit", "variables.missing"] {
            let via_tool = trace_variables_response(path, &execution)["value"].clone();
            let via_shared =
                resolve_reference_value(path, &synthetic_summaries(Vec::new()), &execution, &top())
                    .unwrap_or(json!(null));
            assert_eq!(
                via_tool, via_shared,
                "trace_reference variables arm diverged from resolve_reference_value for {path}"
            );
        }
    }

    #[test]
    fn trace_variables_unknown_name_is_unresolved() {
        let response = trace_variables_response("variables.missing", &execution());
        assert_eq!(response["resolved"], json!(false));
        assert_eq!(response["value"], json!(null));
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
            resolve_reference_value("steps.__error.message", &summaries, &execution(), &top()),
            Some(json!("Delivery failed"))
        );
        assert_eq!(
            resolve_reference_value("steps.error.category", &summaries, &execution(), &top()),
            Some(json!("transient"))
        );
    }

    #[test]
    fn steps_dunder_error_resolves_flat_error_step_outputs() {
        // SYN-467: verified against a live server — an `Error`-step-type failure
        // has no nested `outputs.error` object at all; the structured fields
        // (`category`/`code`/`message`/`severity`) sit flat on `outputs`
        // alongside `_error`, and the top-level `error` field collapses to the
        // generic "Output reported _error=true" string. Must still recover
        // the real fields from `outputs` directly.
        let summaries = json!({
            "data": {
                "steps": [{
                    "stepId": "boom",
                    "stepType": "Error",
                    "status": "failed",
                    "error": "Output reported _error=true",
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
            resolve_reference_value("steps.__error.message", &summaries, &execution(), &top()),
            Some(json!("Delivery failed"))
        );
        assert_eq!(
            resolve_reference_value("steps.__error.code", &summaries, &execution(), &top()),
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
            resolve_reference_value("steps.__error.category", &summaries, &execution(), &top()),
            Some(json!("transient"))
        );
        assert_eq!(
            resolve_reference_value("steps.__error.code", &summaries, &execution(), &top()),
            Some(json!("NETWORK_ERROR"))
        );
        assert_eq!(
            resolve_reference_value("steps.__error.message", &summaries, &execution(), &top()),
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
            resolve_reference_value("steps.__error.message", &summaries, &execution(), &top()),
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

        let resolved = resolve_input_mappings(&input_mapping, &summaries(), &execution(), &top());

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

        let resolved = resolve_input_mappings(&input_mapping, &summaries(), &execution(), &top());

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

    /// The debugging tools read a reference path with the runtime's tokenizer,
    /// so every spelling of the same path shows the same value. Splitting on
    /// `.` left every bracketed spelling unresolved.
    #[test]
    fn bracket_spellings_resolve_like_dotted_ones() {
        let summaries = summaries_with_failed_step();
        for (dotted, bracketed) in [
            (
                "steps.build.outputs.status",
                r#"steps["build"].outputs.status"#,
            ),
            (
                "steps.build.outputs.nested.name",
                r#"steps['build']["outputs"]["nested"].name"#,
            ),
            (
                "steps.embed.outputs.embeddings.0.1",
                "steps.embed.outputs.embeddings[0][1]",
            ),
            ("steps.__error.message", r#"steps["__error"].message"#),
            ("data.customer.name", r#"data["customer"]['name']"#),
            ("variables.limit", "variables['limit']"),
        ] {
            let expected = resolve_reference_value(dotted, &summaries, &execution(), &top());
            assert!(expected.is_some(), "fixture must resolve {dotted}");
            assert_eq!(
                resolve_reference_value(bracketed, &summaries, &execution(), &top()),
                expected,
                "{bracketed} must resolve like {dotted}"
            );
        }
    }

    /// A bracket-quoted body is one key, dots included — the runtime looks up
    /// `a.b` itself, never `a` then `b`.
    #[test]
    fn bracket_quoted_dotted_key_resolves_as_one_key() {
        let execution = json!({
            "data": { "inputs": { "data": {
                "a.b": "literal dotted key",
                "a": { "b": "nested" }
            }}}
        });
        assert_eq!(
            resolve_reference_value(r#"data["a.b"]"#, &summaries(), &execution, &top()),
            Some(json!("literal dotted key"))
        );
        assert_eq!(
            resolve_reference_value("data.a.b", &summaries(), &execution, &top()),
            Some(json!("nested"))
        );
    }

    /// Segments are walked the way the runtime's `descend` walks them: an
    /// object segment is always a key, and only an index token indexes an
    /// array.
    #[test]
    fn resolve_json_path_reads_segments_like_the_runtime() {
        let value = json!({ "by_slot": { "0": "zero" }, "items": ["a", "b"] });
        assert_eq!(
            resolve_json_path(&value, &segments("by_slot.0")),
            Some(json!("zero"))
        );
        assert_eq!(resolve_json_path(&value, &segments("items[+1]")), None);
        assert_eq!(
            resolve_json_path(&value, &segments("items[1]")),
            Some(json!("b"))
        );
    }

    #[test]
    fn referenced_step_ids_collects_bracketed_ids() {
        let (ids, wants_error) = referenced_step_ids(&json!({
            "a": { "valueType": "reference", "value": "steps['fetch'].outputs" },
            "b": { "valueType": "reference", "value": r#"steps["a.b"].outputs.x"# },
            "c": { "valueType": "reference", "value": r#"steps["__error"].message"# },
        }));
        let ids: Vec<&str> = ids.iter().map(String::as_str).collect();
        assert_eq!(ids, ["a.b", "fetch"]);
        assert!(wants_error);
    }

    /// `fn` call arguments are classified exactly as the runtime classifies
    /// them: by the tokenized root. A bracketed workflow root resolves (the
    /// runtime resolves it before the agent sees it); a column ref, bracketed
    /// or not, stays a reference; and an `item`-rooted ref is a workflow
    /// reference, so it is reported as unresolved rather than shown as a
    /// column name.
    #[test]
    fn score_expression_classifies_bracketed_roots_like_the_runtime() {
        let input_mapping = json!({
            "score_expression": {
                "valueType": "immediate",
                "value": {
                    "alias": "similarity",
                    "expression": {
                        "fn": "SIMILARITY",
                        "arguments": [
                            {"valueType": "reference", "value": "embedding"},
                            {"valueType": "reference", "value": "meta[\"k\"]"},
                            {"valueType": "reference", "value": "steps[\"embed\"].outputs.embeddings[0]"},
                            {"valueType": "reference", "value": "data['threshold']"},
                            {"valueType": "reference", "value": "item.sku"}
                        ]
                    }
                }
            }
        });

        let resolved = resolve_input_mappings(&input_mapping, &summaries(), &execution(), &top());

        assert_eq!(
            resolved["score_expression"]["resolvedValue"]["expression"]["arguments"],
            json!([
                {"valueType": "reference", "value": "embedding"},
                {"valueType": "reference", "value": "meta[\"k\"]"},
                {"valueType": "immediate", "value": [0.1, 0.2, 0.3]},
                {"valueType": "immediate", "value": 7},
                {"valueType": "reference", "value": "item.sku"}
            ])
        );
        // Not placed under a Split, so there is no `item` to resolve (under
        // one it is runtime-only — see the nested runtime-only test).
        assert_eq!(
            resolved["score_expression"]["unresolvedNestedReferences"],
            json!(["item.sku"])
        );
    }

    #[test]
    fn input_mapping_reference_takes_the_bracketed_roots_arm() {
        let input_mapping = json!({
            "status": { "valueType": "reference", "value": r#"steps["build"].outputs.status"# },
            "name": { "valueType": "reference", "value": "data['customer'].name" },
            "limit": { "valueType": "reference", "value": r#"variables["limit"]"# },
        });

        let resolved = resolve_input_mappings(&input_mapping, &summaries(), &execution(), &top());

        assert_eq!(resolved["status"]["sourceStep"], json!("build"));
        assert_eq!(resolved["status"]["resolvedValue"], json!("active"));
        assert_eq!(resolved["name"]["resolvedValue"], json!("Ada"));
        assert_eq!(resolved["limit"]["variableName"], json!("limit"));
        assert_eq!(resolved["limit"]["resolvedValue"], json!(10));
    }

    /// A bare `data` / `variables` is the whole object at runtime, so both
    /// `trace_reference` (via `resolve_reference_value`) and `inspect_step`
    /// (via `resolve_input_mappings`) show it whole rather than nothing.
    #[test]
    fn bare_data_and_variables_resolve_to_the_whole_object() {
        let data = json!({"customer": {"name": "Ada"}, "threshold": 7});
        let variables = json!({"limit": 10});
        assert_eq!(
            resolve_reference_value("data", &summaries(), &execution(), &top()),
            Some(data.clone())
        );
        assert_eq!(
            resolve_reference_value("variables", &summaries(), &execution(), &top()),
            Some(variables.clone())
        );

        let resolved = resolve_input_mappings(
            &json!({
                "all_data": { "valueType": "reference", "value": "data" },
                "all_vars": { "valueType": "reference", "value": "variables" },
            }),
            &summaries(),
            &execution(),
            &top(),
        );
        assert_eq!(resolved["all_data"]["resolvedValue"], data);
        assert_eq!(resolved["all_data"]["source"], json!("workflow_input"));
        assert_eq!(resolved["all_vars"]["resolvedValue"], variables);
        assert_eq!(resolved["all_vars"]["source"], json!("variable"));
        assert!(resolved["all_vars"].get("variableName").is_none());
    }

    #[test]
    fn explain_unresolved_path_reads_a_bracketed_tail() {
        let envelope = json!({ "stepType": "Split", "outputs": [{"id": 1}] });
        let reason = explain_unresolved_path(
            &envelope,
            "steps.split_users",
            &segments(r#"["outputs"]["result"]"#),
        )
        .expect("named key into array must produce a reason");
        assert!(
            reason.contains("is an array") && reason.contains("'result'"),
            "reason: {reason}"
        );
    }

    /// `build_source` sets `workflow` to `{inputs: {data, variables}}` from the
    /// same values as the `data` / `variables` roots, so every spelling of the
    /// same field must resolve to the same value.
    #[test]
    fn workflow_inputs_resolve_like_the_data_and_variables_roots() {
        let resolve =
            |path: &str| resolve_reference_value(path, &summaries(), &execution(), &top());

        for (workflow_path, root_path) in [
            ("workflow.inputs.data.customer.name", "data.customer.name"),
            (r#"workflow["inputs"]['data'].threshold"#, "data.threshold"),
            ("workflow.inputs.data", "data"),
            ("workflow.inputs.variables.limit", "variables.limit"),
            ("workflow.inputs.variables", "variables"),
        ] {
            assert!(resolve(root_path).is_some(), "{root_path} must resolve");
            assert_eq!(
                resolve(workflow_path),
                resolve(root_path),
                "{workflow_path}"
            );
        }

        let inputs = json!({
            "data": resolve("data").unwrap(),
            "variables": resolve("variables").unwrap(),
        });
        assert_eq!(resolve("workflow.inputs"), Some(inputs.clone()));
        assert_eq!(resolve("workflow"), Some(json!({ "inputs": inputs })));
        assert_eq!(resolve("workflow.inputs.data.missing"), None);
        assert_eq!(resolve("workflow.outputs"), None);

        let resolved = resolve_input_mappings(
            &json!({
                "name": { "valueType": "reference", "value": "workflow.inputs.data.customer.name" },
            }),
            &summaries(),
            &execution(),
            &top(),
        );
        assert_eq!(resolved["name"]["resolvedValue"], json!("Ada"));
        assert_eq!(resolved["name"]["source"], json!("workflow_input"));
    }

    /// The bare onError aliases are a mirror of `steps.__error`, so they read
    /// the same envelope — and ask inspect_step to fetch the failed step.
    #[test]
    fn bare_error_aliases_resolve_like_steps_dunder_error() {
        let failed = summaries_with_failed_step();
        for (bare, qualified) in [
            ("__error.message", "steps.__error.message"),
            ("error.category", "steps.error.category"),
            ("__error['code']", "steps.__error.code"),
            ("__error", "steps.__error"),
        ] {
            let via_bare = resolve_reference_value(bare, &failed, &execution(), &top());
            assert!(via_bare.is_some(), "{bare} must resolve");
            assert_eq!(
                via_bare,
                resolve_reference_value(qualified, &failed, &execution(), &top()),
                "{bare}"
            );
        }
        // No failed step, no envelope.
        assert_eq!(
            resolve_reference_value("__error.message", &summaries(), &execution(), &top()),
            None
        );

        let mapping = json!({
            "reason": { "valueType": "reference", "value": "__error.message" },
        });
        let (ids, wants_error) = referenced_step_ids(&mapping);
        assert!(ids.is_empty());
        assert!(wants_error);

        let resolved = resolve_input_mappings(&mapping, &failed, &execution(), &top());
        assert_eq!(
            resolved["reason"]["resolvedValue"],
            json!("Delivery failed")
        );
        assert_eq!(resolved["reason"]["source"], json!("error_context"));
    }

    /// A Split binds `data` to its current element and `item` to the same; a
    /// While or onWait graph inside it keeps that binding. Neither is ever
    /// stored, so for a step under a Split they are runtime-only — and
    /// `workflow.inputs.data`, the same value, with them.
    #[test]
    fn data_and_item_under_a_split_are_runtime_only() {
        let resolve = |path: &str, scope: &StepScope| {
            resolve_reference(path, &summaries(), &execution(), scope)
        };
        let in_split = in_loops("sc_split_4", &[("split", true)], &[]);
        let while_in_split = in_loops("sc_split_1_w_0", &[("split", true), ("w", false)], &[]);
        for scope in [&in_split, &while_in_split] {
            for path in [
                "data.customer.name",
                "data",
                "workflow.inputs.data.threshold",
            ] {
                assert_eq!(
                    resolve(path, scope),
                    Resolution::RuntimeOnly(DATA_IN_SPLIT),
                    "{path}"
                );
            }
            for path in [
                "item",
                "item.sku",
                "item['sku']",
                "iteration.item.sku",
                "iteration",
            ] {
                assert_eq!(
                    resolve(path, scope),
                    Resolution::RuntimeOnly(ITEM_IN_SPLIT),
                    "{path}"
                );
            }
            assert!(matches!(
                resolve("workflow", scope),
                Resolution::RuntimeOnly(_)
            ));
        }

        // A While keeps the workflow input as `data`, and has no `item`.
        let in_while = in_loops("sc_w_0", &[("w", false)], &[]);
        assert_eq!(
            resolve("data.customer.name", &in_while),
            Resolution::Found(json!("Ada"))
        );
        assert_eq!(
            resolve("workflow.inputs.data.threshold", &in_while),
            Resolution::Found(json!(7))
        );
        assert_eq!(resolve("item.sku", &in_while), Resolution::Missing(None));
        // Not placed, or at the top level: no `item` at run time either.
        assert_eq!(resolve("item.sku", &top()), Resolution::Missing(None));
        assert_eq!(resolve("iteration.item", &top()), Resolution::Missing(None));
    }

    /// Every Split/While iteration and onWait run starts with an empty `steps`
    /// map, and the top level never sees the steps inside a loop: a placed
    /// step sees only its own graph's steps, and only their records from its
    /// own iteration.
    #[test]
    fn steps_resolve_only_within_the_same_graph_and_iteration() {
        let summaries = json!({
            "data": {
                "steps": [
                    { "stepId": "fetch", "status": "completed", "scopeId": "sc_split_1",
                      "outputs": { "outputs": { "n": 1 } } },
                    { "stepId": "fetch", "status": "completed", "scopeId": "sc_split_0",
                      "outputs": { "outputs": { "n": 0 } } },
                    { "stepId": "outer", "status": "completed",
                      "outputs": { "outputs": { "ok": true } } },
                ]
            }
        });
        let resolve = |path: &str, scope: &StepScope| {
            resolve_reference(path, &summaries, &execution(), scope)
        };

        // A sibling in the same Split iteration — not the newest record.
        let iteration_0 = in_loops("sc_split_0", &[("split", true)], &["fetch", "use"]);
        assert_eq!(
            resolve("steps.fetch.outputs.n", &iteration_0),
            Resolution::Found(json!(0))
        );
        // A sibling that has no record in this iteration.
        let iteration_2 = in_loops("sc_split_2", &[("split", true)], &["fetch", "use"]);
        assert_eq!(
            resolve("steps.fetch.outputs.n", &iteration_2),
            Resolution::Missing(None)
        );
        // A step outside the loop body is invisible inside it…
        assert_eq!(
            resolve("steps.outer.outputs.ok", &iteration_0),
            Resolution::Missing(Some(STEP_OUTSIDE_GRAPH))
        );
        // …and a top-level step can't see into the loop body.
        let top_level = StepScope::new(
            None,
            Some(Placement {
                siblings: ["outer", "split"].map(String::from).into(),
                ..Placement::default()
            }),
        );
        assert_eq!(
            resolve("steps.outer.outputs.ok", &top_level),
            Resolution::Found(json!(true))
        );
        assert_eq!(
            resolve("steps.fetch.outputs.n", &top_level),
            Resolution::Missing(Some(STEP_OUTSIDE_GRAPH))
        );
        // Unplaced (trace_reference): the newest record, as before.
        assert_eq!(
            resolve("steps.fetch.outputs.n", &top()),
            Resolution::Found(json!(1))
        );

        // inspect_step's mapping view reads the same record and says why.
        let resolved = resolve_input_mappings(
            &json!({
                "mine": { "valueType": "reference", "value": "steps.fetch.outputs.n" },
                "outer": { "valueType": "reference", "value": "steps.outer.outputs.ok" },
            }),
            &summaries,
            &execution(),
            &iteration_0,
        );
        assert_eq!(resolved["mine"]["resolvedValue"], json!(0));
        assert_eq!(resolved["outer"]["sourceStatus"], json!("not_found"));
        assert_eq!(
            resolved["outer"]["resolutionNote"],
            json!(STEP_OUTSIDE_GRAPH)
        );
        assert!(resolved["outer"].get("runtimeOnly").is_none());
    }

    /// Inside a loop, `variables` also holds what the loop binds per
    /// iteration; outside one, the iteration bookkeeping doesn't exist.
    #[test]
    fn variables_bound_by_the_runtime_or_a_loop_are_runtime_only() {
        let resolve = |path: &str, scope: &StepScope| {
            resolve_reference(path, &summaries(), &execution(), scope)
        };
        let mut in_split = in_loops("sc_split_0", &[("split", true)], &[]);
        if let Some(placement) = in_split.placement.as_mut() {
            placement.loops[0].variables.insert("batch".to_string());
            placement.loops[0].variables.insert("limit".to_string());
        }

        // Bound by the loop — even where it shadows a workflow variable.
        assert_eq!(
            resolve("variables.batch", &in_split),
            Resolution::RuntimeOnly(LOOP_VARIABLE)
        );
        assert_eq!(
            resolve("variables.limit", &in_split),
            Resolution::RuntimeOnly(LOOP_VARIABLE)
        );
        assert_eq!(
            resolve("variables", &in_split),
            Resolution::RuntimeOnly(VARIABLES_IN_NESTED_GRAPH)
        );
        assert_eq!(
            resolve("workflow.inputs.variables.batch", &in_split),
            Resolution::RuntimeOnly(LOOP_VARIABLE)
        );
        assert_eq!(
            resolve("variables._item", &in_split),
            Resolution::RuntimeOnly(ITERATION_VARIABLE)
        );
        assert_eq!(
            resolve("variables._loop", &in_split),
            Resolution::Missing(None)
        );

        // Everything else still comes from the workflow's variables.
        let in_while = in_loops("sc_w_0", &[("w", false)], &[]);
        assert_eq!(
            resolve("variables.limit", &in_while),
            Resolution::Found(json!(10))
        );
        assert_eq!(
            resolve("variables._loop", &in_while),
            Resolution::RuntimeOnly(ITERATION_VARIABLE)
        );
        assert_eq!(
            resolve("variables._item", &in_while),
            Resolution::Missing(None)
        );

        // Internal bookkeeping is runtime-only everywhere, in either spelling.
        for scope in [&top(), &in_while] {
            for path in [
                "variables._loop_path",
                "variables['_durable_key_version']",
                "workflow.inputs.variables._manifest_graph_path",
            ] {
                assert_eq!(
                    resolve(path, scope),
                    Resolution::RuntimeOnly(INTERNAL_VARIABLE),
                    "{path}"
                );
            }
        }
        // Iteration bookkeeping doesn't exist at the top level.
        assert_eq!(
            resolve("variables._scope_id", &top()),
            Resolution::Missing(None)
        );
        assert_eq!(
            resolve("variables._scope_id", &in_while),
            Resolution::RuntimeOnly(ITERATION_VARIABLE)
        );

        // onWait binds `_signal_id`; nowhere else has it.
        let on_wait = StepScope::new(
            None,
            Some(Placement {
                in_on_wait: true,
                ..Placement::default()
            }),
        );
        assert_eq!(
            resolve("variables._signal_id", &on_wait),
            Resolution::RuntimeOnly(SIGNAL_ID_VARIABLE)
        );
        assert_eq!(
            resolve("variables._signal_id", &top()),
            Resolution::Missing(None)
        );
        assert_eq!(
            resolve("variables", &on_wait),
            Resolution::RuntimeOnly(VARIABLES_IN_NESTED_GRAPH)
        );
        assert_eq!(
            resolve("data.threshold", &on_wait),
            Resolution::Found(json!(7))
        );
    }

    fn execution_with_stored_variables(variables: serde_json::Value) -> serde_json::Value {
        json!({
            "data": {
                "id": "inst-1",
                "workflowId": "wf-from-record",
                "inputs": { "data": { "x": 1 }, "variables": variables },
            }
        })
    }

    fn definition_declaring(variables: serde_json::Value) -> serde_json::Value {
        json!({ "data": { "definition": { "executionGraph": { "variables": variables } } } })
    }

    /// The runtime's top-level `variables`: the declared defaults, the stored
    /// input over them minus `_`-prefixed input (bar `_cache_key_prefix`),
    /// and the identity the runtime injects itself.
    #[test]
    fn with_runtime_variables_matches_the_runtime_variable_set() {
        let definition = definition_declaring(json!({
            "limit": { "type": "number", "value": 5 },
            "region": { "type": "string", "value": "eu" },
            "plain": true,
        }));
        let execution = with_runtime_variables(
            execution_with_stored_variables(json!({
                "limit": 10,
                "_instance_id": "forged",
                "_loop_path": ["forged"],
                "_cache_key_prefix": "ns",
            })),
            Some(&definition),
            "inst-1",
            "wf-from-param",
            "tenant-a",
        );

        let resolve = |path: &str| resolve_reference_value(path, &summaries(), &execution, &top());
        assert_eq!(
            resolve("variables"),
            Some(json!({
                "limit": 10,
                "region": "eu",
                "plain": true,
                "_cache_key_prefix": "ns",
                "_instance_id": "inst-1",
                "_tenant_id": "tenant-a",
                "_workflow_id": "wf-from-record",
            }))
        );
        assert_eq!(resolve("variables.region"), Some(json!("eu")));
        assert_eq!(resolve("variables._instance_id"), Some(json!("inst-1")));
        assert_eq!(resolve("variables._loop_path"), None);
        assert_eq!(
            resolve("workflow.inputs.variables._tenant_id"),
            Some(json!("tenant-a"))
        );
        // `data` is untouched.
        assert_eq!(resolve("data"), Some(json!({ "x": 1 })));

        // No definition, no stored variables, no workflow id on the record.
        let bare = with_runtime_variables(
            json!({ "data": { "inputs": { "data": {} } } }),
            None,
            "inst-2",
            "wf-from-param",
            "tenant-a",
        );
        assert_eq!(
            instance_variables(&bare),
            Some(&json!({
                "_instance_id": "inst-2",
                "_tenant_id": "tenant-a",
                "_workflow_id": "wf-from-param",
            }))
        );
    }

    #[test]
    fn trace_variables_shows_runtime_identity_and_marks_runtime_only() {
        let execution = with_runtime_variables(
            execution_with_stored_variables(json!({ "limit": 10 })),
            Some(&definition_declaring(
                json!({ "region": { "type": "string", "value": "eu" } }),
            )),
            "inst-1",
            "wf",
            "tenant-a",
        );

        let response = trace_variables_response("variables", &execution);
        assert_eq!(response["resolved"], json!(true));
        assert_eq!(response["value"]["_instance_id"], json!("inst-1"));
        assert_eq!(response["value"]["_tenant_id"], json!("tenant-a"));
        assert_eq!(response["value"]["_workflow_id"], json!("wf-from-record"));
        assert_eq!(response["value"]["region"], json!("eu"));
        assert_eq!(response["source"]["allVariables"], response["value"]);
        assert!(response.get("runtimeOnly").is_none());

        let response = trace_variables_response("variables._loop_path", &execution);
        assert_eq!(response["resolved"], json!(false));
        assert_eq!(response["runtimeOnly"], json!(true));
        assert_eq!(response["reason"], json!(INTERNAL_VARIABLE));

        let response = trace_variables_response("variables.missing", &execution);
        assert_eq!(response["resolved"], json!(false));
        assert!(response.get("runtimeOnly").is_none());
        assert!(response.get("reason").is_none());
    }

    #[test]
    fn trace_response_reports_each_resolution() {
        let source = json!({ "type": "workflow_input" });
        let response = trace_response("x", Resolution::Found(json!(1)), source.clone());
        assert_eq!(
            (response["resolved"].clone(), response["value"].clone()),
            (json!(true), json!(1))
        );
        let response = trace_response(
            "x",
            Resolution::RuntimeOnly(INTERNAL_VARIABLE),
            source.clone(),
        );
        assert_eq!(response["runtimeOnly"], json!(true));
        assert_eq!(response["reason"], json!(INTERNAL_VARIABLE));
        let response = trace_response("x", Resolution::Missing(None), source);
        assert_eq!(response["resolved"], json!(false));
        assert!(response.get("reason").is_none());
    }

    #[test]
    fn trace_iteration_response_points_at_inspect_step() {
        for (reference, root, kind) in [
            ("item.sku", "item", "iteration"),
            ("iteration.index", "iteration", "iteration"),
            ("loop.index", "loop", "loop_context"),
        ] {
            let response = trace_iteration_response(reference, root);
            assert_eq!(response["resolved"], json!(false));
            assert_eq!(response["source"]["type"], json!(kind));
            assert!(response.get("runtimeOnly").is_none());
            assert!(
                response["reason"]
                    .as_str()
                    .is_some_and(|reason| reason.contains("inspect_step")),
                "reason: {}",
                response["reason"]
            );
        }
    }

    /// `item.*` inside an fn call under a Split is a workflow reference the
    /// tools can't resolve: it is reported as runtime-only, apart from
    /// genuinely missing references, and a `default` doesn't stand in for it.
    #[test]
    fn nested_runtime_only_references_are_listed_apart_from_missing_ones() {
        let mapping = json!({
            "label": {
                "valueType": "immediate",
                "value": {
                    "fn": "concat",
                    "arguments": [
                        { "valueType": "reference", "value": "item.sku", "default": "fallback" },
                        { "valueType": "reference", "value": "steps.ghost.outputs.x" },
                        { "valueType": "reference", "value": "iteration.index" },
                    ]
                }
            }
        });

        let in_split = in_loops("sc_split_7", &[("split", true)], &["ghost", "label"]);
        let resolved = resolve_input_mappings(&mapping, &summaries(), &execution(), &in_split);
        let label = &resolved["label"];
        assert_eq!(
            label["unresolvedNestedReferences"],
            json!(["steps.ghost.outputs.x"])
        );
        let runtime_only = label["runtimeOnlyReferences"].as_array().unwrap();
        assert_eq!(runtime_only.len(), 1);
        assert_eq!(runtime_only[0]["reference"], json!("item.sku"));
        assert_eq!(runtime_only[0]["reason"], json!(ITEM_IN_SPLIT));
        assert_eq!(
            label["resolvedValue"]["arguments"][0]["valueType"],
            json!("reference")
        );
        assert_eq!(
            label["resolvedValue"]["arguments"][2],
            json!({ "valueType": "immediate", "value": 7 })
        );

        // Only runtime-only references: no `unresolvedNestedReferences` key.
        let resolved = resolve_input_mappings(
            &json!({
                "sku": {
                    "valueType": "composite",
                    "value": { "sku": { "valueType": "reference", "value": "item.sku" } }
                }
            }),
            &summaries(),
            &execution(),
            &in_split,
        );
        assert!(resolved["sku"].get("unresolvedNestedReferences").is_none());
        assert_eq!(
            resolved["sku"]["runtimeOnlyReferences"][0]["reference"],
            json!("item.sku")
        );
    }

    /// A found `null` or a miss falls back to the reference's `default`, as
    /// the runtime's `resolve_lookup` does.
    #[test]
    fn nested_reference_defaults_apply_like_the_runtime() {
        let resolved = resolve_input_mappings(
            &json!({
                "pair": {
                    "valueType": "composite",
                    "value": {
                        "missing": { "valueType": "reference", "value": "data.nope", "default": 1 },
                        "found": { "valueType": "reference", "value": "data.threshold", "default": 1 },
                    }
                }
            }),
            &summaries(),
            &execution(),
            &top(),
        );
        assert_eq!(
            resolved["pair"]["resolvedValue"],
            json!({ "missing": 1, "found": 7 })
        );
        assert!(resolved["pair"].get("unresolvedNestedReferences").is_none());
    }

    /// A step inside a loop only exists in its container's nested graph; the
    /// lookup must find it there — and record where it sits.
    #[test]
    fn locate_step_reaches_nested_graphs_and_records_placement() {
        let workflow = json!({
            "data": {
                "definition": {
                    "executionGraph": {
                        "steps": {
                            "top": { "id": "top", "inputMapping": { "a": 1 } },
                            "split": {
                                "id": "split",
                                "stepType": "Split",
                                "config": { "variables": { "batch": {} } },
                                "subgraph": {
                                    "variables": { "declared": {} },
                                    "steps": {
                                        "emit": { "id": "emit", "inputMapping": { "b": 2 } },
                                        "loop": {
                                            "id": "loop",
                                            "stepType": "While",
                                            "subgraph": {
                                                "steps": {
                                                    "deep": { "id": "deep", "inputMapping": { "c": 3 } }
                                                }
                                            }
                                        }
                                    }
                                }
                            },
                            "wait": {
                                "id": "wait",
                                "stepType": "WaitForSignal",
                                "onWait": {
                                    "steps": {
                                        "ping": { "id": "ping", "inputMapping": { "d": 4 } }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        });

        let (step, placement) = locate_step(&workflow, "top").unwrap();
        assert_eq!(step["inputMapping"], json!({ "a": 1 }));
        assert!(!placement.in_nested_graph());
        assert_eq!(
            placement.siblings,
            BTreeSet::from(["split", "top", "wait"].map(String::from))
        );

        let (step, placement) = locate_step(&workflow, "emit").unwrap();
        assert_eq!(step["inputMapping"], json!({ "b": 2 }));
        assert_eq!(placement.loops.len(), 1);
        assert!(placement.loops[0].is_split);
        assert_eq!(
            placement.loops[0].variables,
            BTreeSet::from(["batch", "declared"].map(String::from))
        );
        assert_eq!(
            placement.siblings,
            BTreeSet::from(["emit", "loop"].map(String::from))
        );

        let (step, placement) = locate_step(&workflow, "deep").unwrap();
        assert_eq!(step["inputMapping"], json!({ "c": 3 }));
        let loops: Vec<(&str, bool)> = placement
            .loops
            .iter()
            .map(|enclosing| (enclosing.step_id.as_str(), enclosing.is_split))
            .collect();
        assert_eq!(loops, [("split", true), ("loop", false)]);

        let (step, placement) = locate_step(&workflow, "ping").unwrap();
        assert_eq!(step["inputMapping"], json!({ "d": 4 }));
        assert!(placement.in_on_wait && !placement.in_loop());

        assert!(locate_step(&workflow, "missing").is_none());
        // The legacy shape without the `definition` wrapper.
        let legacy = json!({ "data": workflow["data"]["definition"].clone() });
        assert!(locate_step(&legacy, "deep").is_some());
    }

    #[test]
    fn direct_runtime_only_reference_is_marked() {
        let in_while = in_loops("sc_while_3", &[("while", false)], &[]);
        let in_split = in_loops("sc_split_3", &[("split", true)], &[]);
        let resolve = |mapping: serde_json::Value, scope: &StepScope| {
            resolve_input_mappings(&mapping, &summaries(), &execution(), scope)
        };
        let resolved = resolve(
            json!({
                "sku": { "valueType": "reference", "value": "item.sku" },
                "idx": { "valueType": "reference", "value": "iteration.index" },
                "name": { "valueType": "reference", "value": "data.customer.name" },
            }),
            &in_split,
        );
        assert_eq!(resolved["sku"]["source"], json!("iteration"));
        assert_eq!(resolved["sku"]["resolvedValue"], json!(null));
        assert_eq!(resolved["sku"]["runtimeOnly"], json!(true));
        assert_eq!(resolved["sku"]["resolutionNote"], json!(ITEM_IN_SPLIT));
        assert_eq!(resolved["idx"]["resolvedValue"], json!(3));
        assert!(resolved["idx"].get("runtimeOnly").is_none());
        assert_eq!(resolved["name"]["runtimeOnly"], json!(true));
        assert_eq!(resolved["name"]["resolutionNote"], json!(DATA_IN_SPLIT));

        let resolved = resolve(
            json!({
                "prev": { "valueType": "reference", "value": "loop.outputs" },
                "gone": { "valueType": "reference", "value": "variables.missing" },
            }),
            &in_while,
        );
        assert_eq!(resolved["prev"]["runtimeOnly"], json!(true));
        assert_eq!(resolved["gone"]["resolvedValue"], json!(null));
        assert!(resolved["gone"].get("runtimeOnly").is_none());
    }
}
