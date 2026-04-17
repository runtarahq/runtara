//! Conversions between Runtara SDK types and local DTOs.
//!
//! Helpers for translating between `runtara-management-sdk` instance
//! representations and the server-side `ScenarioInstanceDto`
//! / `ExecutionWithMetadata` shapes used by HTTP handlers.
//!
//! Previously housed inside `api/services/executions.rs`; extracted so the
//! shared `ExecutionEngine` can use them without pulling in the legacy
//! service.

use chrono::{DateTime, Utc};
use runtara_management_sdk::{
    EventSortOrder, InstanceInfo, InstanceStatus as RuntaraInstanceStatus, ListEventsOptions,
};
use serde_json::Value;

use crate::api::dto::scenarios::{InstanceInputs, ScenarioInstanceDto};
use crate::runtime_client::RuntimeClient;
use crate::types::ExecutionStatus;

/// Extended execution data with metadata from the scenario record.
///
/// Used when fetching a single execution with full details.
#[derive(Debug)]
pub struct ExecutionWithMetadata {
    pub instance: ScenarioInstanceDto,
    pub scenario_name: Option<String>,
    pub scenario_description: Option<String>,
    pub worker_id: Option<String>,
    pub heartbeat_at: Option<DateTime<Utc>>,
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

/// Enrich running instances with `has_pending_input` by checking for unresolved
/// `external_input_requested` events. Only queries events for running instances.
pub async fn enrich_pending_input(instances: &mut [ScenarioInstanceDto], client: &RuntimeClient) {
    for instance in instances.iter_mut() {
        if instance.status != ExecutionStatus::Running {
            continue;
        }

        // Check for external_input_requested events
        let options = ListEventsOptions::new()
            .with_limit(1)
            .with_event_type("custom")
            .with_subtype("external_input_requested")
            .with_sort_order(EventSortOrder::Desc);

        match client.list_events(&instance.id, Some(options)).await {
            Ok(result) if !result.events.is_empty() => {
                // Found at least one external_input_requested event.
                // Check if the most recent one has been resolved by looking for
                // a corresponding step_debug_end event (covers both
                // AiAgentToolCall and standalone WaitForSignal).
                let end_options = ListEventsOptions::new()
                    .with_limit(1)
                    .with_event_type("custom")
                    .with_subtype("step_debug_end")
                    .with_sort_order(EventSortOrder::Desc);

                let has_completion = match client.list_events(&instance.id, Some(end_options)).await
                {
                    Ok(end_result) => {
                        // If the latest end event is newer than the latest request,
                        // the most recent request has been resolved
                        if let (Some(req), Some(end)) =
                            (result.events.first(), end_result.events.first())
                        {
                            end.created_at >= req.created_at
                        } else {
                            false
                        }
                    }
                    Err(_) => false,
                };

                instance.has_pending_input = !has_completion;
            }
            _ => {}
        }
    }
}

/// Convert Runtara `InstanceSummary` to `ScenarioInstanceDto` with scenario
/// info from a database lookup.
pub fn runtara_instance_to_dto_with_info(
    inst: runtara_management_sdk::InstanceSummary,
    scenario_id: String,
    version: i32,
    scenario_name: Option<String>,
) -> ScenarioInstanceDto {
    // Convert to execution status
    let status = runtara_status_to_execution_status(inst.status);

    // Calculate execution duration if available
    let execution_duration_seconds = inst.started_at.and_then(|start| {
        inst.finished_at
            .map(|end| (end - start).num_milliseconds() as f64 / 1000.0)
    });

    ScenarioInstanceDto {
        id: inst.instance_id.clone(),
        created: inst.created_at.to_rfc3339(),
        updated: inst
            .finished_at
            .map(|t| t.to_rfc3339())
            .unwrap_or_else(|| inst.created_at.to_rfc3339()),
        status,
        termination_type: None, // Not available from Runtara summary
        scenario_id,
        scenario_name,
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

/// Convert Runtara `InstanceInfo` (detailed) to `ScenarioInstanceDto`.
pub fn runtara_info_to_dto(info: InstanceInfo) -> ScenarioInstanceDto {
    // Convert to execution status
    let status = runtara_status_to_execution_status(info.status);

    // Calculate execution duration if available
    let execution_duration_seconds = info.started_at.and_then(|start| {
        info.finished_at
            .map(|end| (end - start).num_milliseconds() as f64 / 1000.0)
    });

    let created = info.created_at.to_rfc3339();

    let updated = info
        .finished_at
        .map(|t| t.to_rfc3339())
        .unwrap_or_else(|| created.clone());

    // Extract scenario_id and version from image_name (format: scenario_id:version)
    let (scenario_id, version) = parse_image_id(&info.image_name);

    // Extract data and variables from input to avoid double-wrapping
    let (data, variables) = extract_input_fields(info.input.as_ref());

    ScenarioInstanceDto {
        id: info.instance_id.clone(),
        created,
        updated,
        status,
        termination_type: None,
        scenario_id,
        scenario_name: None,
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
/// Used when enriching a single execution with scenario metadata.
pub fn runtara_info_to_execution_with_metadata(
    info: InstanceInfo,
    scenario_name: Option<String>,
    scenario_description: Option<String>,
) -> ExecutionWithMetadata {
    // Convert to ScenarioInstanceDto first
    let status = runtara_status_to_execution_status(info.status);

    let execution_duration_seconds = info.started_at.and_then(|start| {
        info.finished_at
            .map(|end| (end - start).num_milliseconds() as f64 / 1000.0)
    });

    let created = info.created_at.to_rfc3339();
    let updated = info
        .finished_at
        .map(|t| t.to_rfc3339())
        .unwrap_or_else(|| created.clone());

    let (scenario_id, version) = parse_image_id(&info.image_name);

    // Extract data and variables from input to avoid double-wrapping
    let (data, variables) = extract_input_fields(info.input.as_ref());

    let instance = ScenarioInstanceDto {
        id: info.instance_id.clone(),
        created,
        updated,
        status,
        termination_type: None,
        scenario_id,
        scenario_name: scenario_name.clone(),
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
        scenario_name,
        scenario_description,
        worker_id: None, // Not tracked by Runtara at server level
        heartbeat_at: info.heartbeat_at,
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

/// Parse `image_id` (format: `"scenario_id:version"`) into
/// `(scenario_id, version)`. Returns `(scenario_id, 0)` if no colon is found.
pub fn parse_image_id(image_id: &str) -> (String, i32) {
    if let Some(pos) = image_id.rfind(':') {
        let scenario_id = image_id[..pos].to_string();
        let version = image_id[pos + 1..].parse::<i32>().unwrap_or(0);
        (scenario_id, version)
    } else {
        (image_id.to_string(), 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // =========================================================================
    // parse_image_id tests
    // =========================================================================

    #[test]
    fn test_parse_image_id_standard_format() {
        let (scenario_id, version) = parse_image_id("my-scenario:5");
        assert_eq!(scenario_id, "my-scenario");
        assert_eq!(version, 5);
    }

    #[test]
    fn test_parse_image_id_uuid_format() {
        let (scenario_id, version) = parse_image_id("550e8400-e29b-41d4-a716-446655440000:42");
        assert_eq!(scenario_id, "550e8400-e29b-41d4-a716-446655440000");
        assert_eq!(version, 42);
    }

    #[test]
    fn test_parse_image_id_no_version() {
        let (scenario_id, version) = parse_image_id("scenario-without-version");
        assert_eq!(scenario_id, "scenario-without-version");
        assert_eq!(version, 0);
    }

    #[test]
    fn test_parse_image_id_invalid_version() {
        let (scenario_id, version) = parse_image_id("my-scenario:invalid");
        assert_eq!(scenario_id, "my-scenario");
        assert_eq!(version, 0);
    }

    #[test]
    fn test_parse_image_id_multiple_colons() {
        // Uses rfind so it should parse from the last colon
        let (scenario_id, version) = parse_image_id("org:tenant:scenario:10");
        assert_eq!(scenario_id, "org:tenant:scenario");
        assert_eq!(version, 10);
    }

    #[test]
    fn test_parse_image_id_empty_string() {
        let (scenario_id, version) = parse_image_id("");
        assert_eq!(scenario_id, "");
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
