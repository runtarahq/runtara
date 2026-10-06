use rmcp::model::{CallToolResult, ContentBlock};
use runtara_workflow_stdlib::reference_path::{
    array_index, is_array_index_token, is_workflow_reference, reference_segments,
};
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

/// Find a step's definition in a fetched workflow, at the top level or nested
/// in a Split/While `subgraph` or a WaitForSignal `onWait` graph. Steps inside
/// a loop — the only ones with an iteration scope for `loop.*` / `iteration.*`
/// to resolve against — never sit at the top level.
fn find_step_definition<'a>(
    workflow: &'a serde_json::Value,
    step_id: &str,
) -> Option<&'a serde_json::Value> {
    let steps = workflow
        .pointer("/data/definition/executionGraph/steps")
        .or_else(|| workflow.pointer("/data/executionGraph/steps"))?;
    find_step_in_graph(steps, step_id)
}

/// Depth-first search of a graph's `steps` map and the graphs nested in it.
fn find_step_in_graph<'a>(
    steps: &'a serde_json::Value,
    step_id: &str,
) -> Option<&'a serde_json::Value> {
    let steps = steps.as_object()?;
    steps.get(step_id).or_else(|| {
        steps.values().find_map(|step| {
            ["subgraph", "onWait"]
                .into_iter()
                .find_map(|nested| find_step_in_graph(step.get(nested)?.get("steps")?, step_id))
        })
    })
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

/// Resolve a `steps.<id>.<path>`, `data.<path>`, `variables.<path>`,
/// `workflow.<path>`, `loop.<path>`, `iteration.<path>` or bare
/// `__error.<path>`/`error.<path>` reference against step summaries /
/// execution input. `item.*` and the parts of `loop`/`iteration` that only
/// live in a running iteration resolve to `None` here; see
/// [`runtime_only_reason`].
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
/// `loop.*` and `iteration.*` need it, to recover the iteration index — see
/// `loop_index_from_scope_id`).
fn resolve_reference_value(
    ref_path: &str,
    summaries: &serde_json::Value,
    execution: &serde_json::Value,
    scope_id: Option<&str>,
) -> Option<serde_json::Value> {
    // Tokenized exactly as the runtime reads the path, so a bracketed spelling
    // (`steps['fetch'].outputs`, `data["a.b"]`) resolves like the dotted one.
    let segments = reference_segments(ref_path);
    let (root, rest) = segments.split_first()?;
    resolve_root(root, rest, summaries, execution, scope_id)
}

/// The per-root half of [`resolve_reference_value`], over already-tokenized
/// segments, so `workflow.inputs.<root>.*` can hand its tail to the same arm.
fn resolve_root(
    root: &str,
    rest: &[String],
    summaries: &serde_json::Value,
    execution: &serde_json::Value,
    scope_id: Option<&str>,
) -> Option<serde_json::Value> {
    match root {
        "steps" => {
            let (source_step_id, field_path) = rest.split_first()?;
            if is_error_alias(source_step_id) {
                // `__error`/`error` aren't real steps — the runtime injects the
                // captured onError envelope under this synthetic id when routing
                // to a failure handler (see `error_steps` in
                // runtara-workflow-stdlib). The MCP tools only see historical
                // step summaries, not which specific failure triggered a given
                // onError edge, so surface the first failed step's error as a
                // best-effort match — the same "primary failure" step
                // `why_execution_failed` already reports.
                let envelope = find_error_envelope(summaries)?;
                return resolve_json_path(&envelope, field_path);
            }
            // A step summary's `outputs` field is the full step *envelope*
            // (`{ "outputs": <actual>, "stepId", "stepType", ... }`) — exactly the
            // runtime `steps.<id>` value. Resolve the remainder after the step id
            // (the leading `outputs` segment included) against that envelope, the
            // same way the workflow runtime resolves `steps.<id>.<path>`.
            let source = find_step_in_summaries(summaries, source_step_id)?;
            let envelope = source.get("outputs")?;
            resolve_json_path(envelope, field_path)
        }
        // The bare onError aliases: `build_source` mirrors `steps.__error` to
        // the source root during onError dispatch, so they read the same
        // envelope as the `steps.__error` arm above.
        "__error" | "error" => resolve_json_path(&find_error_envelope(summaries)?, rest),
        // A bare `data` / `variables` is the whole object, as at runtime.
        "data" => resolve_json_path(instance_data(execution)?, rest),
        "variables" => resolve_json_path(instance_variables(execution)?, rest),
        // `build_source` sets `workflow` to `{inputs: {data, variables}}` from
        // the same `data` and `variables` the step sees, so its two subtrees
        // resolve exactly like those roots.
        "workflow" => match rest {
            [inputs, scope, tail @ ..]
                if inputs == "inputs" && (scope == "data" || scope == "variables") =>
            {
                resolve_root(scope, tail, summaries, execution, scope_id)
            }
            _ => {
                let workflow = json!({
                    "inputs": {
                        "data": instance_data(execution)?,
                        "variables": instance_variables(execution)
                            .cloned()
                            .unwrap_or_else(|| json!({})),
                    }
                });
                resolve_json_path(&workflow, rest)
            }
        },
        // Of `loop` and `iteration` only the index is recoverable, from the
        // step's scope id (see `loop_index_from_scope_id`). The rest —
        // `loop.outputs`, `iteration.indices`/`.item` — only ever lived in the
        // running iteration's variables and was never persisted, so it is
        // absent here rather than fabricated (see `runtime_only_reason`).
        "loop" => {
            let loop_context = json!({ "index": loop_index_from_scope_id(scope_id?)? });
            resolve_json_path(&loop_context, rest)
        }
        "iteration" => match rest {
            [index, tail @ ..] if index == "index" => {
                let index = json!(loop_index_from_scope_id(scope_id?)?);
                resolve_json_path(&index, tail)
            }
            _ => None,
        },
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

/// The instance's variables as stored with its execution record. Run them
/// through [`with_runtime_variables`] first to see the runtime's `variables`.
fn instance_variables(execution: &serde_json::Value) -> Option<&serde_json::Value> {
    execution
        .pointer("/data/inputs/variables")
        .or_else(|| execution.pointer("/data/variables"))
}

/// True for the synthetic onError step id / bare root (`__error`, `error`).
fn is_error_alias(segment: &str) -> bool {
    segment == "__error" || segment == "error"
}

/// Variables the runtime sets for itself on every run or inside every loop
/// iteration — durable-key and manifest bookkeeping, scope ids and the legacy
/// per-iteration bindings behind `loop`/`iteration`/`item`. None of them is
/// stored with the instance, and the tools can't rebuild them, so a reference
/// to one is reported as runtime-only rather than as missing.
const RUNTIME_ONLY_VARIABLES: &[&str] = &[
    "_durable_key_version",
    "_loop_path",
    "_manifest_graph_path",
    "_cache_key_prefix",
    "_scope_id",
    "_parent_scope_id",
    "_loop_indices",
    "_index",
    "_item",
    "_loop",
    "_previousOutputs",
];

/// Why a reference the tools could not resolve has a value only while the
/// workflow runs, or `None` when it is simply missing. Consulted only after
/// [`resolve_reference_value`] comes back empty.
fn runtime_only_reason(ref_path: &str) -> Option<&'static str> {
    let segments = reference_segments(ref_path);
    match segments.as_slice() {
        [root, field, ..] if root == "loop" && field == "outputs" => Some(
            "loop.outputs is the previous While iteration's output; it lives only in the \
             running iteration and is never persisted, so it is known only at run time.",
        ),
        [root, field, ..] if root == "iteration" && field != "index" => Some(
            "Only iteration.index can be recovered (from the step's scope id); the rest of \
             iteration lives only in the running Split/While iteration and is known only at \
             run time.",
        ),
        [root] if root == "iteration" => Some(
            "Only iteration.index can be recovered (from the step's scope id); the whole \
             iteration object lives only in the running Split/While iteration and is known \
             only at run time.",
        ),
        [root, ..] if root == "item" => Some(
            "item is the current Split element; it lives only in the running iteration and \
             is never persisted, so it is known only at run time.",
        ),
        [root, name, ..]
            if root == "variables" && RUNTIME_ONLY_VARIABLES.contains(&name.as_str()) =>
        {
            Some(
                "This variable is set by the runtime itself and never stored with the \
                 instance, so its value is known only at run time.",
            )
        }
        _ => None,
    }
}

/// The instance record with its variables replaced by the set the runtime
/// resolves `variables.*` against, as far as the tools can rebuild it.
/// `build_source` drops `_`-prefixed input variables (all but
/// `_cache_key_prefix`) and the runtime adds `_instance_id`, `_tenant_id` and
/// `_workflow_id` itself; those three are this instance's id, the server's
/// tenant and the record's workflow id. The other runtime-set variables stay
/// out (see [`RUNTIME_ONLY_VARIABLES`]).
fn with_runtime_variables(
    mut execution: serde_json::Value,
    instance_id: &str,
    workflow_id: &str,
    tenant_id: &str,
) -> serde_json::Value {
    let workflow_id = execution
        .pointer("/data/workflowId")
        .and_then(|v| v.as_str())
        .unwrap_or(workflow_id)
        .to_string();
    let mut variables = match instance_variables(&execution) {
        Some(serde_json::Value::Object(stored)) => stored.clone(),
        _ => serde_json::Map::new(),
    };
    variables.retain(|name, _| !name.starts_with('_') || name == "_cache_key_prefix");
    variables.insert("_instance_id".into(), json!(instance_id));
    variables.insert("_tenant_id".into(), json!(tenant_id));
    variables.insert("_workflow_id".into(), json!(workflow_id));

    // Written back where `instance_variables` reads first: inside the
    // `{data, variables}` input envelope when there is one, else beside it.
    let in_envelope = execution.pointer("/data/inputs/variables").is_some()
        || execution.pointer("/data/inputs/data").is_some();
    let target = if in_envelope {
        execution.pointer_mut("/data/inputs")
    } else {
        execution.pointer_mut("/data")
    };
    if let Some(target) = target.and_then(|t| t.as_object_mut()) {
        target.insert("variables".into(), serde_json::Value::Object(variables));
    }
    execution
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
/// run time (see [`runtime_only_reason`]) apart from the ones that are simply
/// missing.
fn note_unresolved_references(entry: &mut serde_json::Value, unresolved_refs: Vec<String>) {
    if unresolved_refs.is_empty() {
        return;
    }
    let mut missing = Vec::new();
    let mut runtime_only = Vec::new();
    for reference in unresolved_refs {
        match runtime_only_reason(&reference) {
            Some(reason) => runtime_only.push(json!({ "reference": reference, "reason": reason })),
            None => missing.push(reference),
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

/// Helper: resolve inputMapping references against step summaries. `scope_id`
/// is the scope of the step this input mapping belongs to (needed to resolve
/// `loop.*` / `iteration.*` references — see `resolve_reference_value`).
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
                    // Tokenized as the runtime reads it, like the shared
                    // resolver below, so a bracketed root or step id
                    // (`steps['fetch'].outputs`) takes the same arm.
                    let segments = reference_segments(ref_path);
                    let (root, rest) = match segments.split_first() {
                        Some((root, rest)) => (root.as_str(), rest),
                        None => ("", &[][..]),
                    };
                    match root {
                        "steps" if !rest.is_empty() => {
                            let source_step_id = rest[0].as_str();

                            if is_error_alias(source_step_id) {
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
                        "__error" | "error" => {
                            entry["source"] = json!("error_context");
                            entry["resolvedValue"] =
                                resolve_reference_value(ref_path, summaries, execution, scope_id)
                                    .unwrap_or(json!(null));
                        }
                        "data" => {
                            if let Some(inputs) = instance_data(execution) {
                                entry["resolvedValue"] =
                                    resolve_json_path(inputs, rest).unwrap_or(json!(null));
                            }
                            entry["source"] = json!("workflow_input");
                        }
                        "workflow" => {
                            entry["source"] = json!("workflow_input");
                            entry["resolvedValue"] =
                                resolve_reference_value(ref_path, summaries, execution, scope_id)
                                    .unwrap_or(json!(null));
                        }
                        "variables" => {
                            entry["source"] = json!("variable");
                            if let Some(name) = rest.first() {
                                entry["variableName"] = json!(name);
                            }
                            // Route through the shared resolver, same as the
                            // steps/data arms above — this used to be dropped,
                            // silently reporting resolvedValue:null even though
                            // the runtime resolves workflow variables fine.
                            entry["resolvedValue"] =
                                resolve_reference_value(ref_path, summaries, execution, scope_id)
                                    .unwrap_or(json!(null));
                        }
                        "loop" => {
                            entry["source"] = json!("loop");
                            entry["resolvedValue"] =
                                resolve_reference_value(ref_path, summaries, execution, scope_id)
                                    .unwrap_or(json!(null));
                        }
                        "iteration" | "item" => {
                            entry["source"] = json!("iteration");
                            entry["resolvedValue"] =
                                resolve_reference_value(ref_path, summaries, execution, scope_id)
                                    .unwrap_or(json!(null));
                        }
                        _ => {}
                    }
                    if entry
                        .get("resolvedValue")
                        .is_none_or(serde_json::Value::is_null)
                        && let Some(reason) = runtime_only_reason(ref_path)
                    {
                        entry["runtimeOnly"] = json!(true);
                        entry["resolutionNote"] = json!(reason);
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
                note_unresolved_references(&mut entry, unresolved_refs);
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
                note_unresolved_references(&mut entry, unresolved_refs);
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
                note_unresolved_references(&mut entry, unresolved_refs);
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

    // Fetch the workflow definition first: the target's inputMapping decides
    // which other steps are needed for reference resolution, so it must be
    // known before any step payloads are fetched.
    let workflow = api_get(
        server,
        &format!("/api/runtime/workflows/{}", params.workflow_id),
    )
    .await?;

    // Fetch execution for input data and variables.
    let execution =
        fetch_execution_for_references(server, &params.workflow_id, &params.instance_id).await?;

    // Extract step definition from workflow graph
    let input_mapping = find_step_definition(&workflow, &params.step_id)
        .and_then(|step| step.get("inputMapping"))
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

    let scope_id = target.get("scopeId").and_then(|v| v.as_str());
    let resolved_inputs = resolve_input_mappings(&input_mapping, &summaries, &execution, scope_id);

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
/// runtime variable state via the shared resolver, never from the static
/// defaults in the workflow definition graph.
fn trace_variables_response(reference: &str, execution: &serde_json::Value) -> serde_json::Value {
    let resolved =
        resolve_reference_value(reference, &synthetic_summaries(Vec::new()), execution, None)
            .unwrap_or(json!(null));

    // Same lookup as the resolver's variables arm, surfaced whole for context
    // alongside the resolved value.
    let variables = instance_variables(execution).cloned().unwrap_or(json!({}));

    let mut response = json!({
        "reference": reference,
        "resolved": !resolved.is_null(),
        "value": resolved,
        "source": {
            "type": "variable",
            "allVariables": variables,
            "note": "allVariables is the instance's stored variables plus the _instance_id, \
                     _tenant_id and _workflow_id the runtime adds. Variables the runtime keeps \
                     for its own bookkeeping (e.g. _durable_key_version, _loop_path, \
                     _manifest_graph_path) are never stored and are not shown.",
        }
    });
    if resolved.is_null()
        && let Some(reason) = runtime_only_reason(reference)
    {
        response["runtimeOnly"] = json!(true);
        response["reason"] = json!(reason);
    }
    response
}

/// The `trace_reference` response for a `loop`/`iteration`/`item` reference.
/// These resolve per iteration and `trace_reference` has no step to take a
/// scope from, so nothing is fetched: the response says why there is no value.
fn trace_iteration_response(reference: &str, root: &str) -> serde_json::Value {
    let runtime_only = runtime_only_reason(reference);
    let reason = runtime_only.unwrap_or(
        "Recovered from the scope of the step that reads it, and trace_reference has no \
         step to take a scope from — use inspect_step on a step inside the loop.",
    );
    let mut response = json!({
        "reference": reference,
        "resolved": false,
        "value": null,
        "source": {
            "type": if root == "loop" { "loop_context" } else { "iteration" },
        },
        "reason": reason,
    });
    if runtime_only.is_some() {
        response["runtimeOnly"] = json!(true);
    }
    response
}

/// Fetch the instance record for reference resolution: in full (a reference
/// may point into a large input field the default detail fetch elides; the
/// MCP response is re-truncated downstream, so the wire stays bounded) and
/// with its variables as the runtime sees them ([`with_runtime_variables`]).
async fn fetch_execution_for_references(
    server: &SmoMcpServer,
    workflow_id: &str,
    instance_id: &str,
) -> Result<serde_json::Value, rmcp::ErrorData> {
    let execution = api_get(
        server,
        &format!("/api/runtime/workflows/instances/{instance_id}?full=true"),
    )
    .await?;
    Ok(with_runtime_variables(
        execution,
        instance_id,
        workflow_id,
        &server.tenant_id,
    ))
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
            "type": "error_context",
            "stepId": alias,
        }
    }))
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
                None,
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
        // `trace_reference` has no step_id param, so there's no scope to
        // recover `loop.index` / `iteration.index` from, and the rest of these
        // roots is runtime-only: say which instead of rejecting the root.
        // inspect_step resolves the index given the step's scope.
        "loop" | "iteration" | "item" => {
            json_result(trace_iteration_response(&params.reference, root))
        }
        "data" => {
            let execution =
                fetch_execution_for_references(server, &params.workflow_id, &params.instance_id)
                    .await?;

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
            // the runtime (see resolve_reference_value).
            let execution =
                fetch_execution_for_references(server, &params.workflow_id, &params.instance_id)
                    .await?;

            json_result(trace_variables_response(&params.reference, &execution))
        }
        "workflow" => {
            // `workflow.inputs.data` / `.variables` are the same values as the
            // `data` / `variables` roots; the shared resolver hands them to
            // those arms.
            let execution =
                fetch_execution_for_references(server, &params.workflow_id, &params.instance_id)
                    .await?;
            let resolved = resolve_reference_value(
                &params.reference,
                &synthetic_summaries(Vec::new()),
                &execution,
                None,
            )
            .unwrap_or(json!(null));

            json_result(json!({
                "reference": params.reference,
                "resolved": !resolved.is_null(),
                "value": resolved,
                "source": {
                    "type": "workflow_input",
                }
            }))
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

    // Fetch execution status
    let execution = with_runtime_variables(
        api_get(
            server,
            &format!("/api/runtime/workflows/instances/{}", params.instance_id),
        )
        .await?,
        &params.instance_id,
        &params.workflow_id,
        &server.tenant_id,
    );

    let status = execution
        .pointer("/data/status")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");

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

        // Try to resolve inputs for the failing step
        let workflow = api_get(
            server,
            &format!("/api/runtime/workflows/{}", params.workflow_id),
        )
        .await
        .ok();

        let input_mapping = workflow
            .as_ref()
            .and_then(|workflow| find_step_definition(workflow, step_id))
            .and_then(|step| step.get("inputMapping"))
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
                resolve_reference_value(path, &synthetic_summaries(Vec::new()), &execution, None)
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
            let expected = resolve_reference_value(dotted, &summaries, &execution(), None);
            assert!(expected.is_some(), "fixture must resolve {dotted}");
            assert_eq!(
                resolve_reference_value(bracketed, &summaries, &execution(), None),
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
            resolve_reference_value(r#"data["a.b"]"#, &summaries(), &execution, None),
            Some(json!("literal dotted key"))
        );
        assert_eq!(
            resolve_reference_value("data.a.b", &summaries(), &execution, None),
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
    /// reference the runtime resolves but persisted state cannot reproduce, so
    /// it is reported as runtime-only rather than shown as a column name.
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

        let resolved = resolve_input_mappings(&input_mapping, &summaries(), &execution(), None);

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
        assert!(
            resolved["score_expression"]
                .get("unresolvedNestedReferences")
                .is_none()
        );
        assert_eq!(
            resolved["score_expression"]["runtimeOnlyReferences"][0]["reference"],
            json!("item.sku")
        );
    }

    #[test]
    fn input_mapping_reference_takes_the_bracketed_roots_arm() {
        let input_mapping = json!({
            "status": { "valueType": "reference", "value": r#"steps["build"].outputs.status"# },
            "name": { "valueType": "reference", "value": "data['customer'].name" },
            "limit": { "valueType": "reference", "value": r#"variables["limit"]"# },
        });

        let resolved = resolve_input_mappings(&input_mapping, &summaries(), &execution(), None);

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
            resolve_reference_value("data", &summaries(), &execution(), None),
            Some(data.clone())
        );
        assert_eq!(
            resolve_reference_value("variables", &summaries(), &execution(), None),
            Some(variables.clone())
        );

        let resolved = resolve_input_mappings(
            &json!({
                "all_data": { "valueType": "reference", "value": "data" },
                "all_vars": { "valueType": "reference", "value": "variables" },
            }),
            &summaries(),
            &execution(),
            None,
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
        let resolve = |path: &str| resolve_reference_value(path, &summaries(), &execution(), None);

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
            None,
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
            let via_bare = resolve_reference_value(bare, &failed, &execution(), None);
            assert!(via_bare.is_some(), "{bare} must resolve");
            assert_eq!(
                via_bare,
                resolve_reference_value(qualified, &failed, &execution(), None),
                "{bare}"
            );
        }
        // No failed step, no envelope.
        assert_eq!(
            resolve_reference_value("__error.message", &summaries(), &execution(), None),
            None
        );

        let mapping = json!({
            "reason": { "valueType": "reference", "value": "__error.message" },
        });
        let (ids, wants_error) = referenced_step_ids(&mapping);
        assert!(ids.is_empty());
        assert!(wants_error);

        let resolved = resolve_input_mappings(&mapping, &failed, &execution(), None);
        assert_eq!(
            resolved["reason"]["resolvedValue"],
            json!("Delivery failed")
        );
        assert_eq!(resolved["reason"]["source"], json!("error_context"));
    }

    /// `iteration.index` is recoverable from the step's scope id, like
    /// `loop.index`; the rest of `iteration`, and `item`, only ever exists in
    /// the running iteration.
    #[test]
    fn iteration_index_resolves_from_scope_id_and_the_rest_is_runtime_only() {
        let resolve = |path: &str, scope: Option<&str>| {
            resolve_reference_value(path, &summaries(), &execution(), scope)
        };
        assert_eq!(
            resolve("iteration.index", Some("sc_split_4")),
            Some(json!(4))
        );
        assert_eq!(
            resolve("iteration['index']", Some("sc_outer_1_split_2")),
            Some(json!(2))
        );
        assert_eq!(resolve("iteration.index", None), None);
        assert_eq!(runtime_only_reason("iteration.index"), None);

        for path in [
            "iteration",
            "iteration.indices",
            "iteration.indices.0",
            "iteration.item.sku",
            "item",
            "item.sku",
            "item['sku']",
        ] {
            assert_eq!(resolve(path, Some("sc_split_4")), None, "{path}");
            assert!(runtime_only_reason(path).is_some(), "{path}");
        }
    }

    #[test]
    fn runtime_only_reason_classifies_only_runtime_held_values() {
        for path in [
            "loop.outputs",
            "loop.outputs.total",
            "variables._loop_path",
            "variables._durable_key_version",
            "variables['_manifest_graph_path']",
            "variables._scope_id",
        ] {
            assert!(runtime_only_reason(path).is_some(), "{path}");
        }
        // Resolvable or genuinely missing — not runtime-only.
        for path in [
            "loop.index",
            "loop",
            "variables.missing",
            "variables._instance_id",
            "variables._typo",
            "data.item",
            "steps.item.outputs",
        ] {
            assert_eq!(runtime_only_reason(path), None, "{path}");
        }
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

    /// The runtime's `variables` is the stored set minus `_`-prefixed input
    /// (bar `_cache_key_prefix`) plus the identity it injects itself.
    #[test]
    fn with_runtime_variables_matches_the_runtime_variable_set() {
        let execution = with_runtime_variables(
            execution_with_stored_variables(json!({
                "limit": 10,
                "_instance_id": "forged",
                "_loop_path": ["forged"],
                "_cache_key_prefix": "ns",
            })),
            "inst-1",
            "wf-from-param",
            "tenant-a",
        );

        let variables =
            resolve_reference_value("variables", &summaries(), &execution, None).unwrap();
        assert_eq!(
            variables,
            json!({
                "limit": 10,
                "_cache_key_prefix": "ns",
                "_instance_id": "inst-1",
                "_tenant_id": "tenant-a",
                "_workflow_id": "wf-from-record",
            })
        );
        assert_eq!(
            resolve_reference_value("variables._instance_id", &summaries(), &execution, None),
            Some(json!("inst-1"))
        );
        assert_eq!(
            resolve_reference_value("variables._loop_path", &summaries(), &execution, None),
            None
        );
        assert_eq!(
            resolve_reference_value(
                "workflow.inputs.variables._tenant_id",
                &summaries(),
                &execution,
                None
            ),
            Some(json!("tenant-a"))
        );
        // `data` is untouched.
        assert_eq!(
            resolve_reference_value("data", &summaries(), &execution, None),
            Some(json!({ "x": 1 }))
        );

        // No stored variables at all, and no workflow id on the record.
        let bare = with_runtime_variables(
            json!({ "data": { "inputs": { "data": {} } } }),
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
            "inst-1",
            "wf",
            "tenant-a",
        );

        let response = trace_variables_response("variables", &execution);
        assert_eq!(response["resolved"], json!(true));
        assert_eq!(response["value"]["_instance_id"], json!("inst-1"));
        assert_eq!(response["value"]["_tenant_id"], json!("tenant-a"));
        assert_eq!(response["value"]["_workflow_id"], json!("wf-from-record"));
        assert_eq!(response["source"]["allVariables"], response["value"]);
        assert!(response.get("runtimeOnly").is_none());

        let response = trace_variables_response("variables._loop_path", &execution);
        assert_eq!(response["resolved"], json!(false));
        assert_eq!(response["runtimeOnly"], json!(true));
        assert!(response["reason"].as_str().is_some());

        let response = trace_variables_response("variables.missing", &execution);
        assert_eq!(response["resolved"], json!(false));
        assert!(response.get("runtimeOnly").is_none());
    }

    #[test]
    fn trace_iteration_response_says_why_there_is_no_value() {
        let response = trace_iteration_response("item.sku", "item");
        assert_eq!(response["resolved"], json!(false));
        assert_eq!(response["runtimeOnly"], json!(true));
        assert_eq!(response["source"]["type"], json!("iteration"));

        // The index is recoverable, just not without a step scope.
        let response = trace_iteration_response("loop.index", "loop");
        assert!(response.get("runtimeOnly").is_none());
        assert_eq!(response["source"]["type"], json!("loop_context"));
        assert!(
            response["reason"]
                .as_str()
                .is_some_and(|reason| reason.contains("inspect_step")),
            "reason: {}",
            response["reason"]
        );
    }

    /// A Split-scoped `item.*` inside an fn call is a workflow reference the
    /// tools can't resolve; it must be reported as runtime-only, apart from
    /// genuinely missing references.
    #[test]
    fn nested_runtime_only_references_are_listed_apart_from_missing_ones() {
        let mapping = json!({
            "label": {
                "valueType": "immediate",
                "value": {
                    "fn": "concat",
                    "arguments": [
                        { "valueType": "reference", "value": "item.sku" },
                        { "valueType": "reference", "value": "steps.ghost.outputs.x" },
                        { "valueType": "reference", "value": "iteration.index" },
                    ]
                }
            }
        });

        let resolved =
            resolve_input_mappings(&mapping, &summaries(), &execution(), Some("sc_split_7"));
        let label = &resolved["label"];
        assert_eq!(
            label["unresolvedNestedReferences"],
            json!(["steps.ghost.outputs.x"])
        );
        let runtime_only = label["runtimeOnlyReferences"].as_array().unwrap();
        assert_eq!(runtime_only.len(), 1);
        assert_eq!(runtime_only[0]["reference"], json!("item.sku"));
        assert!(runtime_only[0]["reason"].as_str().is_some());
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
            None,
        );
        assert!(resolved["sku"].get("unresolvedNestedReferences").is_none());
        assert_eq!(
            resolved["sku"]["runtimeOnlyReferences"][0]["reference"],
            json!("item.sku")
        );
    }

    /// A step inside a loop only exists in its container's nested graph; the
    /// lookup must find it there, or inspect_step resolves nothing for it.
    #[test]
    fn find_step_definition_reaches_nested_graphs() {
        let workflow = json!({
            "data": {
                "definition": {
                    "executionGraph": {
                        "steps": {
                            "top": { "id": "top", "inputMapping": { "a": 1 } },
                            "split": {
                                "id": "split",
                                "stepType": "Split",
                                "subgraph": {
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

        let mapping =
            |id: &str| find_step_definition(&workflow, id).map(|step| step["inputMapping"].clone());
        assert_eq!(mapping("top"), Some(json!({ "a": 1 })));
        assert_eq!(mapping("emit"), Some(json!({ "b": 2 })));
        assert_eq!(mapping("deep"), Some(json!({ "c": 3 })));
        assert_eq!(mapping("ping"), Some(json!({ "d": 4 })));
        assert_eq!(mapping("missing"), None);

        // The legacy shape without the `definition` wrapper.
        let legacy = json!({ "data": workflow["data"]["definition"].clone() });
        assert!(find_step_definition(&legacy, "deep").is_some());
    }

    #[test]
    fn direct_runtime_only_reference_is_marked() {
        let resolved = resolve_input_mappings(
            &json!({
                "sku": { "valueType": "reference", "value": "item.sku" },
                "idx": { "valueType": "reference", "value": "iteration.index" },
                "prev": { "valueType": "reference", "value": "loop.outputs" },
                "gone": { "valueType": "reference", "value": "variables.missing" },
            }),
            &summaries(),
            &execution(),
            Some("sc_while_3"),
        );

        assert_eq!(resolved["sku"]["source"], json!("iteration"));
        assert_eq!(resolved["sku"]["resolvedValue"], json!(null));
        assert_eq!(resolved["sku"]["runtimeOnly"], json!(true));
        assert!(resolved["sku"]["resolutionNote"].as_str().is_some());

        assert_eq!(resolved["idx"]["resolvedValue"], json!(3));
        assert!(resolved["idx"].get("runtimeOnly").is_none());

        assert_eq!(resolved["prev"]["runtimeOnly"], json!(true));

        assert_eq!(resolved["gone"]["resolvedValue"], json!(null));
        assert!(resolved["gone"].get("runtimeOnly").is_none());
    }
}
