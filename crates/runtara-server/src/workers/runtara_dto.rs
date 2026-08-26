//! Conversions between Runtara SDK types and local DTOs.
//!
//! Helpers for translating between `runtara-management-sdk` instance
//! representations and the server-side `WorkflowInstanceDto`
//! / `ExecutionWithMetadata` shapes used by HTTP handlers.
//!
//! Previously housed inside `api/services/executions.rs`; extracted so the
//! shared `ExecutionEngine` can use them without pulling in the legacy
//! service.

use chrono::{DateTime, Utc};
use futures::StreamExt;
use runtara_management_sdk::{InstanceInfo, InstanceStatus as RuntaraInstanceStatus};
use serde_json::Value;

use crate::api::dto::workflows::{InstanceInputs, WorkflowInstanceDto};
use crate::api::services::pending_inputs::has_open_inputs;
use crate::runtime_client::RuntimeClient;
use crate::types::ExecutionStatus;

/// Extended execution data with metadata from the workflow record.
///
/// Used when fetching a single execution with full details.
#[derive(Debug)]
pub struct ExecutionWithMetadata {
    pub instance: WorkflowInstanceDto,
    pub workflow_name: Option<String>,
    pub workflow_description: Option<String>,
    pub worker_id: Option<String>,
    pub retry_count: Option<i32>,
    pub max_retries: Option<i32>,
    pub additional_metadata: Option<Value>,
    pub error_message: Option<String>,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
}

/// Convert Runtara instance status to the local `ExecutionStatus` enum.
pub fn runtara_status_to_execution_status(status: RuntaraInstanceStatus) -> ExecutionStatus {
    match status {
        RuntaraInstanceStatus::Unknown => ExecutionStatus::Queued,
        RuntaraInstanceStatus::Pending => ExecutionStatus::Queued,
        RuntaraInstanceStatus::Running => ExecutionStatus::Running,
        RuntaraInstanceStatus::Suspended => ExecutionStatus::Suspended,
        RuntaraInstanceStatus::Completed => ExecutionStatus::Completed,
        RuntaraInstanceStatus::Failed => ExecutionStatus::Failed,
        RuntaraInstanceStatus::Cancelled => ExecutionStatus::Cancelled,
    }
}

/// Convert a local execution status string to its Runtara counterpart.
pub fn execution_status_to_runtara(status: &str) -> Option<RuntaraInstanceStatus> {
    match status {
        "queued" => Some(RuntaraInstanceStatus::Pending),
        "compiling" => Some(RuntaraInstanceStatus::Pending), // No direct equivalent
        "running" => Some(RuntaraInstanceStatus::Running),
        "suspended" => Some(RuntaraInstanceStatus::Suspended),
        "completed" => Some(RuntaraInstanceStatus::Completed),
        "failed" | "timeout" => Some(RuntaraInstanceStatus::Failed),
        "cancelled" => Some(RuntaraInstanceStatus::Cancelled),
        _ => None,
    }
}

/// Convert a set of local execution statuses to the Runtara statuses a listing
/// should match.
///
/// The mapping is many-to-one — `queued` and `compiling` both mean `Pending`,
/// `failed` and `timeout` both mean `Failed` — so repeats are collapsed and the
/// result can be shorter than the input. Unrecognized entries are dropped;
/// callers validate the vocabulary before getting here.
pub fn execution_statuses_to_runtara(statuses: &[String]) -> Vec<RuntaraInstanceStatus> {
    let mut mapped: Vec<RuntaraInstanceStatus> = Vec::new();
    for status in statuses {
        if let Some(runtara_status) = execution_status_to_runtara(status)
            && !mapped.contains(&runtara_status)
        {
            mapped.push(runtara_status);
        }
    }
    mapped
}

/// Execution duration in seconds derived from the instance timestamps.
///
/// Returns `None` unless both timestamps are present and `finished_at` is at
/// or after `started_at`. A relaunched/resumed run can briefly carry a
/// `finished_at` stamped by an earlier suspend that predates its current
/// `started_at`; such a row would otherwise report a negative duration, so it
/// is dropped here rather than surfaced to the UI.
fn duration_seconds(
    started_at: Option<DateTime<Utc>>,
    finished_at: Option<DateTime<Utc>>,
) -> Option<f64> {
    let (start, end) = started_at.zip(finished_at)?;
    let ms = (end - start).num_milliseconds();
    (ms >= 0).then(|| ms as f64 / 1000.0)
}

/// Enrich running instances with `has_pending_input` by checking for
/// unresolved `external_input_requested` events.
///
/// Only queries events for running instances. The request/resolution pairing is
/// shared with the per-instance pending-input and action endpoints (see
/// `api::services::pending_inputs`) so the list and the detail views cannot
/// disagree about the same run.
///
/// Each lookup is a round trip to the environment service, so they run
/// concurrently rather than one row at a time — a page holds up to 100
/// instances and the executions views poll it on a 10s interval, which would
/// otherwise make list latency scale with how busy the tenant is.
pub async fn enrich_pending_input(instances: &mut [WorkflowInstanceDto], client: &RuntimeClient) {
    // Bounded rather than unbounded: the executions views poll this endpoint
    // per open tab and per tenant, so a full page fanning out at once would
    // turn one slow list into a burst against the environment service.
    const MAX_CONCURRENT_LOOKUPS: usize = 8;

    // `iter_mut` hands each future a disjoint borrow of its own row, so results
    // are written back in place and completion order does not matter. The
    // futures are built in an explicit loop (concrete lifetimes, no closure
    // HRTB) as elsewhere in this codebase, since a `map` closure yielding a
    // future that borrows its argument mutably does not infer.
    let mut lookups = Vec::new();
    for instance in instances.iter_mut() {
        if instance.status != ExecutionStatus::Running {
            continue;
        }

        lookups.push(async move {
            match has_open_inputs(client, &instance.id).await {
                Ok(has_open) => instance.has_pending_input = has_open,
                // The flag stays false, so the indicator is hidden rather than
                // wrongly shown for every running row during a runtime hiccup —
                // but the hiccup itself must not pass silently.
                Err(error) => tracing::warn!(
                    instance_id = %instance.id,
                    error = %error,
                    "Failed to resolve pending input state; reporting none"
                ),
            }
        });
    }

    futures::stream::iter(lookups)
        .buffer_unordered(MAX_CONCURRENT_LOOKUPS)
        .for_each(|()| std::future::ready(()))
        .await;
}

/// Convert Runtara `InstanceSummary` to `WorkflowInstanceDto` with workflow
/// info from a database lookup.
pub fn runtara_instance_to_dto_with_info(
    inst: runtara_management_sdk::InstanceSummary,
    workflow_id: String,
    version: i32,
    workflow_name: Option<String>,
) -> WorkflowInstanceDto {
    // Convert to execution status
    let status = runtara_status_to_execution_status(inst.status);

    // Calculate execution duration if available (dropping negatives from a
    // stale suspend `finished_at` on a resumed run).
    let execution_duration_seconds = duration_seconds(inst.started_at, inst.finished_at);

    WorkflowInstanceDto {
        id: inst.instance_id.clone(),
        created: inst.created_at.to_rfc3339(),
        updated: inst
            .finished_at
            .map(|t| t.to_rfc3339())
            .unwrap_or_else(|| inst.created_at.to_rfc3339()),
        status,
        termination_type: None, // Not available from Runtara summary
        error: None,            // Summary carries only `has_error`, not the message
        workflow_id,
        workflow_name,
        inputs: InstanceInputs {
            data: Value::Null,
            variables: Value::Null,
        },
        outputs: None, // Not available in summary
        tags: vec![],
        used_version: version,
        steps: vec![],
        execution_duration_seconds,
        max_memory_mb: None,
        queue_duration_seconds: None,
        processing_overhead_seconds: None,
        has_pending_input: false,
    }
}

/// Convert Runtara `InstanceInfo` (detailed) to `WorkflowInstanceDto`.
pub fn runtara_info_to_dto(info: InstanceInfo) -> WorkflowInstanceDto {
    // Convert to execution status
    let status = runtara_status_to_execution_status(info.status);

    // Calculate execution duration if available (dropping negatives from a
    // stale suspend `finished_at` on a resumed run).
    let execution_duration_seconds = duration_seconds(info.started_at, info.finished_at);

    let created = info.created_at.to_rfc3339();

    let updated = info
        .finished_at
        .map(|t| t.to_rfc3339())
        .unwrap_or_else(|| created.clone());

    // Extract workflow_id and version from image_name (format: workflow_id:version)
    let (workflow_id, version) = parse_image_id(&info.image_name);

    // Extract data and variables from input to avoid double-wrapping
    let (data, variables) = extract_input_fields(info.input.as_ref());

    WorkflowInstanceDto {
        id: info.instance_id.clone(),
        created,
        updated,
        status,
        termination_type: None,
        error: info.error.clone(),
        workflow_id,
        workflow_name: None,
        inputs: InstanceInputs { data, variables },
        outputs: info.output,
        tags: vec![],
        used_version: version,
        steps: vec![],
        execution_duration_seconds,
        max_memory_mb: None,
        queue_duration_seconds: None,
        processing_overhead_seconds: None,
        has_pending_input: false,
    }
}

/// Convert Runtara `InstanceInfo` to `ExecutionWithMetadata`.
///
/// Used when enriching a single execution with workflow metadata.
pub fn runtara_info_to_execution_with_metadata(
    info: InstanceInfo,
    workflow_name: Option<String>,
    workflow_description: Option<String>,
) -> ExecutionWithMetadata {
    // Convert to WorkflowInstanceDto first
    let status = runtara_status_to_execution_status(info.status);

    // Calculate execution duration if available (dropping negatives from a
    // stale suspend `finished_at` on a resumed run).
    let execution_duration_seconds = duration_seconds(info.started_at, info.finished_at);

    let created = info.created_at.to_rfc3339();
    let updated = info
        .finished_at
        .map(|t| t.to_rfc3339())
        .unwrap_or_else(|| created.clone());

    let (workflow_id, version) = parse_image_id(&info.image_name);

    // Extract data and variables from input to avoid double-wrapping
    let (data, variables) = extract_input_fields(info.input.as_ref());

    let instance = WorkflowInstanceDto {
        id: info.instance_id.clone(),
        created,
        updated,
        status,
        termination_type: None,
        error: info.error.clone(),
        workflow_id,
        workflow_name: workflow_name.clone(),
        inputs: InstanceInputs { data, variables },
        outputs: info.output,
        tags: vec![],
        used_version: version,
        steps: vec![],
        execution_duration_seconds,
        max_memory_mb: None,
        queue_duration_seconds: None,
        processing_overhead_seconds: None,
        has_pending_input: false,
    };

    ExecutionWithMetadata {
        instance,
        workflow_name,
        workflow_description,
        worker_id: None, // Not tracked by Runtara at server level
        retry_count: Some(info.retry_count as i32),
        max_retries: Some(info.max_retries as i32),
        additional_metadata: None,
        error_message: info.error,
        started_at: info.started_at,
        completed_at: info.finished_at,
    }
}

/// Extract `data` and `variables` from Runtara input.
///
/// Runtara stores inputs in the format: `{"data": {...}, "variables": {...}}`.
/// This helper peels those fields so callers don't double-wrap when
/// constructing `InstanceInputs`.
pub fn extract_input_fields(input: Option<&Value>) -> (Value, Value) {
    if let Some(obj) = input.and_then(|v| v.as_object()) {
        (
            obj.get("data").cloned().unwrap_or(Value::Null),
            obj.get("variables").cloned().unwrap_or(Value::Null),
        )
    } else {
        // Fallback: treat entire input as data
        (input.cloned().unwrap_or(Value::Null), Value::Null)
    }
}

/// Per-string ceiling for instance-detail input/output payloads. Strings above
/// this (overwhelmingly base64 file uploads inlined into the input envelope)
/// are replaced with a compact stub on the default detail fetch; the full value
/// is still retrievable via `?full=true`. 16 KB sits just above the 8 KB
/// step-debug cap so instance detail is never *more* aggressive than step IO.
pub const ELIDE_THRESHOLD_BYTES: usize = 16 * 1024;
const ELIDE_PREVIEW_CHARS: usize = 256;

/// Recursively replace any string longer than [`ELIDE_THRESHOLD_BYTES`] with a
/// `{_truncated,_elided,_original_size,_preview}` stub, walking objects and
/// arrays. The stub keys match the frontend's existing `truncated-payload`
/// shape so the truncation badge renders without client changes; `_elided`
/// marks that the full value is fetchable via `?full=true`. Catches base64
/// `content` fields, giant prose outputs, and stringified blobs uniformly —
/// cheaper and more robust than file-shape detection.
pub fn elide_large_strings(value: Value) -> Value {
    match value {
        Value::String(s) if s.len() > ELIDE_THRESHOLD_BYTES => {
            let mut cut = ELIDE_PREVIEW_CHARS.min(s.len());
            while cut > 0 && !s.is_char_boundary(cut) {
                cut -= 1;
            }
            serde_json::json!({
                "_truncated": true,
                "_elided": true,
                "_original_size": s.len(),
                "_preview": &s[..cut],
            })
        }
        Value::Array(items) => Value::Array(items.into_iter().map(elide_large_strings).collect()),
        Value::Object(map) => Value::Object(
            map.into_iter()
                .map(|(k, v)| (k, elide_large_strings(v)))
                .collect(),
        ),
        other => other,
    }
}

/// Elide large strings in a detail DTO's input/output payloads in place. Applied
/// on the default detail fetch; skipped when the caller asks for `?full=true`.
pub fn elide_instance_io(dto: &mut WorkflowInstanceDto) {
    dto.inputs.data = elide_large_strings(std::mem::take(&mut dto.inputs.data));
    dto.inputs.variables = elide_large_strings(std::mem::take(&mut dto.inputs.variables));
    if let Some(outputs) = dto.outputs.take() {
        dto.outputs = Some(elide_large_strings(outputs));
    }
}

/// Parse `image_id` (format: `"workflow_id:version"`) into
/// `(workflow_id, version)`. Returns `(workflow_id, 0)` if no colon is found.
pub fn parse_image_id(image_id: &str) -> (String, i32) {
    if let Some(pos) = image_id.rfind(':') {
        let workflow_id = image_id[..pos].to_string();
        let version = image_id[pos + 1..].parse::<i32>().unwrap_or(0);
        (workflow_id, version)
    } else {
        (image_id.to_string(), 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn elide_large_strings_collapses_big_strings_and_keeps_small_ones() {
        let big = "A".repeat(ELIDE_THRESHOLD_BYTES + 1);
        let input = json!({
            "data": {
                "upload": { "content": big, "filename": "big.bin" },
                "small": "hello",
                "n": 42,
            },
            "list": ["short", "x".repeat(ELIDE_THRESHOLD_BYTES + 10)],
        });

        let out = elide_large_strings(input);

        // Large string -> stub with the frontend-recognized keys.
        let stub = &out["data"]["upload"]["content"];
        assert_eq!(stub["_truncated"], json!(true));
        assert_eq!(stub["_elided"], json!(true));
        assert_eq!(stub["_original_size"], json!(ELIDE_THRESHOLD_BYTES + 1));
        assert!(stub["_preview"].as_str().unwrap().len() <= 256);

        // Small values pass through verbatim, including inside arrays/objects.
        assert_eq!(out["data"]["upload"]["filename"], json!("big.bin"));
        assert_eq!(out["data"]["small"], json!("hello"));
        assert_eq!(out["data"]["n"], json!(42));
        assert_eq!(out["list"][0], json!("short"));
        assert_eq!(out["list"][1]["_elided"], json!(true));
    }

    // =========================================================================
    // parse_image_id tests
    // =========================================================================

    #[test]
    fn test_parse_image_id_standard_format() {
        let (workflow_id, version) = parse_image_id("my-workflow:5");
        assert_eq!(workflow_id, "my-workflow");
        assert_eq!(version, 5);
    }

    #[test]
    fn test_parse_image_id_uuid_format() {
        let (workflow_id, version) = parse_image_id("550e8400-e29b-41d4-a716-446655440000:42");
        assert_eq!(workflow_id, "550e8400-e29b-41d4-a716-446655440000");
        assert_eq!(version, 42);
    }

    #[test]
    fn test_parse_image_id_no_version() {
        let (workflow_id, version) = parse_image_id("workflow-without-version");
        assert_eq!(workflow_id, "workflow-without-version");
        assert_eq!(version, 0);
    }

    #[test]
    fn test_parse_image_id_invalid_version() {
        let (workflow_id, version) = parse_image_id("my-workflow:invalid");
        assert_eq!(workflow_id, "my-workflow");
        assert_eq!(version, 0);
    }

    #[test]
    fn test_parse_image_id_multiple_colons() {
        // Uses rfind so it should parse from the last colon
        let (workflow_id, version) = parse_image_id("org:tenant:workflow:10");
        assert_eq!(workflow_id, "org:tenant:workflow");
        assert_eq!(version, 10);
    }

    #[test]
    fn test_parse_image_id_empty_string() {
        let (workflow_id, version) = parse_image_id("");
        assert_eq!(workflow_id, "");
        assert_eq!(version, 0);
    }

    // =========================================================================
    // extract_input_fields tests
    // =========================================================================

    #[test]
    fn test_extract_input_fields_standard_format() {
        let input = json!({
            "data": {"user": "john"},
            "variables": {"env": "prod"}
        });

        let (data, variables) = extract_input_fields(Some(&input));
        assert_eq!(data, json!({"user": "john"}));
        assert_eq!(variables, json!({"env": "prod"}));
    }

    #[test]
    fn test_extract_input_fields_missing_variables() {
        let input = json!({"data": {"user": "john"}});

        let (data, variables) = extract_input_fields(Some(&input));
        assert_eq!(data, json!({"user": "john"}));
        assert_eq!(variables, Value::Null);
    }

    #[test]
    fn test_extract_input_fields_none() {
        let (data, variables) = extract_input_fields(None);
        assert_eq!(data, Value::Null);
        assert_eq!(variables, Value::Null);
    }

    #[test]
    fn test_extract_input_fields_non_object() {
        let input = json!("just a string");
        let (data, variables) = extract_input_fields(Some(&input));
        // Fallback: treat entire input as data
        assert_eq!(data, json!("just a string"));
        assert_eq!(variables, Value::Null);
    }

    // =========================================================================
    // execution_status_to_runtara tests
    // =========================================================================

    #[test]
    fn test_execution_status_to_runtara_queued() {
        assert_eq!(
            execution_status_to_runtara("queued"),
            Some(RuntaraInstanceStatus::Pending)
        );
    }

    #[test]
    fn test_execution_status_to_runtara_running() {
        assert_eq!(
            execution_status_to_runtara("running"),
            Some(RuntaraInstanceStatus::Running)
        );
    }

    #[test]
    fn test_execution_status_to_runtara_completed() {
        assert_eq!(
            execution_status_to_runtara("completed"),
            Some(RuntaraInstanceStatus::Completed)
        );
    }

    #[test]
    fn test_execution_status_to_runtara_failed() {
        assert_eq!(
            execution_status_to_runtara("failed"),
            Some(RuntaraInstanceStatus::Failed)
        );
    }

    #[test]
    fn test_execution_status_to_runtara_cancelled() {
        assert_eq!(
            execution_status_to_runtara("cancelled"),
            Some(RuntaraInstanceStatus::Cancelled)
        );
    }

    #[test]
    fn test_execution_status_to_runtara_unknown() {
        assert_eq!(execution_status_to_runtara("invalid_status"), None);
    }

    // =========================================================================
    // execution_statuses_to_runtara tests
    // =========================================================================

    fn statuses(values: &[&str]) -> Vec<String> {
        values.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn test_execution_statuses_to_runtara_keeps_every_status() {
        assert_eq!(
            execution_statuses_to_runtara(&statuses(&["failed", "cancelled"])),
            vec![
                RuntaraInstanceStatus::Failed,
                RuntaraInstanceStatus::Cancelled
            ]
        );
    }

    #[test]
    fn test_execution_statuses_to_runtara_collapses_shared_mappings() {
        // failed/timeout and queued/compiling each collapse onto one runtime
        // status; the filter must not ask for it twice.
        assert_eq!(
            execution_statuses_to_runtara(&statuses(&["failed", "timeout"])),
            vec![RuntaraInstanceStatus::Failed]
        );
        assert_eq!(
            execution_statuses_to_runtara(&statuses(&["queued", "compiling"])),
            vec![RuntaraInstanceStatus::Pending]
        );
    }

    #[test]
    fn test_execution_statuses_to_runtara_preserves_order() {
        assert_eq!(
            execution_statuses_to_runtara(&statuses(&["cancelled", "running", "completed"])),
            vec![
                RuntaraInstanceStatus::Cancelled,
                RuntaraInstanceStatus::Running,
                RuntaraInstanceStatus::Completed
            ]
        );
    }

    #[test]
    fn test_execution_statuses_to_runtara_drops_unknown_entries() {
        assert_eq!(
            execution_statuses_to_runtara(&statuses(&["nonsense", "running"])),
            vec![RuntaraInstanceStatus::Running]
        );
        assert!(execution_statuses_to_runtara(&statuses(&["nonsense"])).is_empty());
        assert!(execution_statuses_to_runtara(&[]).is_empty());
    }

    // =========================================================================
    // runtara_status_to_execution_status tests
    // =========================================================================

    #[test]
    fn test_runtara_status_to_execution_status_pending() {
        assert_eq!(
            runtara_status_to_execution_status(RuntaraInstanceStatus::Pending),
            ExecutionStatus::Queued
        );
    }

    #[test]
    fn test_runtara_status_to_execution_status_running() {
        assert_eq!(
            runtara_status_to_execution_status(RuntaraInstanceStatus::Running),
            ExecutionStatus::Running
        );
    }

    #[test]
    fn test_runtara_status_to_execution_status_completed() {
        assert_eq!(
            runtara_status_to_execution_status(RuntaraInstanceStatus::Completed),
            ExecutionStatus::Completed
        );
    }

    #[test]
    fn test_runtara_status_to_execution_status_failed() {
        assert_eq!(
            runtara_status_to_execution_status(RuntaraInstanceStatus::Failed),
            ExecutionStatus::Failed
        );
    }

    #[test]
    fn test_runtara_status_to_execution_status_cancelled() {
        assert_eq!(
            runtara_status_to_execution_status(RuntaraInstanceStatus::Cancelled),
            ExecutionStatus::Cancelled
        );
    }

    #[test]
    fn test_runtara_status_to_execution_status_suspended() {
        assert_eq!(
            runtara_status_to_execution_status(RuntaraInstanceStatus::Suspended),
            ExecutionStatus::Suspended
        );
    }
}
