use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::ToSchema;

use super::executions::QueryExecutionsRequest;

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct QueryOperationRequests {
    pub workflow_id: String,
    pub action_key: String,
    #[serde(default)]
    pub query: QueryExecutionsRequest,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct OperationRequest {
    pub instance_id: String,
    pub workflow_id: String,
    pub used_version: i32,
    pub run_label: Option<String>,
    pub request_id: String,
    pub action_key: String,
    pub requested_at: DateTime<Utc>,
    pub deadline: Option<DateTime<Utc>>,
    pub input_schema: Option<Value>,
    pub context: Value,
    pub label: String,
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<Value>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct OperationRequestPage {
    pub content: Vec<OperationRequest>,
    pub total_elements: i64,
    pub total_pages: i64,
    pub number: i32,
    pub size: i32,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct OperationQueue {
    pub workflow_id: String,
    pub workflow_name: String,
    pub action_key: String,
    pub name: String,
    pub count: i64,
    /// Current version metadata for display. Answer forms always use request schemas.
    pub state_schema: Value,
}

/// Generic presentation choices. No domain-specific types or transformations.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OperationViewConfig {
    pub name: String,
    pub workflow: String,
    #[serde(default)]
    pub columns: Vec<String>,
    #[serde(default, rename = "where")]
    pub filter: OperationViewFilter,
    #[serde(default)]
    pub roles: OperationViewRoles,
    #[serde(default)]
    pub answers: OperationViewAnswers,
    #[serde(default)]
    pub formats: std::collections::BTreeMap<String, OperationDisplayFormat>,
    #[serde(default)]
    pub labels: std::collections::BTreeMap<String, String>,
    pub sort: Option<super::executions::StateSortDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OperationViewFilter {
    pub open_request: Option<String>,
    #[serde(default)]
    pub state: Vec<super::executions::StateFilterDto>,
    pub status: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OperationViewRoles {
    pub key: Option<String>,
    pub stage: Option<String>,
    pub due: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OperationViewAnswers {
    pub inline: Option<String>,
    #[serde(default)]
    pub bulk: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OperationDisplayFormat {
    pub kind: Option<OperationDisplayKind>,
    pub decimals: Option<u8>,
    pub prefix: Option<String>,
    pub suffix: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum OperationDisplayKind {
    Text,
    Number,
    Date,
    Datetime,
    Relative,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SaveOperationView {
    pub configuration: OperationViewConfig,
    /// Required when editing; a stale revision is rejected instead of losing edits.
    pub revision: Option<i32>,
}
#[derive(Debug, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct SavedOperationView {
    pub id: String,
    #[sqlx(json)]
    pub configuration: OperationViewConfig,
    pub revision: i32,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct OperationErrorSummary {
    pub code: Option<String>,
    pub category: Option<String>,
    pub message: String,
    pub retryable: Option<bool>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct OperationProcess {
    pub workflow_id: String,
    pub name: String,
    pub state_schema: Value,
}
