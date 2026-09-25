//! Conversions between Runtara SDK types and local DTOs.
//!
//! Helpers for translating between `runtara-management-sdk` instance
//! representations and the server-side `WorkflowInstanceDto`
//! / `ExecutionWithMetadata` shapes used by HTTP handlers.
//!
//! Previously housed inside `api/services/executions.rs`; extracted so the
//! shared `ExecutionEngine` can use them without pulling in the legacy
//! service.

use crate::runtime_types::{InstanceInfo, InstanceStatus as RuntaraInstanceStatus};
use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::api::dto::workflows::{InstanceInputs, WorkflowInstanceDto};
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

/// Enrich authorized execution rows from one authoritative request snapshot.
/// Suspended and paused waits remain eligible. Failure propagates to the
/// enclosing response instead of presenting unknown state as no pending input.
pub async fn enrich_pending_input(
    instances: &mut [WorkflowInstanceDto],
    client: &RuntimeClient,
    tenant_id: &str,
) -> runtara_core::persistence::inputs::InputResult<()> {
    let ids: Vec<_> = instances
        .iter()
        .map(|instance| instance.id.clone())
        .collect();
    if ids.is_empty() {
        return Ok(());
    }
    let pending = client.instances_with_open_inputs(tenant_id, &ids).await?;
    // Apply only after the entire lookup succeeded; never leave partial flags.
    for instance in instances {
        instance.has_pending_input = pending.contains(&instance.id);
    }
    Ok(())
}

/// Convert Runtara `InstanceSummary` to `WorkflowInstanceDto` with workflow
/// info from a database lookup.
pub fn runtara_instance_to_dto_with_info(
    inst: crate::runtime_types::InstanceSummary,
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
        run_label: inst.run_label,
        completed_at: inst.finished_at.map(|t| t.to_rfc3339()),
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

    // Extract workflow_id and version from image_name (format:
    // workflow_id:version or workflow_id:version@artifact-fingerprint).
    let (workflow_id, version) = parse_image_id(&info.image_name);

    // Extract data and variables from input to avoid double-wrapping
    let (data, variables) = extract_input_fields(info.input.as_ref());

    WorkflowInstanceDto {
        id: info.instance_id.clone(),
        run_label: info.run_label.clone(),
        completed_at: info.finished_at.map(|t| t.to_rfc3339()),
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
        run_label: info.run_label.clone(),
        completed_at: info.finished_at.map(|t| t.to_rfc3339()),
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

/// Parse an image name (`"workflow_id:version"` or
/// `"workflow_id:version@artifact-fingerprint"`) into `(workflow_id, version)`.
/// Returns `(workflow_id, 0)` if no colon is found.
pub fn parse_image_id(image_id: &str) -> (String, i32) {
    if let Some(pos) = image_id.rfind(':') {
        let workflow_id = image_id[..pos].to_string();
        let version = image_id[pos + 1..]
            .split('@')
            .next()
            .unwrap_or_default()
            .parse::<i32>()
            .unwrap_or(0);
        (workflow_id, version)
    } else {
        (image_id.to_string(), 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn input_flag_row(id: &str) -> WorkflowInstanceDto {
        serde_json::from_value(json!({
            "id": id, "created": "2026-01-01T00:00:00Z", "updated": "2026-01-01T00:00:00Z",
            "status": "suspended", "workflowId": "workflow", "usedVersion": 1, "inputs": {}
        }))
        .unwrap()
    }

    fn input_flag_client(
        persistence: std::sync::Arc<dyn runtara_core::persistence::Persistence>,
    ) -> RuntimeClient {
        use runtara_environment::{handlers::EnvironmentHandlerState, runner::MockRunner};
        use std::sync::Arc;
        let unused_pool = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgresql://localhost:1/unused")
            .unwrap();
        RuntimeClient::new(
            Arc::new(EnvironmentHandlerState::new(
                unused_pool,
                persistence,
                Arc::new(MockRunner::new()),
                std::env::temp_dir(),
            )),
            crate::runtime_client::RuntimeClientConfig::new(Default::default()),
        )
    }

    #[tokio::test]
    async fn pending_input_flags_include_suspended_waits_and_remove_accepted_requests() {
        use runtara_core::{
            domain::InstanceStatus,
            persistence::{Persistence, inputs::*, memory::InMemoryPersistence},
        };
        let persistence = std::sync::Arc::new(InMemoryPersistence::new());
        persistence
            .register_instance("waiting", "tenant")
            .await
            .unwrap();
        persistence
            .update_instance_status("waiting", InstanceStatus::Running, None)
            .await
            .unwrap();
        let spec = InputRequestSpec {
            signal_id: "wait".into(),
            response_schema: None,
            metadata: json!({}),
            deadline: None,
        };
        let inputs = persistence.input_requests().unwrap();
        inputs
            .register_input(
                &InputAuthority::Root {
                    tenant_id: "tenant".into(),
                    instance_id: "waiting".into(),
                },
                &spec,
            )
            .await
            .unwrap();
        persistence
            .update_instance_status("waiting", InstanceStatus::Suspended, None)
            .await
            .unwrap();
        let client = input_flag_client(persistence.clone());
        let mut rows = vec![input_flag_row("waiting")];
        enrich_pending_input(&mut rows, &client, "tenant")
            .await
            .unwrap();
        assert!(rows[0].has_pending_input);
        submit_input(
            inputs,
            "tenant",
            "waiting",
            &spec.request_id(),
            "reply",
            &json!({}),
        )
        .await
        .unwrap();
        enrich_pending_input(&mut rows, &client, "tenant")
            .await
            .unwrap();
        assert!(!rows[0].has_pending_input);
        assert!(
            enrich_pending_input(&mut rows, &client, "foreign")
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn pending_input_flags_propagate_storage_failure_without_mutating_rows() {
        let unavailable_pool = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgresql://localhost:1/unused")
            .unwrap();
        // A closed, never-connected fixture pool deterministically fails IO.
        unavailable_pool.close().await;
        let client = input_flag_client(std::sync::Arc::new(
            runtara_store_postgres::PostgresPersistence::new(unavailable_pool),
        ));
        let mut rows = vec![input_flag_row("waiting"), input_flag_row("other")];
        rows[0].has_pending_input = true;
        let result = enrich_pending_input(&mut rows, &client, "tenant").await;
        assert!(matches!(
            result,
            Err(runtara_core::persistence::inputs::InputError::Storage(_))
        ));
        assert!(rows[0].has_pending_input);
        assert!(!rows[1].has_pending_input);
    }

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
    fn test_parse_image_id_artifact_qualified_format() {
        let (workflow_id, version) =
            parse_image_id("550e8400-e29b-41d4-a716-446655440000:42@f81d4fae7dec11d0a32e");
        assert_eq!(workflow_id, "550e8400-e29b-41d4-a716-446655440000");
        assert_eq!(version, 42);
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
