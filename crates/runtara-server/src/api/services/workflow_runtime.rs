use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use thiserror::Error;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::api::services::pending_inputs::discover_inputs;
use crate::runtime_client::RuntimeClient;
use crate::workers::execution_engine::{ExecutionEngine, ExecutionError};
use runtara_core::persistence::inputs::{InputError, InputReceipt, InputRequest};

#[derive(Debug, Error)]
pub enum WorkflowRuntimeError {
    #[error(transparent)]
    Managed(#[from] InputError),
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

impl WorkflowRuntimeError {
    /// Stable external errors; never serialize storage details or execution fences.
    pub fn public_error(&self) -> (axum::http::StatusCode, InputSubmissionCode, String) {
        use axum::http::StatusCode;
        let (status, code, message) = match self {
            Self::Managed(InputError::NotFound) | Self::NotFound(_) => (
                StatusCode::NOT_FOUND,
                InputSubmissionCode::InputNotFound,
                "Input request not found".into(),
            ),
            Self::Managed(InputError::InvalidRequest) | Self::InvalidRequest(_) => (
                StatusCode::BAD_REQUEST,
                InputSubmissionCode::InputInvalidRequest,
                "Invalid input request".into(),
            ),
            Self::Managed(InputError::InvalidPayload(message)) => (
                StatusCode::BAD_REQUEST,
                InputSubmissionCode::InputInvalidPayload,
                message.clone(),
            ),
            Self::Managed(InputError::Inactive) | Self::Conflict(_) => (
                StatusCode::CONFLICT,
                InputSubmissionCode::InputInactive,
                "Input request is no longer active".into(),
            ),
            Self::Managed(InputError::AlreadyAnswered) => (
                StatusCode::CONFLICT,
                InputSubmissionCode::InputAlreadyAnswered,
                "Input request is already answered".into(),
            ),
            Self::Managed(InputError::OperationConflict) => (
                StatusCode::CONFLICT,
                InputSubmissionCode::InputOperationConflict,
                "Operation ID was used for a different request or payload".into(),
            ),
            _ => (
                StatusCode::SERVICE_UNAVAILABLE,
                InputSubmissionCode::InputUnavailable,
                "Input service unavailable; retry with the same operation ID and payload".into(),
            ),
        };
        (status, code, message)
    }
}

/// Stable machine-readable reasons for managed submission failures.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum InputSubmissionCode {
    InputNotFound,
    InputInvalidRequest,
    InputInvalidPayload,
    InputInactive,
    InputAlreadyAnswered,
    InputOperationConflict,
    InputUnavailable,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct InputSubmissionErrorResponse {
    pub success: bool,
    pub code: InputSubmissionCode,
    pub message: String,
    pub data: Option<Value>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowRuntimeAction {
    pub id: String,
    pub action_id: String,
    pub request_id: String,
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
    pub request_id: String,
    pub operation_id: String,
    pub payload: Value,
}

/// Public acknowledgement deliberately omits the accepted response payload.
#[derive(Debug, Clone, Serialize, ToSchema, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowActionReceipt {
    pub receipt_id: String,
    pub request_id: String,
    pub accepted_at: DateTime<Utc>,
}

impl From<InputReceipt> for WorkflowActionReceipt {
    fn from(receipt: InputReceipt) -> Self {
        Self {
            receipt_id: receipt.receipt_id,
            request_id: receipt.request_id,
            accepted_at: receipt.accepted_at,
        }
    }
}

pub async fn list_instance_actions(
    client: &RuntimeClient,
    tenant_id: &str,
    workflow_id: &str,
    instance_id: &str,
) -> Result<Vec<WorkflowRuntimeAction>, WorkflowRuntimeError> {
    validate_instance_id(instance_id)?;
    let page = discover_inputs(client, tenant_id, &[instance_id.to_owned()], 0, u32::MAX).await?;
    Ok(page
        .requests
        .iter()
        .map(|request| action_from_request(workflow_id, request))
        .collect())
}

/// Stable presentation from immutable registration metadata, also usable when
/// authorizing a retained receipt that is no longer in actionable discovery.
pub fn action_from_request(workflow_id: &str, request: &InputRequest) -> WorkflowRuntimeAction {
    let payload = &request.spec.metadata;
    let step_id = payload.get("step_id").and_then(Value::as_str);
    let step_name = payload.get("step_name").and_then(Value::as_str);
    let tool_name = payload.get("tool_name").and_then(Value::as_str);
    let input_schema = request.spec.response_schema.clone();
    WorkflowRuntimeAction {
        id: request.request_id.clone(),
        action_id: request.request_id.clone(),
        request_id: request.request_id.clone(),
        action_kind: "workflow.signal_response".into(),
        target_kind: "workflow_instance".into(),
        target_id: request.instance_id.clone(),
        workflow_id: workflow_id.into(),
        instance_id: request.instance_id.clone(),
        signal_id: request.spec.signal_id.clone(),
        action_key: payload
            .get("action_key")
            .and_then(Value::as_str)
            .filter(|key| !key.trim().is_empty())
            .map(str::to_owned),
        label: tool_name
            .or(step_name)
            .or_else(|| payload.get("message").and_then(Value::as_str))
            .unwrap_or("Workflow action")
            .into(),
        message: payload
            .get("message")
            .or_else(|| payload.get("step_name"))
            .and_then(Value::as_str)
            .unwrap_or("External input requested")
            .into(),
        schema_format: schema_format(input_schema.as_ref()).into(),
        input_schema,
        status: "open".into(),
        requested_at: Some(request.created_at),
        correlation: payload
            .get("correlation")
            .cloned()
            .unwrap_or_else(|| json!({})),
        context: payload.get("context").cloned().unwrap_or_else(|| json!({})),
        runtime: json!({
            "requestId": request.request_id, "signalId": request.spec.signal_id,
            "stepId": step_id, "stepName": step_name, "toolName": tool_name,
            "aiAgentStepId": payload.get("ai_agent_step_id"),
            "iteration": payload.get("iteration"), "callNumber": payload.get("call_number"),
        }),
    }
}

pub async fn list_workflow_actions(
    _engine: &ExecutionEngine,
    client: &RuntimeClient,
    tenant_id: &str,
    workflow_id: &str,
    page: Option<i32>,
    size: Option<i32>,
) -> Result<WorkflowRuntimeActionPage, WorkflowRuntimeError> {
    let page_number = crate::api::utils::pagination::normalize_page(page);
    let page_size = size.unwrap_or(25).clamp(1, 100);
    let offset = i64::from(page_number) * i64::from(page_size);
    let instances = client
        .workflow_input_instances(tenant_id, workflow_id)
        .await?;
    let found = discover_inputs(
        client,
        tenant_id,
        &instances,
        offset as u64,
        page_size as u32,
    )
    .await?;
    Ok(WorkflowRuntimeActionPage {
        workflow_id: workflow_id.into(),
        page: WorkflowRuntimeActionPageInfo {
            offset,
            size: i64::from(page_size),
            total_count: found.total_count as i64,
            has_next_page: (offset as u64).saturating_add(u64::from(page_size as u32))
                < found.total_count,
        },
        actions: found
            .requests
            .iter()
            .map(|request| action_from_request(workflow_id, request))
            .collect(),
    })
}

#[allow(clippy::too_many_arguments)]
pub async fn submit_workflow_action(
    engine: &ExecutionEngine,
    client: &RuntimeClient,
    tenant_id: &str,
    workflow_id: &str,
    instance_id: &str,
    request_id: &str,
    operation_id: &str,
    payload: &Value,
) -> Result<WorkflowActionReceipt, WorkflowRuntimeError> {
    validate_instance_id(instance_id)?;
    engine
        .authorize_execution(workflow_id, instance_id, tenant_id)
        .await
        .map_err(map_execution_error)?;
    Ok(client
        .submit_input_response(tenant_id, instance_id, request_id, operation_id, payload)
        .await?
        .into())
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

pub(crate) fn map_execution_error(error: ExecutionError) -> WorkflowRuntimeError {
    match error {
        ExecutionError::ValidationError(message) => WorkflowRuntimeError::InvalidRequest(message),
        ExecutionError::NotFound(message) | ExecutionError::WorkflowNotFound(message) => {
            WorkflowRuntimeError::NotFound(message)
        }
        ExecutionError::NotConnected(_) => WorkflowRuntimeError::RuntimeUnavailable,
        _ => WorkflowRuntimeError::Runtime(error.to_string()),
    }
}
