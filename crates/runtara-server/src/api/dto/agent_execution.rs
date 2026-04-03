//! Agent Execution DTOs
//!
//! Request/response types for host-mediated agent capability execution.
//! Used by scenario instances to delegate I/O agent work to the host.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::ToSchema;

/// Request body for executing an agent capability on the host
#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[schema(example = json!({
    "inputs": {"url": "https://api.example.com/data", "method": "GET"},
    "connectionId": "conn_shopify_main",
    "instanceId": "inst-abc-123",
    "tenantId": "org_p0IkAFnrVqVOvQw9"
}))]
pub struct ExecuteAgentRequest {
    /// Agent-specific input data (structure depends on the agent/capability).
    pub inputs: Value,

    /// Optional connection ID for agents that require credentials.
    /// The host resolves the connection and injects it as `_connection` in agent input.
    #[serde(
        rename = "connectionId",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub connection_id: Option<String>,

    /// Instance ID of the calling scenario (for tracing/logging).
    #[serde(
        rename = "instanceId",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub instance_id: Option<String>,

    /// Tenant ID of the calling scenario.
    /// Used as fallback if not available from auth context.
    #[serde(rename = "tenantId", default, skip_serializing_if = "Option::is_none")]
    pub tenant_id: Option<String>,
}

/// Successful response from agent execution
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ExecuteAgentResponse {
    /// Whether the agent executed successfully
    pub success: bool,

    /// Agent output (present on success)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<Value>,

    /// Error message (present on failure)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,

    /// Execution time in milliseconds
    #[serde(rename = "executionTimeMs")]
    pub execution_time_ms: f64,
}

/// Error response from agent execution
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ExecuteAgentErrorResponse {
    pub success: bool,
    pub error: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}
