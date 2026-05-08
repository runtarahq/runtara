use std::collections::HashSet;

use chrono::{DateTime, Utc};
use runtara_management_sdk::{EventSortOrder, ListEventsOptions};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use thiserror::Error;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::api::services::input_validation::{is_empty_schema, validate_inputs};
use crate::runtime_client::RuntimeClient;
use crate::workers::execution_engine::{ExecutionEngine, ExecutionError};

#[derive(Debug, Error)]
pub enum WorkflowRuntimeError {
    #[error("{0}")]
    InvalidRequest(String),
    #[error("{0}")]
    NotFound(String),
    #[error("{0}")]
    Conflict(String),
    #[error("Runtime client not configured")]
    RuntimeUnavailable,
    #[error("{0}")]
    Runtime(String),
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowRuntimeAction {
    pub id: String,
    pub action_id: String,
    pub action_kind: String,
    pub target_kind: String,
    pub target_id: String,
    pub workflow_id: String,
    pub instance_id: String,
    pub signal_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub action_key: Option<String>,
    pub label: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_schema: Option<Value>,
    pub schema_format: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requested_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub correlation: Value,
    #[serde(default)]
    pub context: Value,
    pub runtime: Value,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowRuntimeActionPage {
    pub workflow_id: String,
    pub actions: Vec<WorkflowRuntimeAction>,
    pub page: WorkflowRuntimeActionPageInfo,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowRuntimeActionPageInfo {
    pub offset: i64,
    pub size: i64,
    pub total_count: i64,
    pub has_next_page: bool,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SubmitWorkflowActionRequest {
    #[serde(default)]
    pub payload: Value,
}

pub async fn list_instance_actions(
    client: &RuntimeClient,
    workflow_id: &str,
    instance_id: &str,
) -> Result<Vec<WorkflowRuntimeAction>, WorkflowRuntimeError> {
    validate_instance_id(instance_id)?;

    let input_options = ListEventsOptions::new()
        .with_limit(100)
        .with_event_type("custom")
        .with_subtype("external_input_requested")
        .with_sort_order(EventSortOrder::Asc);

    let input_events = client
        .list_events(instance_id, Some(input_options))
        .await
        .map_err(|error| map_runtime_error(error.to_string(), instance_id))?
        .events;

    let end_options = ListEventsOptions::new()
        .with_limit(1000)
        .with_event_type("custom")
        .with_subtype("step_debug_end");

    let end_events = client
        .list_events(instance_id, Some(end_options))
        .await
        .map(|result| result.events)
        .unwrap_or_default();

    let completed_step_ids: HashSet<String> = end_events
        .iter()
        .filter_map(|event| {
            event
                .payload
                .as_ref()
                .and_then(|payload| payload.get("step_id"))
                .and_then(Value::as_str)
                .map(ToString::to_string)
        })
        .collect();

    let actions = input_events
        .iter()
        .filter_map(|event| {
            let payload = event.payload.as_ref()?;
            let signal_id = payload.get("signal_id")?.as_str()?.to_string();
            let ai_agent_step_id = payload.get("ai_agent_step_id").and_then(Value::as_str);
            let tool_name = payload.get("tool_name").and_then(Value::as_str);
            let step_id = payload.get("step_id").and_then(Value::as_str);
            let step_name = payload.get("step_name").and_then(Value::as_str);
            let call_number = payload
                .get("call_number")
                .and_then(Value::as_u64)
                .map(|value| value as u32);

            let check_step_id = match (ai_agent_step_id, tool_name, call_number) {
                (Some(step), Some(tool), Some(number)) => {
                    format!("{}.tool.{}.{}", step, tool, number)
                }
                _ => step_id.unwrap_or_default().to_string(),
            };

            if !check_step_id.is_empty() && completed_step_ids.contains(&check_step_id) {
                return None;
            }

            let input_schema = payload.get("response_schema").cloned();
            let action_key = payload
                .get("action_key")
                .and_then(Value::as_str)
                .filter(|value| !value.trim().is_empty())
                .map(ToString::to_string);
            let correlation = payload
                .get("correlation")
                .cloned()
                .unwrap_or_else(|| json!({}));
            let context = payload.get("context").cloned().unwrap_or_else(|| json!({}));
            let label = tool_name
                .or(step_name)
                .or_else(|| payload.get("message").and_then(Value::as_str))
                .unwrap_or("Workflow action")
                .to_string();
            let message = payload
                .get("message")
                .or_else(|| payload.get("step_name"))
                .and_then(Value::as_str)
                .unwrap_or("External input requested")
                .to_string();

            Some(WorkflowRuntimeAction {
                id: signal_id.clone(),
                action_id: signal_id.clone(),
                action_kind: "workflow.signal_response".to_string(),
                target_kind: "workflow_instance".to_string(),
                target_id: instance_id.to_string(),
                workflow_id: workflow_id.to_string(),
                instance_id: instance_id.to_string(),
                signal_id,
                action_key,
                label,
                message,
                schema_format: schema_format(input_schema.as_ref()).to_string(),
                input_schema,
                status: "open".to_string(),
                requested_at: Some(event.created_at),
                correlation,
                context,
                runtime: json!({
                    "signalId": payload.get("signal_id"),
                    "stepId": step_id,
                    "stepName": step_name,
                    "toolName": tool_name,
                    "aiAgentStepId": ai_agent_step_id,
                    "iteration": payload.get("iteration"),
                    "callNumber": call_number,
                }),
            })
        })
        .collect();

    Ok(actions)
}

pub async fn list_workflow_actions(
    engine: &ExecutionEngine,
    client: &RuntimeClient,
    tenant_id: &str,
    workflow_id: &str,
    page: Option<i32>,
    size: Option<i32>,
) -> Result<WorkflowRuntimeActionPage, WorkflowRuntimeError> {
    let page_number = page.unwrap_or(0).max(0);
    let page_size = size.unwrap_or(25).clamp(1, 100);
    let instances = engine
        .list_executions(tenant_id, workflow_id, Some(page_number), Some(page_size))
        .await
        .map_err(map_execution_error)?;

    let mut actions = Vec::new();
    for instance in &instances.content {
        if !instance.has_pending_input {
            continue;
        }
        actions.extend(list_instance_actions(client, workflow_id, &instance.id).await?);
    }

    Ok(WorkflowRuntimeActionPage {
        workflow_id: workflow_id.to_string(),
        page: WorkflowRuntimeActionPageInfo {
            offset: (page_number * page_size) as i64,
            size: page_size as i64,
            total_count: actions.len() as i64,
            has_next_page: !instances.last,
        },
        actions,
    })
}

pub async fn submit_workflow_action(
    engine: &ExecutionEngine,
    client: &RuntimeClient,
    tenant_id: &str,
    workflow_id: &str,
    instance_id: &str,
    action_id: &str,
    payload: &Value,
) -> Result<WorkflowRuntimeAction, WorkflowRuntimeError> {
    validate_instance_id(instance_id)?;
    if action_id.trim().is_empty() {
        return Err(WorkflowRuntimeError::InvalidRequest(
            "actionId is required".to_string(),
        ));
    }

    let execution = engine
        .get_execution_with_metadata(workflow_id, instance_id, tenant_id)
        .await
        .map_err(map_execution_error)?;
    if execution.instance.status.is_terminal() || !execution.instance.has_pending_input {
        return Err(WorkflowRuntimeError::Conflict(format!(
            "Action '{}' is no longer open for instance '{}'",
            action_id, instance_id
        )));
    }

    let actions = list_instance_actions(client, workflow_id, instance_id).await?;
    let action = actions
        .into_iter()
        .find(|action| action.action_id == action_id || action.signal_id == action_id)
        .ok_or_else(|| {
            WorkflowRuntimeError::Conflict(format!(
                "Action '{}' is no longer open for instance '{}'",
                action_id, instance_id
            ))
        })?;

    if let Some(schema) = action
        .input_schema
        .as_ref()
        .filter(|schema| !is_empty_schema(schema))
    {
        validate_inputs(payload, schema).map_err(|message| {
            WorkflowRuntimeError::InvalidRequest(format!("Action payload is invalid: {}", message))
        })?;
    }

    let payload_bytes = serde_json::to_vec(payload).map_err(|error| {
        WorkflowRuntimeError::InvalidRequest(format!("Failed to serialize payload: {}", error))
    })?;

    client
        .send_custom_signal(instance_id, &action.signal_id, Some(&payload_bytes))
        .await
        .map_err(|error| map_runtime_error(error.to_string(), instance_id))?;

    Ok(action)
}

fn validate_instance_id(instance_id: &str) -> Result<(), WorkflowRuntimeError> {
    Uuid::parse_str(instance_id).map_err(|_| {
        WorkflowRuntimeError::InvalidRequest(
            "Invalid instance ID format. Instance ID must be a valid UUID".to_string(),
        )
    })?;
    Ok(())
}

fn schema_format(schema: Option<&Value>) -> &'static str {
    if schema
        .and_then(Value::as_object)
        .is_some_and(|object| object.contains_key("properties"))
    {
        "json_schema"
    } else {
        "runtara_schema_field_map"
    }
}

fn map_execution_error(error: ExecutionError) -> WorkflowRuntimeError {
    match error {
        ExecutionError::ValidationError(message) => WorkflowRuntimeError::InvalidRequest(message),
        ExecutionError::NotFound(message) | ExecutionError::WorkflowNotFound(message) => {
            WorkflowRuntimeError::NotFound(message)
        }
        ExecutionError::NotConnected(_) => WorkflowRuntimeError::RuntimeUnavailable,
        _ => WorkflowRuntimeError::Runtime(error.to_string()),
    }
}

fn map_runtime_error(message: String, instance_id: &str) -> WorkflowRuntimeError {
    if message.contains("not found") || message.contains("InstanceNotFound") {
        WorkflowRuntimeError::NotFound(format!(
            "Instance '{}' was not found: {}",
            instance_id, message
        ))
    } else {
        WorkflowRuntimeError::Runtime(message)
    }
}
