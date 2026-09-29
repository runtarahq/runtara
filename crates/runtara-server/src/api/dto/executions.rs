/// Execution-related DTOs for listing all executions
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

/// Query parameters for listing all executions
#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ListAllExecutionsQuery {
    /// Case-insensitive literal substring search across execution labels and metadata.
    pub search: Option<String>,
    /// Exact execution label (duplicates are returned).
    #[serde(rename = "runLabel")]
    pub run_label: Option<String>,
    /// Only the children of this run: the executions its `control:start`
    /// steps started.
    #[serde(default, rename = "parentInstanceId")]
    pub parent_instance_id: Option<String>,
    /// Page number (0-based, default: 0)
    #[serde(default)]
    pub page: Option<i32>,

    /// Page size (default: 20, max: 100)
    #[serde(default)]
    pub size: Option<i32>,

    /// Filter by workflow ID
    #[serde(rename = "workflowId")]
    pub workflow_id: Option<String>,

    /// Filter by status. Comma-separated, lowercase; an execution matches if it
    /// holds any one of them (queued, compiling, running, suspended, completed,
    /// failed, timeout, cancelled).
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

    /// Sort by field (default: completedAt). Options: createdAt, completedAt, status, workflowId
    #[serde(rename = "sortBy")]
    pub sort_by: Option<String>,

    /// Sort order (default: desc). Options: asc, desc
    #[serde(rename = "sortOrder")]
    pub sort_order: Option<String>,
}

/// A filter on a run's published state (what its SetState steps wrote).
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct StateFilterDto {
    /// A top-level state field.
    pub field: String,
    /// `eq`, `ne`, `in`, `lt`, `lte`, `gt`, `gte` or `exists`.
    pub op: String,
    /// A JSON string, number or boolean; an array of them for `in`; `true` or
    /// `false` for `exists` (default `true`). A date-time string compares in
    /// UTC. There is no `now`: pass the time you mean.
    #[serde(default)]
    #[schema(value_type = Object)]
    pub value: serde_json::Value,
}

/// Body of `POST /api/runtime/executions/query`: the listing filters of
/// `GET /api/runtime/executions`, plus filters on published state. Returns
/// executions, optionally projecting selected state fields.
#[derive(Debug, Default, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct QueryExecutionsRequest {
    pub search: Option<String>,
    pub run_label: Option<String>,
    pub parent_instance_id: Option<String>,
    /// Page number (0-based, default 0).
    pub page: Option<i32>,
    /// Page size (default 20, max 100).
    pub size: Option<i32>,
    pub workflow_id: Option<String>,
    /// Comma-separated statuses, as for the GET listing.
    pub status: Option<String>,
    pub created_from: Option<DateTime<Utc>>,
    pub created_to: Option<DateTime<Utc>>,
    pub completed_from: Option<DateTime<Utc>>,
    pub completed_to: Option<DateTime<Utc>>,
    pub sort_by: Option<String>,
    pub sort_order: Option<String>,
    /// All must hold; a run without the field does not match. At most 16.
    #[serde(default)]
    pub state: Vec<StateFilterDto>,
    /// Explicit top-level state projection (maximum 32); omitted returns no state.
    #[serde(default)]
    pub state_fields: Vec<String>,
    /// Optional typed state ordering, with missing values last.
    pub state_sort: Option<StateSortDto>,
}

impl QueryExecutionsRequest {
    /// The listing part, as the GET query.
    pub fn listing(&self) -> ListAllExecutionsQuery {
        ListAllExecutionsQuery {
            search: self.search.clone(),
            run_label: self.run_label.clone(),
            parent_instance_id: self.parent_instance_id.clone(),
            page: self.page,
            size: self.size,
            workflow_id: self.workflow_id.clone(),
            status: self.status.clone(),
            created_from: self.created_from,
            created_to: self.created_to,
            completed_from: self.completed_from,
            completed_to: self.completed_to,
            sort_by: self.sort_by.clone(),
            sort_order: self.sort_order.clone(),
        }
    }
}

/// Response for listing all executions
#[derive(Debug, Serialize, ToSchema)]
pub struct ListAllExecutionsResponse {
    pub success: bool,
    pub data: super::workflows::PageWorkflowInstanceHistoryDto,
}

/// Filter parameters passed to repository
#[derive(Debug, Clone)]
pub struct ExecutionFilters {
    pub search: Option<String>,
    pub run_label: Option<String>,
    /// Only the children of this run (`control:start`).
    pub parent_instance_id: Option<String>,
    pub workflow_id: Option<String>,
    pub statuses: Option<Vec<String>>,
    pub created_from: Option<DateTime<Utc>>,
    pub created_to: Option<DateTime<Utc>>,
    pub completed_from: Option<DateTime<Utc>>,
    pub completed_to: Option<DateTime<Utc>>,
    pub sort_by: String,
    pub sort_order: String,
    /// Published-state filters, validated and canonical.
    pub state_filters: Vec<runtara_environment::state_filter::StateFilter>,
    pub state_fields: Vec<String>,
    pub state_sort: Option<runtara_environment::operations::StateSort>,
}

impl Default for ExecutionFilters {
    fn default() -> Self {
        Self {
            search: None,
            run_label: None,
            parent_instance_id: None,
            workflow_id: None,
            statuses: None,
            created_from: None,
            created_to: None,
            completed_from: None,
            completed_to: None,
            sort_by: "completed_at".to_string(),
            sort_order: "DESC".to_string(),
            state_filters: Vec::new(),
            state_fields: Vec::new(),
            state_sort: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StateSortDto {
    pub field: String,
    #[serde(default)]
    pub descending: bool,
}

impl From<&StateSortDto> for runtara_environment::operations::StateSort {
    fn from(value: &StateSortDto) -> Self {
        Self {
            field: value.field.clone(),
            descending: value.descending,
        }
    }
}

/// Non-status predicates for status totals. Counts deliberately ignore pagination.
#[derive(Debug, Default, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExecutionSummaryRequest {
    pub search: Option<String>,
    pub run_label: Option<String>,
    pub parent_instance_id: Option<String>,
    pub workflow_id: Option<String>,
    pub created_from: Option<DateTime<Utc>>,
    pub created_to: Option<DateTime<Utc>>,
    pub completed_from: Option<DateTime<Utc>>,
    pub completed_to: Option<DateTime<Utc>>,
}

impl ExecutionSummaryRequest {
    pub fn listing(self) -> QueryExecutionsRequest {
        QueryExecutionsRequest {
            search: self.search,
            run_label: self.run_label,
            parent_instance_id: self.parent_instance_id,
            workflow_id: self.workflow_id,
            created_from: self.created_from,
            created_to: self.created_to,
            completed_from: self.completed_from,
            completed_to: self.completed_to,
            ..Default::default()
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionSummary {
    pub total: i64,
    /// Counts by displayed status. Filter aliases (compiling/timeout) are not
    /// counted twice: their rows display as queued/failed in execution lists.
    pub counts: std::collections::BTreeMap<String, i64>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ExecutionSummaryResponse {
    pub success: bool,
    pub data: ExecutionSummary,
}

#[cfg(test)]
mod summary_tests {
    use super::*;
    #[test]
    fn summary_rejects_status_and_pagination_instead_of_silently_changing_totals() {
        for key in ["status", "page", "size", "sortBy", "unexpected"] {
            assert!(
                serde_json::from_value::<ExecutionSummaryRequest>(serde_json::json!({key: "x"}))
                    .is_err()
            );
        }
        assert!(
            serde_json::from_value::<ExecutionSummaryRequest>(
                serde_json::json!({"search":"order", "workflowId":"workflow"})
            )
            .is_ok()
        );
    }
}
