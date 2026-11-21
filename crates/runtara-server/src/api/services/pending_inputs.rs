//! Authoritative managed-input discovery, independent of debug event history.

use serde_json::Value;

use crate::runtime_client::RuntimeClient;
use chrono::{DateTime, Utc};
use serde::Serialize;
use utoipa::ToSchema;

#[derive(Debug, Serialize, ToSchema, serde::Deserialize)]
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
    /// The queue a wait step's `action.key` names, when it gives one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action_key: Option<String>,
    /// What the wait step's `action.context` says for whoever answers it,
    /// such as the tool a builder proposes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<Value>,
}

#[derive(Debug, Serialize, ToSchema, serde::Deserialize)]
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
    let pending_inputs: Vec<_> = page.requests.into_iter().map(pending_input).collect();
    Ok(PendingInputPage {
        instance_id: instance.into(),
        count: pending_inputs.len(),
        pending_inputs,
    })
}

/// One open request as a caller sees it: what it asks, and what its wait
/// step's action says (`action.key`, `action.context`) for whoever answers.
fn pending_input(request: runtara_core::persistence::inputs::InputRequest) -> PendingInputResponse {
    let metadata = &request.spec.metadata;
    PendingInputResponse {
        action_key: metadata
            .get("action_key")
            .and_then(Value::as_str)
            .map(str::to_owned),
        context: metadata.get("context").filter(|c| !c.is_null()).cloned(),
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

#[cfg(test)]
mod tests {
    use super::*;
    use runtara_core::persistence::inputs::{InputRequest, InputRequestSpec, InputState};
    use serde_json::json;

    fn request(metadata: Value) -> InputRequest {
        InputRequest {
            tenant_id: "tenant".into(),
            instance_id: "instance".into(),
            request_id: "request".into(),
            spec: InputRequestSpec {
                signal_id: "signal".into(),
                response_schema: Some(json!({"approved": {"type": "boolean"}})),
                metadata,
                deadline: None,
            },
            created_at: Utc::now(),
            state: InputState::Open,
            wake_pending: false,
        }
    }

    /// A caller answering over MCP sees what the wait step put on its action
    /// for whoever answers it, as the console does.
    #[test]
    fn a_pending_input_carries_its_action_key_and_context() {
        let input = pending_input(request(json!({
            "step_name": "Approve the new tool",
            "action_key": "tool_builder_approval",
            "context": {"tool_name": "slugify", "agent_id": "text"},
        })));
        assert_eq!(input.request_id, "request");
        assert_eq!(input.message, "Approve the new tool");
        assert_eq!(input.action_key.as_deref(), Some("tool_builder_approval"));
        assert_eq!(
            input.context,
            Some(json!({"tool_name": "slugify", "agent_id": "text"}))
        );

        let plain = pending_input(request(json!({"message": "Continue?", "context": null})));
        assert_eq!(plain.action_key, None);
        assert_eq!(plain.context, None);
        let wire = serde_json::to_value(&plain).unwrap();
        assert!(wire.get("context").is_none() && wire.get("actionKey").is_none());
    }
}
