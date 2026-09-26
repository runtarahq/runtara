//! Authoritative managed-input discovery, independent of debug event history.

use serde_json::Value;

use crate::runtime_client::RuntimeClient;
use chrono::{DateTime, Utc};
use serde::Serialize;
use utoipa::ToSchema;

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PendingInputResponse {
    pub request_id: String,
    /// Diagnostic wait address; submissions use `request_id`.
    pub signal_id: String,
    pub tool_name: Option<String>,
    pub message: String,
    pub response_schema: Option<Value>,
    pub ai_agent_step_id: Option<String>,
    pub iteration: Option<u64>,
    pub call_number: Option<u64>,
    pub requested_at: DateTime<Utc>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PendingInputPage {
    pub instance_id: String,
    pub pending_inputs: Vec<PendingInputResponse>,
    pub count: usize,
}

pub async fn pending_input_page(
    client: &RuntimeClient,
    tenant: &str,
    instance: &str,
) -> runtara_core::persistence::inputs::InputResult<PendingInputPage> {
    let page = discover_inputs(client, tenant, &[instance.to_owned()], 0, u32::MAX).await?;
    let pending_inputs: Vec<_> = page
        .requests
        .into_iter()
        .map(|request| {
            let metadata = &request.spec.metadata;
            PendingInputResponse {
                request_id: request.request_id,
                signal_id: request.spec.signal_id,
                tool_name: metadata
                    .get("tool_name")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                message: metadata
                    .get("message")
                    .or_else(|| metadata.get("step_name"))
                    .and_then(Value::as_str)
                    .unwrap_or("External input requested")
                    .into(),
                response_schema: request.spec.response_schema,
                ai_agent_step_id: metadata
                    .get("ai_agent_step_id")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                iteration: metadata.get("iteration").and_then(Value::as_u64),
                call_number: metadata.get("call_number").and_then(Value::as_u64),
                requested_at: request.created_at,
            }
        })
        .collect();
    Ok(PendingInputPage {
        instance_id: instance.into(),
        count: pending_inputs.len(),
        pending_inputs,
    })
}

/// Authoritative actionable discovery. Callers establish any additional
/// workflow/report scope; persistence verifies every root's tenant ownership.
pub async fn discover_inputs(
    client: &RuntimeClient,
    tenant: &str,
    instances: &[String],
    offset: u64,
    limit: u32,
) -> runtara_core::persistence::inputs::InputResult<
    runtara_core::persistence::inputs::InputRequestPage,
> {
    client
        .list_input_requests(tenant, instances, offset, limit)
        .await
}
