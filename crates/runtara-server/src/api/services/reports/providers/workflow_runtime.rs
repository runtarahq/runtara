//! Workflow runtime source provider — `Instances` + `Actions` entities.
//! Pulls from the execution engine + runtime client. Aggregates are always
//! virtual (no pushdown); the provider rejects aggregate mode since the
//! legacy validator did the same.

use async_trait::async_trait;
use serde_json::{Map, Value, json};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::api::dto::executions::ExecutionFilters;
use crate::api::dto::object_model::{AggregateRequest, Condition};
use crate::api::dto::reports::*;
use crate::api::dto::workflows::WorkflowInstanceDto;
use crate::api::services::reports::{
    ReportServiceError, condition_matches_row, humanize_label, validate_block_interactions,
    validate_report_condition_field_refs, validate_report_condition_filter_refs,
    validate_report_interaction_buttons, validate_report_source_filter_mappings,
    validate_report_table_action_config, validate_report_table_display_templates,
    validate_report_workflow_action_config, validate_report_workflow_action_context_field,
    validate_report_workflow_action_row_conditions,
};
use crate::api::services::workflow_runtime::{
    WorkflowRuntimeAction, WorkflowRuntimeError, action_from_request, list_instance_actions,
    list_workflow_actions, map_execution_error,
};
use crate::runtime_client::RuntimeClient;
use crate::workers::execution_engine::ExecutionEngine;

use super::{FetchAggregateOutput, FetchParams, FetchRowsOutput, ReportSourceProvider};

pub struct WorkflowRuntimeProvider {
    engine: Option<Arc<ExecutionEngine>>,
    runtime_client: Option<Arc<RuntimeClient>>,
}

impl WorkflowRuntimeProvider {
    /// Re-authorize report scope from retained immutable request metadata. An
    /// accepted request need not remain in the current actionable result set.
    pub async fn retained_action_for_block_context(
        &self,
        tenant_id: &str,
        block: &ReportBlockDefinition,
        condition: Option<&Condition>,
        instance_id: &str,
        request_id: &str,
    ) -> Result<WorkflowRuntimeAction, ReportServiceError> {
        let workflow_id = workflow_runtime_workflow_id(block)?;
        let not_found = || {
            WorkflowRuntimeError::Managed(runtara_core::persistence::inputs::InputError::NotFound)
        };
        if block
            .source
            .instance_id
            .as_deref()
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .is_some_and(|id| id != instance_id)
        {
            return Err(not_found().into());
        }
        self.engine()?
            .authorize_execution(workflow_id, instance_id, tenant_id)
            .await
            .map_err(map_execution_error)?;
        let request = self
            .runtime_client()?
            .get_input_request(tenant_id, instance_id, request_id)
            .await
            .map_err(WorkflowRuntimeError::from)?;
        let action = action_from_request(workflow_id, &request);
        if let Some(condition) = condition
            && !condition_matches_row(condition, &workflow_action_report_row(&action), &block.id)?
        {
            return Err(not_found().into());
        }
        Ok(action)
    }

    pub fn new(
        engine: Option<Arc<ExecutionEngine>>,
        runtime_client: Option<Arc<RuntimeClient>>,
    ) -> Self {
        Self {
            engine,
            runtime_client,
        }
    }

    fn engine(&self) -> Result<&Arc<ExecutionEngine>, ReportServiceError> {
        self.engine.as_ref().ok_or_else(|| {
            ReportServiceError::Validation(
                "Workflow runtime report sources require the execution engine".to_string(),
            )
        })
    }

    fn runtime_client(&self) -> Result<&Arc<RuntimeClient>, ReportServiceError> {
        self.runtime_client.as_ref().ok_or_else(|| {
            ReportServiceError::Validation(
                "Workflow runtime report sources require a configured runtime client".to_string(),
            )
        })
    }

    /// Used by `ReportService::submit_report_action` + `render_actions_block`
    /// to load the action set for the block (applies the block-level condition
    /// post-fetch since there's no pushdown).
    pub async fn actions_for_block_context(
        &self,
        tenant_id: &str,
        block: &ReportBlockDefinition,
        condition: Option<&Condition>,
    ) -> Result<Vec<WorkflowRuntimeAction>, ReportServiceError> {
        let actions = self.actions_for_source(tenant_id, block).await?;
        let Some(condition) = condition else {
            return Ok(actions);
        };
        actions
            .into_iter()
            .filter_map(|action| {
                let row = workflow_action_report_row(&action);
                match condition_matches_row(condition, &row, &block.id) {
                    Ok(true) => Some(Ok(action)),
                    Ok(false) => None,
                    Err(error) => Some(Err(error)),
                }
            })
            .collect()
    }

    async fn actions_for_source(
        &self,
        tenant_id: &str,
        block: &ReportBlockDefinition,
    ) -> Result<Vec<WorkflowRuntimeAction>, ReportServiceError> {
        let workflow_id = workflow_runtime_workflow_id(block)?;
        let engine = self.engine()?;
        let runtime_client = self.runtime_client()?;

        if let Some(instance_id) = block
            .source
            .instance_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            engine
                .authorize_execution(workflow_id, instance_id, tenant_id)
                .await
                .map_err(map_execution_error)?;
            return list_instance_actions(runtime_client, tenant_id, workflow_id, instance_id)
                .await
                .map_err(Into::into);
        }

        let mut actions = Vec::new();
        let mut page = 0_i32;
        loop {
            let result = list_workflow_actions(
                engine,
                runtime_client,
                tenant_id,
                workflow_id,
                Some(page),
                Some(100),
            )
            .await?;
            actions.extend(result.actions);
            if !result.page.has_next_page {
                break;
            }
            page = page.checked_add(1).ok_or_else(|| {
                ReportServiceError::Validation("Action page offset out of range".into())
            })?;
        }
        Ok(actions)
    }
}

#[async_trait]
impl ReportSourceProvider for WorkflowRuntimeProvider {
    fn kind(&self) -> ReportSourceKind {
        ReportSourceKind::WorkflowRuntime
    }

    async fn fetch_rows(
        &self,
        params: FetchParams<'_>,
    ) -> Result<FetchRowsOutput, ReportServiceError> {
        let entity = workflow_runtime_entity(params.block)?;
        match entity {
            ReportWorkflowRuntimeEntity::Instances => {
                let engine = self.engine()?;
                let runtime_client = self.runtime_client()?;
                let workflow_id = workflow_runtime_workflow_id(params.block)?;
                let mut filters = ExecutionFilters {
                    workflow_id: Some(workflow_id.to_owned()),
                    sort_by: "created_at".into(),
                    ..Default::default()
                };
                let residual = instance_query_filters(params.condition, &mut filters)?;
                let sql_sort = match params.sort {
                    [] => true,
                    [order] if matches!(order.field.as_str(), "createdAt" | "completedAt") => {
                        filters.sort_by = if order.field == "createdAt" {
                            "created_at"
                        } else {
                            "completed_at"
                        }
                        .into();
                        filters.sort_order = order.direction.to_ascii_uppercase();
                        true
                    }
                    _ => false,
                };
                // Supported predicates and ordering are applied in SQL, before
                // the page and count. Residual predicates require the entire
                // matching set; never silently truncate to the first 100 runs.
                let paginated = residual.is_none() && sql_sort;
                let offset = params.offset.max(0) as usize;
                let limit = params.limit.max(0) as usize;
                let mut page = if paginated { offset / 100 } else { 0 };
                let mut skip = if paginated { offset % 100 } else { 0 };
                let mut rows = Vec::new();
                let total;
                loop {
                    // The execution API multiplies its i32 page by page size.
                    if page > (i32::MAX / 100) as usize {
                        return Err(ReportServiceError::Validation(
                            "Execution report page offset is out of range".into(),
                        ));
                    }
                    let result = engine
                        .list_all_executions(
                            params.tenant_id,
                            Some(page as i32),
                            Some(100),
                            filters.clone(),
                        )
                        .await?;
                    let count = result.total_elements;
                    let last = result.last || result.content.is_empty();
                    for instance in result.content.into_iter().skip(skip) {
                        if paginated && rows.len() >= limit {
                            break;
                        }
                        let actions = if should_check_instance_actions(&instance) {
                            list_instance_actions(
                                runtime_client,
                                params.tenant_id,
                                workflow_id,
                                &instance.id,
                            )
                            .await?
                        } else {
                            Vec::new()
                        };
                        rows.push(workflow_instance_report_row(&instance, &actions));
                    }
                    if last || (paginated && rows.len() >= limit) {
                        total = count;
                        break;
                    }
                    skip = 0;
                    page += 1;
                }
                let rows = post_filter_rows(rows, residual.as_ref(), &params.block.id)?;
                Ok(FetchRowsOutput {
                    rows,
                    total_count: paginated.then_some(total),
                })
            }
            ReportWorkflowRuntimeEntity::Actions => {
                let actions = self
                    .actions_for_source(params.tenant_id, params.block)
                    .await?;
                let rows = actions
                    .into_iter()
                    .map(|action| workflow_action_report_row(&action))
                    .collect();
                let rows = post_filter_rows(rows, params.condition, &params.block.id)?;
                Ok(FetchRowsOutput {
                    rows,
                    total_count: None,
                })
            }
            _ => Err(ReportServiceError::Validation(format!(
                "Block '{}' workflow_runtime source does not support system entity {:?}",
                params.block.id, entity
            ))),
        }
    }

    async fn fetch_aggregate(
        &self,
        _params: FetchParams<'_>,
        _request: AggregateRequest,
    ) -> Result<FetchAggregateOutput, ReportServiceError> {
        Err(ReportServiceError::Validation(
            "workflow_runtime source does not support aggregate mode".to_string(),
        ))
    }

    fn validate_block(
        &self,
        block: &ReportBlockDefinition,
        filter_ids: &HashSet<String>,
        view_ids: &HashSet<String>,
        filter_defs: &HashMap<String, &ReportFilterDefinition>,
    ) -> Result<(), ReportServiceError> {
        validate_workflow_runtime_block(block, filter_ids, view_ids, filter_defs)
    }

    fn field_is_known(&self, block: &ReportBlockDefinition, field: &str) -> bool {
        let Ok(entity) = workflow_runtime_entity(block) else {
            return false;
        };
        let fields = workflow_runtime_fields(entity);
        workflow_runtime_row_field_known(&fields, field)
    }

    fn field_set(&self, block: &ReportBlockDefinition) -> Option<HashSet<&'static str>> {
        workflow_runtime_entity(block)
            .ok()
            .map(workflow_runtime_fields)
    }

    fn table_columns(
        &self,
        block: &ReportBlockDefinition,
    ) -> Result<Vec<Value>, ReportServiceError> {
        let entity = workflow_runtime_entity(block)?;
        Ok(workflow_runtime_table_columns(block.table.as_ref(), entity))
    }
}

/// Consume predicates supported by the execution query. OR/NOT and predicates
/// on virtual action fields stay intact and are evaluated over all fetched pages.
fn instance_query_filters(
    condition: Option<&Condition>,
    filters: &mut ExecutionFilters,
) -> Result<Option<Condition>, ReportServiceError> {
    let Some(condition) = condition else {
        return Ok(None);
    };
    let args = condition.arguments.as_deref().unwrap_or_default();
    let op = condition.op.to_ascii_uppercase();
    if matches!(op.as_str(), "OR" | "NOT") {
        // Validate exact references even when this logical expression cannot
        // be pushed down. Each branch gets independent scratch filters.
        for value in args {
            let child: Condition = serde_json::from_value(value.clone())
                .map_err(|e| ReportServiceError::Validation(e.to_string()))?;
            instance_query_filters(Some(&child), &mut ExecutionFilters::default())?;
        }
        return Ok(Some(condition.clone()));
    }
    if op == "AND" {
        let mut remaining = Vec::new();
        for value in args {
            let child: Condition = serde_json::from_value(value.clone())
                .map_err(|e| ReportServiceError::Validation(e.to_string()))?;
            if let Some(child) = instance_query_filters(Some(&child), filters)? {
                remaining.push(serde_json::to_value(child).expect("condition serializes"));
            }
        }
        return Ok((!remaining.is_empty()).then_some(Condition {
            op,
            arguments: Some(remaining),
        }));
    }
    if args.len() != 2 {
        return Ok(Some(condition.clone()));
    }
    let field = args[0].as_str().unwrap_or_default();
    match (field, op.as_str()) {
        ("runLabel", "EQ") => {
            if let Some(label) = args[1].as_str() {
                let label = runtara_dsl::run_label::normalize_run_label(Some(label))
                    .map_err(ReportServiceError::Validation)?;
                if filters.run_label.is_none() {
                    filters.run_label = label;
                    return Ok(None);
                }
            }
        }
        ("status", "EQ" | "IN") if filters.statuses.is_none() => {
            let values = if op == "EQ" {
                vec![args[1].clone()]
            } else {
                args[1].as_array().cloned().unwrap_or_default()
            };
            let statuses: Option<Vec<String>> = values
                .iter()
                .map(|v| v.as_str().map(str::to_owned))
                .collect();
            if let Some(statuses) = statuses.filter(|s| {
                !s.is_empty()
                    && s.iter().all(|s| {
                        matches!(
                            s.as_str(),
                            "queued"
                                | "compiling"
                                | "running"
                                | "suspended"
                                | "completed"
                                | "failed"
                                | "timeout"
                                | "cancelled"
                        )
                    })
            }) {
                filters.statuses = Some(statuses);
                return Ok(None);
            }
        }
        ("createdAt" | "completedAt", "GTE" | "LTE") => {
            let slot = match (field, op.as_str()) {
                ("createdAt", "GTE") => &mut filters.created_from,
                ("createdAt", _) => &mut filters.created_to,
                (_, "GTE") => &mut filters.completed_from,
                _ => &mut filters.completed_to,
            };
            let date = args[1]
                .as_str()
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok());
            if let Some(date) = date {
                let date = date.with_timezone(&chrono::Utc);
                *slot = Some(match *slot {
                    Some(existing) if op == "GTE" => existing.max(date),
                    Some(existing) => existing.min(date),
                    None => date,
                });
                return Ok(None);
            }
        }
        _ => {}
    }
    Ok(Some(condition.clone()))
}

// ============================================================================
// Row builders
// ============================================================================

fn workflow_instance_report_row(
    instance: &WorkflowInstanceDto,
    actions: &[WorkflowRuntimeAction],
) -> Map<String, Value> {
    let mut row = Map::new();
    row.insert("id".to_string(), Value::String(instance.id.clone()));
    row.insert("instanceId".to_string(), Value::String(instance.id.clone()));
    row.insert(
        "workflowId".to_string(),
        Value::String(instance.workflow_id.clone()),
    );
    if let Some(workflow_name) = &instance.workflow_name {
        row.insert(
            "workflowName".to_string(),
            Value::String(workflow_name.clone()),
        );
    }
    row.insert("runLabel".to_string(), json!(instance.run_label));
    row.insert("status".to_string(), json!(instance.status));
    row.insert("completedAt".to_string(), json!(instance.completed_at));
    row.insert(
        "createdAt".to_string(),
        Value::String(instance.created.clone()),
    );
    row.insert(
        "updatedAt".to_string(),
        Value::String(instance.updated.clone()),
    );
    row.insert("usedVersion".to_string(), json!(instance.used_version));
    row.insert(
        "durationSeconds".to_string(),
        instance
            .execution_duration_seconds
            .map(Value::from)
            .unwrap_or(Value::Null),
    );
    row.insert("hasActions".to_string(), Value::Bool(!actions.is_empty()));
    row.insert("actionCount".to_string(), json!(actions.len()));
    row
}

fn workflow_action_report_row(action: &WorkflowRuntimeAction) -> Map<String, Value> {
    let mut row = Map::new();
    row.insert("requestId".into(), Value::String(action.request_id.clone()));
    row.insert("id".to_string(), Value::String(action.id.clone()));
    row.insert(
        "actionId".to_string(),
        Value::String(action.action_id.clone()),
    );
    row.insert(
        "actionKind".to_string(),
        Value::String(action.action_kind.clone()),
    );
    row.insert(
        "targetKind".to_string(),
        Value::String(action.target_kind.clone()),
    );
    row.insert(
        "targetId".to_string(),
        Value::String(action.target_id.clone()),
    );
    row.insert(
        "workflowId".to_string(),
        Value::String(action.workflow_id.clone()),
    );
    row.insert(
        "instanceId".to_string(),
        Value::String(action.instance_id.clone()),
    );
    row.insert(
        "signalId".to_string(),
        Value::String(action.signal_id.clone()),
    );
    row.insert(
        "actionKey".to_string(),
        action
            .action_key
            .as_ref()
            .map(|value| Value::String(value.clone()))
            .unwrap_or(Value::Null),
    );
    row.insert("label".to_string(), Value::String(action.label.clone()));
    row.insert("message".to_string(), Value::String(action.message.clone()));
    row.insert(
        "inputSchema".to_string(),
        action.input_schema.clone().unwrap_or(Value::Null),
    );
    row.insert(
        "schemaFormat".to_string(),
        Value::String(action.schema_format.clone()),
    );
    row.insert("status".to_string(), Value::String(action.status.clone()));
    row.insert(
        "requestedAt".to_string(),
        action
            .requested_at
            .map(|value| Value::String(value.to_rfc3339()))
            .unwrap_or(Value::Null),
    );
    row.insert("correlation".to_string(), action.correlation.clone());
    row.insert("context".to_string(), action.context.clone());
    row.insert("runtime".to_string(), action.runtime.clone());
    row
}

pub(crate) fn workflow_action_row(action: &WorkflowRuntimeAction) -> Map<String, Value> {
    workflow_action_report_row(action)
}

// ============================================================================
// Entity / field metadata
// ============================================================================

pub(crate) fn workflow_runtime_entity(
    block: &ReportBlockDefinition,
) -> Result<ReportWorkflowRuntimeEntity, ReportServiceError> {
    let entity = block.source.entity.ok_or_else(|| {
        ReportServiceError::Validation(format!(
            "Block '{}' workflow_runtime source must specify entity",
            block.id
        ))
    })?;
    match entity {
        ReportWorkflowRuntimeEntity::Instances | ReportWorkflowRuntimeEntity::Actions => Ok(entity),
        _ => Err(ReportServiceError::Validation(format!(
            "Block '{}' workflow_runtime source does not support system entity {:?}",
            block.id, entity
        ))),
    }
}

pub(crate) fn workflow_runtime_workflow_id(
    block: &ReportBlockDefinition,
) -> Result<&str, ReportServiceError> {
    block
        .source
        .workflow_id
        .as_deref()
        .map(str::trim)
        .filter(|workflow_id| !workflow_id.is_empty())
        .ok_or_else(|| {
            ReportServiceError::Validation(format!(
                "Block '{}' workflow_runtime source must specify workflowId",
                block.id
            ))
        })
}

pub(crate) fn workflow_runtime_fields(
    entity: ReportWorkflowRuntimeEntity,
) -> HashSet<&'static str> {
    match entity {
        ReportWorkflowRuntimeEntity::Instances => [
            "id",
            "instanceId",
            "workflowId",
            "workflowName",
            "runLabel",
            "completedAt",
            "status",
            "createdAt",
            "updatedAt",
            "usedVersion",
            "durationSeconds",
            "hasActions",
            "actionCount",
        ]
        .into_iter()
        .collect(),
        ReportWorkflowRuntimeEntity::Actions => [
            "id",
            "actionId",
            "requestId",
            "actionKind",
            "targetKind",
            "targetId",
            "workflowId",
            "instanceId",
            "signalId",
            "actionKey",
            "label",
            "message",
            "inputSchema",
            "schemaFormat",
            "status",
            "requestedAt",
            "correlation",
            "context",
            "runtime",
        ]
        .into_iter()
        .collect(),
        ReportWorkflowRuntimeEntity::RuntimeExecutionMetricBuckets
        | ReportWorkflowRuntimeEntity::RuntimeSystemSnapshot
        | ReportWorkflowRuntimeEntity::ConnectionRateLimitStatus
        | ReportWorkflowRuntimeEntity::ConnectionRateLimitEvents
        | ReportWorkflowRuntimeEntity::ConnectionRateLimitTimeline => HashSet::new(),
    }
}

pub(crate) fn workflow_runtime_row_field_known(
    fields: &HashSet<&'static str>,
    field: &str,
) -> bool {
    fields.contains(field)
        || field
            .split_once('.')
            .is_some_and(|(base, _)| fields.contains(base))
}

pub(crate) fn workflow_runtime_table_columns(
    table: Option<&ReportTableConfig>,
    entity: ReportWorkflowRuntimeEntity,
) -> Vec<Value> {
    if let Some(table) = table
        && !table.columns.is_empty()
    {
        return table
            .columns
            .iter()
            .map(|column| {
                json!({
                    "key": column.field,
                    "label": column.label.clone().unwrap_or_else(|| humanize_label(&column.field)),
                    "format": column.format,
                })
            })
            .collect();
    }

    let columns: &[(&str, &str, Option<&str>)] = match entity {
        ReportWorkflowRuntimeEntity::Instances => &[
            ("instanceId", "Instance ID", None),
            ("status", "Status", None),
            ("hasActions", "Has Actions", Some("boolean")),
            ("actionCount", "Actions", Some("number")),
            ("createdAt", "Created", Some("datetime")),
            ("updatedAt", "Updated", Some("datetime")),
            ("usedVersion", "Version", Some("number")),
            ("durationSeconds", "Duration", Some("number")),
        ],
        ReportWorkflowRuntimeEntity::Actions => &[
            ("actionId", "Action ID", None),
            ("label", "Action", None),
            ("status", "Status", None),
            ("instanceId", "Instance ID", None),
            ("requestedAt", "Requested", Some("datetime")),
        ],
        _ => &[],
    };

    columns
        .iter()
        .map(|(key, label, format)| {
            json!({
                "key": key,
                "label": label,
                "format": format,
            })
        })
        .collect()
}

// ============================================================================
// Validation
// ============================================================================

fn validate_workflow_runtime_block(
    block: &ReportBlockDefinition,
    filter_ids: &HashSet<String>,
    view_ids: &HashSet<String>,
    filter_defs: &HashMap<String, &ReportFilterDefinition>,
) -> Result<(), ReportServiceError> {
    let entity = workflow_runtime_entity(block)?;
    workflow_runtime_workflow_id(block)?;

    if block.source.mode != ReportSourceMode::Filter {
        return Err(ReportServiceError::Validation(format!(
            "Block '{}' workflow_runtime source only supports filter mode",
            block.id
        )));
    }
    if !block.source.schema.trim().is_empty() {
        return Err(ReportServiceError::Validation(format!(
            "Block '{}' workflow_runtime source must not set schema",
            block.id
        )));
    }
    if block.source.connection_id.is_some() {
        return Err(ReportServiceError::Validation(format!(
            "Block '{}' workflow_runtime source must not set connectionId",
            block.id
        )));
    }
    if !block.source.join.is_empty()
        || !block.source.group_by.is_empty()
        || !block.source.aggregates.is_empty()
    {
        return Err(ReportServiceError::Validation(format!(
            "Block '{}' workflow_runtime source does not support joins or aggregates",
            block.id
        )));
    }

    match block.block_type {
        ReportBlockType::Table => {}
        ReportBlockType::Markdown => {}
        ReportBlockType::Actions if entity == ReportWorkflowRuntimeEntity::Actions => {}
        ReportBlockType::Actions => {
            return Err(ReportServiceError::Validation(format!(
                "Block '{}' actions block requires workflow_runtime entity 'actions'",
                block.id
            )));
        }
        _ => {
            return Err(ReportServiceError::Validation(format!(
                "Block '{}' workflow_runtime source only supports table, markdown, and actions blocks",
                block.id
            )));
        }
    }

    let fields = workflow_runtime_fields(entity);
    validate_report_condition_filter_refs(
        block.source.condition.as_ref(),
        filter_defs,
        &format!("block '{}'", block.id),
    )?;
    validate_report_condition_field_refs(
        block.source.condition.as_ref(),
        &|field| workflow_runtime_row_field_known(&fields, field),
        &format!("block '{}'", block.id),
    )?;
    validate_report_source_filter_mappings(
        &block.source.filter_mappings,
        filter_ids,
        &|field| workflow_runtime_row_field_known(&fields, field),
        "source.filterMappings",
        &format!("block '{}'", block.id),
    )?;
    if let Some(table) = &block.table {
        validate_report_table_display_templates(table, &format!("block '{}'", block.id))?;
        for column in &table.columns {
            if column.is_workflow_button() {
                let action = column.workflow_action.as_ref().ok_or_else(|| {
                    ReportServiceError::Validation(format!(
                        "Block '{}' workflow button column '{}' must define workflowAction",
                        block.id, column.field
                    ))
                })?;
                validate_report_workflow_action_config(
                    action,
                    &format!("block '{}' table column '{}'", block.id, column.field),
                )?;
                validate_report_workflow_action_context_field(
                    action,
                    column.field.as_str(),
                    |field| fields.contains(field),
                    &format!(
                        "Block '{}' workflow button column '{}'",
                        block.id, column.field
                    ),
                )?;
                validate_report_workflow_action_row_conditions(
                    action,
                    |field| workflow_runtime_row_field_known(&fields, field),
                    &format!(
                        "Block '{}' workflow button column '{}'",
                        block.id, column.field
                    ),
                )?;
                continue;
            }
            if column.is_interaction_buttons() {
                validate_report_interaction_buttons(
                    &column.interaction_buttons,
                    filter_ids,
                    view_ids,
                    &|field| workflow_runtime_row_field_known(&fields, field),
                    &format!(
                        "Block '{}' interaction button column '{}'",
                        block.id, column.field
                    ),
                )?;
                continue;
            }
            if column.is_chart() || column.is_value_lookup() {
                return Err(ReportServiceError::Validation(format!(
                    "Block '{}' workflow_runtime table columns cannot use nested sources",
                    block.id
                )));
            }
            if !fields.contains(column.field.as_str()) {
                return Err(ReportServiceError::Validation(format!(
                    "Block '{}' references unknown workflow_runtime field '{}'",
                    block.id, column.field
                )));
            }
        }
        for sort in &table.default_sort {
            if !fields.contains(sort.field.as_str()) {
                return Err(ReportServiceError::Validation(format!(
                    "Block '{}' references unknown workflow_runtime sort field '{}'",
                    block.id, sort.field
                )));
            }
        }
        for action in &table.actions {
            validate_report_table_action_config(
                action,
                &format!("block '{}' table action '{}'", block.id, action.id),
            )?;
        }
    }
    for sort in &block.source.order_by {
        if !fields.contains(sort.field.as_str()) {
            return Err(ReportServiceError::Validation(format!(
                "Block '{}' references unknown workflow_runtime orderBy field '{}'",
                block.id, sort.field
            )));
        }
    }
    validate_block_interactions(block, filter_ids, view_ids)
}

// ============================================================================
// Helpers
// ============================================================================

fn should_check_instance_actions(instance: &WorkflowInstanceDto) -> bool {
    !instance.status.is_terminal() && instance.has_pending_input
}

fn post_filter_rows(
    rows: Vec<Map<String, Value>>,
    condition: Option<&Condition>,
    block_id: &str,
) -> Result<Vec<Map<String, Value>>, ReportServiceError> {
    let Some(condition) = condition else {
        return Ok(rows);
    };
    rows.into_iter()
        .filter_map(
            |row| match condition_matches_row(condition, &row, block_id) {
                Ok(true) => Some(Ok(row)),
                Ok(false) => None,
                Err(error) => Some(Err(error)),
            },
        )
        .collect()
}

#[cfg(test)]
mod run_label_query_tests {
    use super::*;

    #[test]
    fn label_status_and_date_conditions_compose_before_pagination() {
        let condition: Condition = serde_json::from_value(json!({"op":"AND","arguments":[
            {"op":"EQ","arguments":["runLabel"," Order_123 "]},
            {"op":"IN","arguments":["status",["suspended","running"]]},
            {"op":"GTE","arguments":["createdAt","2026-01-01T00:00:00Z"]},
            {"op":"LTE","arguments":["createdAt","2026-01-31T23:59:59Z"]},
            {"op":"GTE","arguments":["completedAt","2026-01-02T00:00:00Z"]}
        ]}))
        .unwrap();
        let mut filters = ExecutionFilters {
            workflow_id: Some("orders".into()),
            ..Default::default()
        };
        assert!(
            instance_query_filters(Some(&condition), &mut filters)
                .unwrap()
                .is_none()
        );
        assert_eq!(filters.run_label.as_deref(), Some(" Order_123 "));
        assert_eq!(filters.workflow_id.as_deref(), Some("orders"));
        assert_eq!(filters.statuses.unwrap(), ["suspended", "running"]);
        assert!(filters.created_from.unwrap() < filters.created_to.unwrap());
        assert!(filters.completed_from.is_some());
    }

    #[test]
    fn disjunction_and_virtual_predicates_keep_the_full_result_set() {
        let condition: Condition = serde_json::from_value(json!({"op":"OR","arguments":[
            {"op":"EQ","arguments":["runLabel","order_123"]},
            {"op":"EQ","arguments":["hasActions",true]}
        ]}))
        .unwrap();
        let mut filters = ExecutionFilters::default();
        let residual = instance_query_filters(Some(&condition), &mut filters)
            .unwrap()
            .unwrap();
        assert_eq!(residual.op, "OR");
        assert!(filters.run_label.is_none());
    }

    #[test]
    fn exact_report_lookup_rejects_invalid_and_oversized_identifiers() {
        for value in ["".into(), "bad\nlabel".into(), "x".repeat(251)] {
            let condition: Condition =
                serde_json::from_value(json!({"op":"EQ","arguments":["runLabel",value]})).unwrap();
            assert!(
                instance_query_filters(Some(&condition), &mut ExecutionFilters::default()).is_err()
            );
            for op in ["AND", "OR", "NOT"] {
                let nested: Condition =
                    serde_json::from_value(json!({"op":op,"arguments":[condition]})).unwrap();
                assert!(
                    instance_query_filters(Some(&nested), &mut ExecutionFilters::default())
                        .is_err()
                );
            }
        }
    }

    #[test]
    fn repeated_date_bounds_use_the_strictest_instant() {
        let condition: Condition = serde_json::from_value(json!({"op":"AND","arguments":[
            {"op":"GTE","arguments":["createdAt","2026-01-01T00:00:00Z"]},
            {"op":"GTE","arguments":["createdAt","2026-01-02T01:00:00+01:00"]},
            {"op":"LTE","arguments":["completedAt","2026-01-05T00:00:00Z"]},
            {"op":"LTE","arguments":["completedAt","2026-01-04T00:00:00Z"]}
        ]}))
        .unwrap();
        let mut filters = ExecutionFilters::default();
        assert!(
            instance_query_filters(Some(&condition), &mut filters)
                .unwrap()
                .is_none()
        );
        assert_eq!(
            filters.created_from.unwrap().to_rfc3339(),
            "2026-01-02T00:00:00+00:00"
        );
        assert_eq!(
            filters.completed_to.unwrap().to_rfc3339(),
            "2026-01-04T00:00:00+00:00"
        );
    }
}
