/// Execution-related DTOs for listing all executions
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

/// Query parameters for listing all executions
#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ListAllExecutionsQuery {
    /// Page number (0-based, default: 0)
    #[serde(default)]
    pub page: Option<i32>,

    /// Page size (default: 20, max: 100)
    #[serde(default)]
    pub size: Option<i32>,

    /// Filter by scenario ID
    #[serde(rename = "scenarioId")]
    pub scenario_id: Option<String>,

    /// Filter by status (comma-separated, lowercase: queued,completed,failed,running,compiling,timeout,cancelled)
    pub status: Option<String>,

    /// Filter by created date - from (inclusive, ISO 8601)
    #[serde(rename = "createdFrom")]
    pub created_from: Option<DateTime<Utc>>,

    /// Filter by created date - to (inclusive, ISO 8601)
    #[serde(rename = "createdTo")]
    pub created_to: Option<DateTime<Utc>>,

    /// Filter by completed date - from (inclusive, ISO 8601)
    #[serde(rename = "completedFrom")]
    pub completed_from: Option<DateTime<Utc>>,

    /// Filter by completed date - to (inclusive, ISO 8601)
    #[serde(rename = "completedTo")]
    pub completed_to: Option<DateTime<Utc>>,

    /// Sort by field (default: completedAt). Options: createdAt, completedAt, status, scenarioId
    #[serde(rename = "sortBy")]
    pub sort_by: Option<String>,

    /// Sort order (default: desc). Options: asc, desc
    #[serde(rename = "sortOrder")]
    pub sort_order: Option<String>,
}

/// Response for listing all executions
#[derive(Debug, Serialize, ToSchema)]
pub struct ListAllExecutionsResponse {
    pub success: bool,
    pub data: super::scenarios::PageScenarioInstanceHistoryDto,
}

/// Filter parameters passed to repository
#[derive(Debug, Clone)]
pub struct ExecutionFilters {
    pub scenario_id: Option<String>,
    pub statuses: Option<Vec<String>>,
    pub created_from: Option<DateTime<Utc>>,
    pub created_to: Option<DateTime<Utc>>,
    pub completed_from: Option<DateTime<Utc>>,
    pub completed_to: Option<DateTime<Utc>>,
    pub sort_by: String,
    pub sort_order: String,
}

impl Default for ExecutionFilters {
    fn default() -> Self {
        Self {
            scenario_id: None,
            statuses: None,
            created_from: None,
            created_to: None,
            completed_from: None,
            completed_to: None,
            sort_by: "completed_at".to_string(),
            sort_order: "DESC".to_string(),
        }
    }
}
