use chrono::{DateTime, Datelike, Duration, NaiveTime, Utc};
use regex::Regex;
use serde_json::{Value, json};
use sqlx::PgPool;
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use thiserror::Error;
use uuid::Uuid;

use crate::api::dto::object_model::{
    AggregateFn, AggregateOrderBy, AggregateRequest, AggregateSpec, ColumnType as ObjectColumnType,
    Condition, FilterRequest, Schema as ObjectSchema, SortDirection,
};
use crate::api::dto::reports::*;
use crate::api::dto::workflows::WorkflowInstanceDto;
use crate::api::repositories::object_model::ObjectStoreManager;
use crate::api::repositories::reports::ReportRepository;
use crate::api::repositories::workflows::WorkflowRepository;
use crate::api::services::object_model::{
    InstanceService, SchemaService, ServiceError as ObjectModelServiceError,
};
use crate::api::services::workflow_runtime::{
    WorkflowRuntimeAction, WorkflowRuntimeError, list_instance_actions, list_workflow_actions,
    submit_workflow_action,
};
use crate::auth::{AuthContext, AuthMethod};
use crate::runtime_client::{GetTenantMetricsOptions, MetricsGranularity, RuntimeClient};
use crate::workers::execution_engine::{ExecutionEngine, ExecutionError};

mod query_plan;

use self::query_plan::{
    JoinResolution, build_alias_index, condition_matches_row, empty_join_result,
    enrich_aggregate_result, field_alias_prefix, primary_pushdown_condition, sort_rows,
    split_qualified_condition, strip_alias_from_condition, validate_join_request,
    value_to_lookup_key,
};

const MAX_TABLE_PAGE_SIZE: i64 = 500;
const MAX_AGGREGATE_ROWS: i64 = 1000;
const MAX_JOIN_POST_FILTER_ROWS: i64 = 50_000;
/// Cap on rows fetched per join from a dimension schema. Broadcast-hash
/// join breaks down (round trip + memory) past this; report a validation
/// error so the caller adds a more selective `<alias>.<field>` condition.
const MAX_BROADCAST_JOIN_DIM_ROWS: i64 = 50_000;

#[derive(Debug, Error)]
pub enum ReportServiceError {
    #[error("Report not found")]
    NotFound,
    #[error("{0}")]
    Validation(String),
    #[error("{0}")]
    ValidationIssue(ReportValidationIssue),
    #[error("{0}")]
    Conflict(String),
    #[error("{0}")]
    Database(String),
}

pub struct ReportService {
    repo: ReportRepository,
    workflow_repo: WorkflowRepository,
    schema_service: SchemaService,
    instance_service: InstanceService,
    connections: Arc<runtara_connections::ConnectionsFacade>,
    engine: Option<Arc<ExecutionEngine>>,
    runtime_client: Option<Arc<RuntimeClient>>,
}

#[derive(Clone, Copy)]
struct ReportConditionRuntimeContext<'a> {
    definition: &'a ReportDefinition,
    block: &'a ReportBlockDefinition,
    resolved_filters: &'a HashMap<String, Value>,
    block_request: Option<&'a ReportBlockDataRequest>,
}

struct ObjectModelOptionQuery<'a> {
    context: String,
    schema: String,
    connection_id: Option<&'a str>,
    value_field: String,
    label_field: String,
    conditions: Vec<Condition>,
    search_fields: Vec<String>,
    search_query: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ReportMarkdownSourcePlaceholder {
    field_path: String,
}

fn report_validation_issue(
    path: impl Into<String>,
    code: impl Into<String>,
    message: impl Into<String>,
    hint: Option<String>,
) -> ReportValidationIssue {
    ReportValidationIssue {
        path: path.into(),
        code: code.into(),
        message: message.into(),
        hint,
    }
}

fn report_validation_error(
    path: impl Into<String>,
    code: impl Into<String>,
    message: impl Into<String>,
    hint: Option<String>,
) -> ReportServiceError {
    ReportServiceError::ValidationIssue(report_validation_issue(path, code, message, hint))
}

fn report_validation_issue_from_error(error: ReportServiceError) -> ReportValidationIssue {
    match error {
        ReportServiceError::ValidationIssue(issue) => issue,
        ReportServiceError::Validation(message) => report_validation_issue(
            "$",
            "VALIDATION_ERROR",
            message,
            Some(
                "Use validate_report with mode='all' for syntax plus semantic checks.".to_string(),
            ),
        ),
        other => report_validation_issue("$", "VALIDATION_ERROR", other.to_string(), None),
    }
}

impl ReportService {
    pub fn report_definition_json_schema() -> Value {
        let mut schema = serde_json::to_value(schemars::schema_for!(ReportDefinition))
            .unwrap_or_else(|_| json!({}));
        seal_json_schema_objects(&mut schema);
        schema
    }

    pub fn validate_report_definition_json_syntax(
        definition: &Value,
    ) -> Result<(), ReportServiceError> {
        let errors = Self::validate_report_definition_json_syntax_issues(definition)?;
        if !errors.is_empty() {
            let first = errors
                .into_iter()
                .next()
                .expect("syntax validation errors cannot be empty here");
            return Err(ReportServiceError::ValidationIssue(first));
        }
        Ok(())
    }

    pub fn validate_report_definition_json_syntax_issues(
        definition: &Value,
    ) -> Result<Vec<ReportValidationIssue>, ReportServiceError> {
        let schema = Self::report_definition_json_schema();
        let validator = jsonschema::validator_for(&schema).map_err(|err| {
            report_validation_error(
                "$",
                "REPORT_SCHEMA_INVALID",
                format!("Report definition JSON Schema is invalid: {}", err),
                None,
            )
        })?;
        let errors = validator
            .iter_errors(definition)
            .take(10)
            .map(|err| {
                let instance_path = err.instance_path.to_string();
                let path = if instance_path.is_empty() {
                    "$".to_string()
                } else {
                    format!("${instance_path}")
                };
                report_validation_issue(
                    path,
                    "REPORT_JSON_SCHEMA_VALIDATION_ERROR",
                    err.to_string(),
                    Some(
                        "Fetch get_report_authoring_schema for examples or GET /api/runtime/reports/schema for the machine schema."
                            .to_string(),
                    ),
                )
            })
            .collect::<Vec<_>>();
        Ok(errors)
    }

    pub fn new(
        pool: PgPool,
        manager: Arc<ObjectStoreManager>,
        connections: Arc<runtara_connections::ConnectionsFacade>,
    ) -> Self {
        Self {
            repo: ReportRepository::new(pool.clone()),
            workflow_repo: WorkflowRepository::new(pool),
            schema_service: SchemaService::new(manager.clone(), connections.clone()),
            instance_service: InstanceService::new(manager, connections.clone()),
            connections,
            engine: None,
            runtime_client: None,
        }
    }

    pub fn with_runtime(
        mut self,
        engine: Arc<ExecutionEngine>,
        runtime_client: Option<Arc<RuntimeClient>>,
    ) -> Self {
        self.engine = Some(engine);
        self.runtime_client = runtime_client;
        self
    }

    pub async fn list_reports(
        &self,
        tenant_id: &str,
    ) -> Result<Vec<ReportSummary>, ReportServiceError> {
        let reports = self.repo.list(tenant_id).await.map_err(map_sqlx_error)?;

        Ok(reports.iter().map(ReportSummary::from).collect())
    }

    pub async fn get_report(
        &self,
        tenant_id: &str,
        id_or_slug: &str,
    ) -> Result<ReportDto, ReportServiceError> {
        self.repo
            .get(tenant_id, id_or_slug)
            .await
            .map_err(map_sqlx_error)?
            .ok_or(ReportServiceError::NotFound)
    }

    pub async fn create_report(
        &self,
        tenant_id: &str,
        request: CreateReportRequest,
    ) -> Result<ReportDto, ReportServiceError> {
        self.validate_definition(tenant_id, &request.definition)
            .await?;

        let slug = request
            .slug
            .unwrap_or_else(|| slugify(&request.name))
            .trim()
            .to_string();
        validate_slug(&slug)?;

        let report = ReportDto {
            id: format!("rep_{}", Uuid::new_v4()),
            slug,
            name: request.name,
            description: request.description,
            tags: request.tags,
            status: request.status,
            definition_version: request.definition.definition_version,
            definition: request.definition,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };

        self.repo
            .create(tenant_id, &report)
            .await
            .map_err(map_sqlx_error)
    }

    pub async fn update_report(
        &self,
        tenant_id: &str,
        id_or_slug: &str,
        request: UpdateReportRequest,
    ) -> Result<ReportDto, ReportServiceError> {
        self.validate_definition(tenant_id, &request.definition)
            .await?;
        validate_slug(&request.slug)?;

        let existing = self.get_report(tenant_id, id_or_slug).await?;
        let report = ReportDto {
            id: existing.id,
            slug: request.slug,
            name: request.name,
            description: request.description,
            tags: request.tags,
            status: request.status,
            definition_version: request.definition.definition_version,
            definition: request.definition,
            created_at: existing.created_at,
            updated_at: Utc::now(),
        };

        self.repo
            .update(tenant_id, id_or_slug, &report)
            .await
            .map_err(map_sqlx_error)?
            .ok_or(ReportServiceError::NotFound)
    }

    pub async fn delete_report(
        &self,
        tenant_id: &str,
        id_or_slug: &str,
    ) -> Result<(), ReportServiceError> {
        let affected = self
            .repo
            .delete(tenant_id, id_or_slug)
            .await
            .map_err(map_sqlx_error)?;

        if affected == 0 {
            Err(ReportServiceError::NotFound)
        } else {
            Ok(())
        }
    }

    pub async fn add_report_block(
        &self,
        tenant_id: &str,
        id_or_slug: &str,
        request: AddReportBlockRequest,
    ) -> Result<ReportBlockMutationResponse, ReportServiceError> {
        let mut report = self.get_report(tenant_id, id_or_slug).await?;
        if report
            .definition
            .blocks
            .iter()
            .any(|block| block.id == request.block.id)
        {
            return Err(ReportServiceError::Conflict(format!(
                "Report block '{}' already exists",
                request.block.id
            )));
        }

        let position = request.position.unwrap_or_default();
        let insert_index = resolve_position_index(&report.definition.blocks, &position)?;
        let block = request.block;
        let block_id = block.id.clone();

        report.definition.blocks.insert(insert_index, block.clone());

        let report = self.save_report_definition(tenant_id, report).await?;
        Ok(ReportBlockMutationResponse {
            success: true,
            report,
            block: Some(block),
            message: format!("Report block '{}' added", block_id),
        })
    }

    pub async fn replace_report_block(
        &self,
        tenant_id: &str,
        id_or_slug: &str,
        block_id: &str,
        request: ReplaceReportBlockRequest,
    ) -> Result<ReportBlockMutationResponse, ReportServiceError> {
        if request.block.id != block_id {
            return Err(ReportServiceError::Validation(
                "Replacement block id must match the path block id".to_string(),
            ));
        }

        let mut report = self.get_report(tenant_id, id_or_slug).await?;
        let index = find_block_index(&report.definition.blocks, block_id)?;
        report.definition.blocks[index] = request.block.clone();

        let report = self.save_report_definition(tenant_id, report).await?;
        Ok(ReportBlockMutationResponse {
            success: true,
            report,
            block: Some(request.block),
            message: format!("Report block '{}' replaced", block_id),
        })
    }

    pub async fn patch_report_block(
        &self,
        tenant_id: &str,
        id_or_slug: &str,
        block_id: &str,
        request: PatchReportBlockRequest,
    ) -> Result<ReportBlockMutationResponse, ReportServiceError> {
        if !request.patch.is_object() {
            return Err(ReportServiceError::Validation(
                "Report block patch must be a JSON object".to_string(),
            ));
        }
        if request.patch.get("id").is_some() {
            return Err(ReportServiceError::Validation(
                "Report block id cannot be changed with patch_report_block".to_string(),
            ));
        }

        let mut report = self.get_report(tenant_id, id_or_slug).await?;
        let index = find_block_index(&report.definition.blocks, block_id)?;
        let mut block_value = serde_json::to_value(&report.definition.blocks[index])
            .map_err(|e| ReportServiceError::Validation(e.to_string()))?;
        apply_json_merge_patch(&mut block_value, &request.patch);
        let patched_block: ReportBlockDefinition = serde_json::from_value(block_value)
            .map_err(|e| ReportServiceError::Validation(format!("Invalid block patch: {}", e)))?;
        if patched_block.id != block_id {
            return Err(ReportServiceError::Validation(
                "Report block id cannot be changed with patch_report_block".to_string(),
            ));
        }

        report.definition.blocks[index] = patched_block.clone();
        let report = self.save_report_definition(tenant_id, report).await?;
        Ok(ReportBlockMutationResponse {
            success: true,
            report,
            block: Some(patched_block),
            message: format!("Report block '{}' updated", block_id),
        })
    }

    pub async fn move_report_block(
        &self,
        tenant_id: &str,
        id_or_slug: &str,
        block_id: &str,
        request: MoveReportBlockRequest,
    ) -> Result<ReportBlockMutationResponse, ReportServiceError> {
        let mut report = self.get_report(tenant_id, id_or_slug).await?;
        let current_index = find_block_index(&report.definition.blocks, block_id)?;
        let block = report.definition.blocks.remove(current_index);
        let new_index = resolve_position_index(&report.definition.blocks, &request.position)?;
        report.definition.blocks.insert(new_index, block.clone());

        let report = self.save_report_definition(tenant_id, report).await?;
        Ok(ReportBlockMutationResponse {
            success: true,
            report,
            block: Some(block),
            message: format!("Report block '{}' moved", block_id),
        })
    }

    pub async fn remove_report_block(
        &self,
        tenant_id: &str,
        id_or_slug: &str,
        block_id: &str,
        _request: RemoveReportBlockRequest,
    ) -> Result<ReportBlockMutationResponse, ReportServiceError> {
        let mut report = self.get_report(tenant_id, id_or_slug).await?;
        let index = find_block_index(&report.definition.blocks, block_id)?;
        report.definition.blocks.remove(index);

        let report = self.save_report_definition(tenant_id, report).await?;
        Ok(ReportBlockMutationResponse {
            success: true,
            report,
            block: None,
            message: format!("Report block '{}' removed", block_id),
        })
    }

    pub async fn validate_report(
        &self,
        tenant_id: &str,
        definition: &ReportDefinition,
    ) -> ValidateReportResponse {
        match self.validate_definition(tenant_id, definition).await {
            Ok(()) => ValidateReportResponse {
                valid: true,
                errors: vec![],
                warnings: vec![],
            },
            Err(error) => ValidateReportResponse {
                valid: false,
                errors: vec![report_validation_issue_from_error(error)],
                warnings: vec![],
            },
        }
    }

    pub async fn render_report(
        &self,
        tenant_id: &str,
        id_or_slug: &str,
        request: ReportRenderRequest,
    ) -> Result<ReportRenderResponse, ReportServiceError> {
        let report = self.get_report(tenant_id, id_or_slug).await?;
        let resolved_filters = resolve_filters(&report.definition, &request.filters);
        let requested_blocks = requested_blocks(&report.definition, request.blocks.as_deref());
        let request_by_id: HashMap<_, _> = request
            .blocks
            .unwrap_or_default()
            .into_iter()
            .map(|block| (block.id.clone(), block))
            .collect();

        let mut blocks = HashMap::new();
        let mut errors = Vec::new();

        for block in requested_blocks {
            let block_request = request_by_id.get(&block.id);
            let result = self
                .render_block(
                    tenant_id,
                    &report.definition,
                    block,
                    &resolved_filters,
                    block_request,
                )
                .await;

            match result {
                Ok(rendered) => {
                    blocks.insert(block.id.clone(), rendered);
                }
                Err(error) => {
                    let block_error = ReportBlockError {
                        code: "BLOCK_RENDER_FAILED".to_string(),
                        message: error.to_string(),
                        block_id: Some(block.id.clone()),
                    };
                    errors.push(block_error.clone());
                    blocks.insert(
                        block.id.clone(),
                        ReportBlockRenderResult {
                            block_type: block.block_type,
                            status: ReportBlockStatus::Error,
                            title: block.title.clone(),
                            data: None,
                            error: Some(block_error),
                        },
                    );
                }
            }
        }

        Ok(ReportRenderResponse {
            success: true,
            report: ReportRenderMetadata {
                id: report.id,
                definition_version: report.definition_version,
            },
            resolved_filters,
            blocks,
            errors,
        })
    }

    pub async fn preview_report(
        &self,
        tenant_id: &str,
        request: ReportPreviewRequest,
    ) -> Result<ReportRenderResponse, ReportServiceError> {
        self.validate_definition(tenant_id, &request.definition)
            .await?;

        let resolved_filters = resolve_filters(&request.definition, &request.filters);
        let requested_blocks = requested_blocks(&request.definition, request.blocks.as_deref());
        let request_by_id: HashMap<_, _> = request
            .blocks
            .unwrap_or_default()
            .into_iter()
            .map(|block| (block.id.clone(), block))
            .collect();

        let mut blocks = HashMap::new();
        let mut errors = Vec::new();

        for block in requested_blocks {
            let block_request = request_by_id.get(&block.id);
            let result = self
                .render_block(
                    tenant_id,
                    &request.definition,
                    block,
                    &resolved_filters,
                    block_request,
                )
                .await;

            match result {
                Ok(rendered) => {
                    blocks.insert(block.id.clone(), rendered);
                }
                Err(error) => {
                    let block_error = ReportBlockError {
                        code: "BLOCK_RENDER_FAILED".to_string(),
                        message: error.to_string(),
                        block_id: Some(block.id.clone()),
                    };
                    errors.push(block_error.clone());
                    blocks.insert(
                        block.id.clone(),
                        ReportBlockRenderResult {
                            block_type: block.block_type,
                            status: ReportBlockStatus::Error,
                            title: block.title.clone(),
                            data: None,
                            error: Some(block_error),
                        },
                    );
                }
            }
        }

        Ok(ReportRenderResponse {
            success: true,
            report: ReportRenderMetadata {
                id: "preview".to_string(),
                definition_version: request.definition.definition_version,
            },
            resolved_filters,
            blocks,
            errors,
        })
    }

    pub async fn render_report_block(
        &self,
        tenant_id: &str,
        id_or_slug: &str,
        block_id: &str,
        request: ReportBlockOnlyDataRequest,
    ) -> Result<ReportBlockRenderResult, ReportServiceError> {
        let report = self.get_report(tenant_id, id_or_slug).await?;
        let block = report
            .definition
            .blocks
            .iter()
            .find(|block| block.id == block_id)
            .ok_or_else(|| {
                ReportServiceError::Validation(format!("Unknown block '{}'", block_id))
            })?;

        let resolved_filters = resolve_filters(&report.definition, &request.filters);
        let block_request = ReportBlockDataRequest {
            id: block_id.to_string(),
            page: request.page,
            sort: request.sort,
            search: request.search,
            block_filters: request.block_filters,
        };

        self.render_block(
            tenant_id,
            &report.definition,
            block,
            &resolved_filters,
            Some(&block_request),
        )
        .await
    }

    pub async fn submit_report_workflow_action(
        &self,
        tenant_id: &str,
        id_or_slug: &str,
        block_id: &str,
        action_id: &str,
        request: SubmitReportWorkflowActionRequest,
        auth_context: &AuthContext,
    ) -> Result<Value, ReportServiceError> {
        let report = self.get_report(tenant_id, id_or_slug).await?;
        let block = report
            .definition
            .blocks
            .iter()
            .find(|block| block.id == block_id)
            .ok_or_else(|| {
                ReportServiceError::Validation(format!("Unknown block '{}'", block_id))
            })?;

        if block.block_type != ReportBlockType::Actions {
            return Err(ReportServiceError::Validation(format!(
                "Block '{}' is not an actions block",
                block.id
            )));
        }
        let entity = workflow_runtime_entity(block)?;
        if entity != ReportWorkflowRuntimeEntity::Actions {
            return Err(ReportServiceError::Validation(format!(
                "Block '{}' action submit requires workflow_runtime entity 'actions'",
                block.id
            )));
        }

        let resolved_filters = resolve_filters(&report.definition, &request.filters);
        let block_request = ReportBlockDataRequest {
            id: block_id.to_string(),
            page: None,
            sort: vec![],
            search: None,
            block_filters: request.block_filters,
        };
        let actions = self
            .workflow_runtime_actions_for_block_context(
                tenant_id,
                &report.definition,
                block,
                &resolved_filters,
                Some(&block_request),
            )
            .await?;
        let action = actions
            .into_iter()
            .find(|action| action.action_id == action_id || action.signal_id == action_id)
            .ok_or_else(|| {
                ReportServiceError::Conflict(format!(
                    "Action '{}' is no longer open for report block '{}'",
                    action_id, block.id
                ))
            })?;

        let payload = merge_report_action_payload(&request.payload, block, auth_context)?;
        let submitted = submit_workflow_action(
            self.require_execution_engine()?,
            self.require_runtime_client()?,
            tenant_id,
            &action.workflow_id,
            &action.instance_id,
            &action.action_id,
            &payload,
        )
        .await
        .map_err(map_workflow_runtime_error_to_report)?;

        Ok(json!({
            "success": true,
            "workflowId": submitted.workflow_id,
            "instanceId": submitted.instance_id,
            "actionId": submitted.action_id,
            "signalId": submitted.signal_id,
            "status": "submitted",
        }))
    }

    pub async fn get_filter_options(
        &self,
        tenant_id: &str,
        id_or_slug: &str,
        filter_id: &str,
        request: ReportFilterOptionsRequest,
    ) -> Result<ReportFilterOptionsResponse, ReportServiceError> {
        let report = self.get_report(tenant_id, id_or_slug).await?;
        let filter = report
            .definition
            .filters
            .iter()
            .find(|filter| filter.id == filter_id)
            .ok_or_else(|| {
                ReportServiceError::Validation(format!("Unknown report filter '{}'", filter_id))
            })?;
        let offset = request.offset.max(0);
        let limit = request.limit.clamp(1, MAX_TABLE_PAGE_SIZE);
        let options = filter.options.as_ref().and_then(Value::as_object);

        let Some(options_config) = options else {
            return Ok(static_filter_options_response(
                filter,
                offset,
                limit,
                &request.query,
            ));
        };

        let source = options_config
            .get("source")
            .and_then(Value::as_str)
            .unwrap_or("static");
        if source != "object_model" {
            return Ok(static_filter_options_response(
                filter,
                offset,
                limit,
                &request.query,
            ));
        }

        let schema = option_string(options_config, "schema")
            .ok_or_else(|| {
                ReportServiceError::Validation(format!(
                    "Filter '{}' object_model options must include schema",
                    filter.id
                ))
            })?
            .to_string();
        let field = option_string(options_config, "field")
            .or_else(|| option_string(options_config, "valueField"))
            .ok_or_else(|| {
                ReportServiceError::Validation(format!(
                    "Filter '{}' object_model options must include field",
                    filter.id
                ))
            })?
            .to_string();
        let label_field = option_string(options_config, "labelField")
            .unwrap_or(&field)
            .to_string();
        let connection_id = option_string(options_config, "connectionId");
        let resolved_filters = resolve_filters(&report.definition, &request.filters);
        let condition_filter_defs = report_filter_definitions_by_id(&report.definition.filters);
        let mut conditions = Vec::new();

        if let Some(condition) = options_config.get("condition") {
            let parsed = serde_json::from_value::<Condition>(condition.clone()).map_err(|err| {
                ReportServiceError::Validation(format!(
                    "Filter '{}' options.condition is invalid: {}",
                    filter.id, err
                ))
            })?;
            if let Some(condition) = resolve_report_condition_values(
                &parsed,
                &condition_filter_defs,
                &resolved_filters,
                &format!("filter '{}'", filter.id),
            )? {
                conditions.push(condition);
            }
        }

        append_option_context_conditions(
            &mut conditions,
            &report.definition,
            filter,
            options_config,
            &resolved_filters,
        );

        let search_query = request.query.as_deref().map(str::trim).unwrap_or("");
        let search_fields = if option_bool(options_config, "search").unwrap_or(false) {
            vec![label_field.clone()]
        } else {
            Vec::new()
        };
        let option_query = ObjectModelOptionQuery {
            context: format!("filter '{}'", filter.id),
            schema,
            connection_id,
            value_field: field,
            label_field,
            conditions,
            search_fields,
            search_query: search_query.to_string(),
        };
        let (options, page) = self
            .query_object_model_options(tenant_id, option_query, offset, limit)
            .await?;

        Ok(ReportFilterOptionsResponse {
            success: true,
            filter: ReportFilterOptionsMetadata {
                id: filter.id.clone(),
            },
            options,
            page,
        })
    }

    pub async fn get_lookup_options(
        &self,
        tenant_id: &str,
        id_or_slug: &str,
        block_id: &str,
        field: &str,
        request: ReportLookupOptionsRequest,
    ) -> Result<ReportLookupOptionsResponse, ReportServiceError> {
        let report = self.get_report(tenant_id, id_or_slug).await?;
        let block = report
            .definition
            .blocks
            .iter()
            .find(|block| block.id == block_id)
            .ok_or_else(|| {
                ReportServiceError::Validation(format!("Unknown block '{}'", block_id))
            })?;
        let lookup = lookup_editor_for_field(block, field).ok_or_else(|| {
            ReportServiceError::Validation(format!(
                "Block '{}' field '{}' is not configured with editor.kind='lookup'",
                block_id, field
            ))
        })?;
        let offset = request.offset.max(0);
        let limit = request.limit.clamp(1, MAX_TABLE_PAGE_SIZE);
        let resolved_filters = resolve_filters(&report.definition, &request.filters);
        let block_request = ReportBlockDataRequest {
            id: block.id.clone(),
            page: None,
            sort: vec![],
            search: None,
            block_filters: request.block_filters,
        };
        let condition_filter_defs = block_condition_filter_definitions(&report.definition, block);
        let condition_filter_values =
            block_condition_filter_values(block, &resolved_filters, Some(&block_request));
        let mut conditions = Vec::new();

        if let Some(condition) = &lookup.condition
            && let Some(condition) = resolve_report_condition_values(
                condition,
                &condition_filter_defs,
                &condition_filter_values,
                &format!("block '{}' field '{}' lookup", block_id, field),
            )?
        {
            conditions.push(condition);
        }
        append_source_mapping_conditions(
            &mut conditions,
            &lookup.filter_mappings,
            &condition_filter_values,
        );

        let mut search_fields = lookup.search_fields.clone();
        if search_fields.is_empty() {
            search_fields.push(lookup.label_field.clone());
        }
        let option_query = ObjectModelOptionQuery {
            context: format!("block '{}' field '{}' lookup", block_id, field),
            schema: lookup.schema.clone(),
            connection_id: lookup.connection_id.as_deref(),
            value_field: lookup.value_field.clone(),
            label_field: lookup.label_field.clone(),
            conditions,
            search_fields,
            search_query: request.query.as_deref().unwrap_or("").trim().to_string(),
        };
        let (options, page) = self
            .query_object_model_options(tenant_id, option_query, offset, limit)
            .await?;

        Ok(ReportLookupOptionsResponse {
            success: true,
            block: ReportLookupBlockMetadata {
                id: block.id.clone(),
            },
            field: field.to_string(),
            options,
            page,
        })
    }

    async fn query_object_model_options(
        &self,
        tenant_id: &str,
        mut query: ObjectModelOptionQuery<'_>,
        offset: i64,
        limit: i64,
    ) -> Result<(Vec<ReportFilterOption>, ReportFilterOptionsPage), ReportServiceError> {
        let search_query = query.search_query.trim();
        if !search_query.is_empty() && !query.search_fields.is_empty() {
            let schema = self
                .schema_service
                .get_schema_by_name(&query.schema, tenant_id, query.connection_id)
                .await
                .map_err(map_object_model_error)?;
            if let Some(search_condition) =
                option_search_condition(&schema, &query.search_fields, search_query)
            {
                query.conditions.push(search_condition);
            }
        }

        let mut group_by = vec![query.value_field.clone()];
        if query.label_field != query.value_field {
            group_by.push(query.label_field.clone());
        }

        let conditions = std::mem::take(&mut query.conditions);
        let aggregate_request = AggregateRequest {
            condition: combine_conditions(conditions),
            group_by,
            aggregates: vec![AggregateSpec {
                alias: "__count".to_string(),
                fn_: AggregateFn::Count,
                column: None,
                distinct: false,
                order_by: vec![],
                expression: None,
                percentile: None,
            }],
            order_by: vec![AggregateOrderBy {
                column: query.label_field.clone(),
                direction: SortDirection::Asc,
            }],
            limit: Some(limit),
            offset: Some(offset),
        };

        let result = self
            .instance_service
            .aggregate_instances_by_schema(
                tenant_id,
                &query.schema,
                aggregate_request,
                query.connection_id,
            )
            .await
            .map_err(|error| {
                let mapped = map_object_model_error(error);
                match mapped {
                    ReportServiceError::Validation(message) => ReportServiceError::Validation(
                        format!("{} options query is invalid: {}", query.context, message),
                    ),
                    other => other,
                }
            })?;
        let value_index = result
            .columns
            .iter()
            .position(|column| column == &query.value_field)
            .unwrap_or(0);
        let label_index = result
            .columns
            .iter()
            .position(|column| column == &query.label_field)
            .unwrap_or(value_index);
        let count_index = result.columns.iter().position(|column| column == "__count");
        let options = result
            .rows
            .into_iter()
            .filter_map(|row| {
                let value = row.get(value_index)?.clone();
                let label_value = row.get(label_index).unwrap_or(&value);
                let count = count_index
                    .and_then(|index| row.get(index))
                    .and_then(Value::as_i64);
                Some(ReportFilterOption {
                    label: filter_option_label(label_value),
                    value,
                    count,
                })
            })
            .collect::<Vec<_>>();
        let page = ReportFilterOptionsPage {
            offset,
            size: limit,
            total_count: result.group_count,
            has_next_page: offset + limit < result.group_count,
        };

        Ok((options, page))
    }

    pub async fn query_dataset(
        &self,
        tenant_id: &str,
        id_or_slug: &str,
        dataset_id: &str,
        request: ReportDatasetQueryRequest,
    ) -> Result<ReportDatasetQueryResponse, ReportServiceError> {
        let report = self.get_report(tenant_id, id_or_slug).await?;
        let dataset = report
            .definition
            .datasets
            .iter()
            .find(|dataset| dataset.id == dataset_id)
            .ok_or_else(|| {
                ReportServiceError::Validation(format!("Unknown report dataset '{}'", dataset_id))
            })?;

        let resolved_filters = resolve_filters(&report.definition, &request.filters);
        let requested_page_size =
            clamp_page_size(request.page.as_ref().map(|page| page.size).unwrap_or(50));
        let offset = request
            .page
            .as_ref()
            .map(|page| page.offset)
            .unwrap_or(0)
            .max(0);
        let requested_limit = request
            .limit
            .map(|limit| limit.clamp(1, MAX_AGGREGATE_ROWS));
        let remaining_limit = requested_limit.map(|limit| limit.saturating_sub(offset));
        let page_size = remaining_limit
            .map(|remaining| requested_page_size.min(remaining))
            .unwrap_or(requested_page_size);
        let compiled = compile_dataset_query(
            "dataset query",
            dataset,
            &request.dimensions,
            &request.measures,
            &request.order_by,
            Some(page_size.max(1)),
        )?;
        if page_size == 0 {
            return Ok(ReportDatasetQueryResponse {
                success: true,
                dataset: ReportDatasetQueryMetadata {
                    id: dataset.id.clone(),
                },
                columns: compiled.columns,
                rows: vec![],
                page: ReportDatasetQueryPage {
                    offset,
                    size: 0,
                    total_count: requested_limit.unwrap_or(0),
                    has_next_page: false,
                },
            });
        }
        let aggregate_request = build_aggregate_request_from_parts(
            "dataset query",
            &compiled.source.group_by,
            &compiled.source.aggregates,
            &compiled.source.order_by,
            Some(page_size),
            Some(offset),
            build_dataset_condition(
                &report.definition,
                dataset,
                &resolved_filters,
                &request.dataset_filters,
                request.search.as_ref(),
            )?,
        )?;

        let result = self
            .instance_service
            .aggregate_instances_by_schema(
                tenant_id,
                &compiled.source.schema,
                aggregate_request,
                compiled.source.connection_id.as_deref(),
            )
            .await
            .map_err(map_object_model_error)?;

        Ok(ReportDatasetQueryResponse {
            success: true,
            dataset: ReportDatasetQueryMetadata {
                id: dataset.id.clone(),
            },
            columns: compiled.columns,
            rows: result.rows,
            page: ReportDatasetQueryPage {
                offset,
                size: page_size,
                total_count: requested_limit
                    .map(|limit| result.group_count.min(limit))
                    .unwrap_or(result.group_count),
                has_next_page: offset + page_size
                    < requested_limit
                        .map(|limit| result.group_count.min(limit))
                        .unwrap_or(result.group_count),
            },
        })
    }

    async fn save_report_definition(
        &self,
        tenant_id: &str,
        mut report: ReportDto,
    ) -> Result<ReportDto, ReportServiceError> {
        self.validate_definition(tenant_id, &report.definition)
            .await?;
        report.definition_version = report.definition.definition_version;
        report.updated_at = Utc::now();

        self.repo
            .update(tenant_id, &report.id, &report)
            .await
            .map_err(map_sqlx_error)?
            .ok_or(ReportServiceError::NotFound)
    }

    async fn validate_definition(
        &self,
        tenant_id: &str,
        definition: &ReportDefinition,
    ) -> Result<(), ReportServiceError> {
        let mut filter_ids = HashSet::new();
        for filter in &definition.filters {
            if filter.id.trim().is_empty() {
                return Err(ReportServiceError::Validation(
                    "Report filter IDs cannot be empty".to_string(),
                ));
            }
            if !filter_ids.insert(filter.id.clone()) {
                return Err(ReportServiceError::Validation(format!(
                    "Duplicate report filter ID '{}'",
                    filter.id
                )));
            }
        }
        let report_condition_filter_defs = report_filter_definitions_by_id(&definition.filters);
        for filter in &definition.filters {
            self.validate_filter_options(tenant_id, filter, &filter_ids)
                .await?;
            validate_filter_option_condition_filter_refs(filter, &report_condition_filter_defs)?;
        }

        let mut view_ids = HashSet::new();
        for view in &definition.views {
            if view.id.trim().is_empty() {
                return Err(ReportServiceError::Validation(
                    "Report view IDs cannot be empty".to_string(),
                ));
            }
            if !view_ids.insert(view.id.clone()) {
                return Err(ReportServiceError::Validation(format!(
                    "Duplicate report view ID '{}'",
                    view.id
                )));
            }
        }
        validate_report_view_navigation(&definition.views, &view_ids, &filter_ids)?;

        let mut dataset_ids = HashSet::new();
        for dataset in &definition.datasets {
            if dataset.id.trim().is_empty() {
                return Err(ReportServiceError::Validation(
                    "Report dataset IDs cannot be empty".to_string(),
                ));
            }
            if !dataset_ids.insert(dataset.id.clone()) {
                return Err(ReportServiceError::Validation(format!(
                    "Duplicate report dataset ID '{}'",
                    dataset.id
                )));
            }
            self.validate_dataset_definition(tenant_id, dataset).await?;
        }

        let mut block_ids = HashSet::new();
        let mut block_types = HashMap::new();
        for block in &definition.blocks {
            if block.id.trim().is_empty() {
                return Err(ReportServiceError::Validation(
                    "Report block IDs cannot be empty".to_string(),
                ));
            }
            if !block_ids.insert(block.id.clone()) {
                return Err(ReportServiceError::Validation(format!(
                    "Duplicate report block ID '{}'",
                    block.id
                )));
            }
            validate_show_when_value(
                block.show_when.as_ref(),
                &format!("block '{}'", block.id),
                &filter_ids,
            )?;
            block_types.insert(block.id.clone(), block.block_type);
            let block_condition_filter_defs = block_condition_filter_definitions(definition, block);
            let markdown_placeholders = if block.block_type == ReportBlockType::Markdown {
                validate_report_markdown_block_shape(block)?
            } else {
                Vec::new()
            };
            if block.block_type == ReportBlockType::Actions
                && block.source.kind != ReportSourceKind::WorkflowRuntime
            {
                return Err(ReportServiceError::Validation(format!(
                    "Block '{}' actions block requires a workflow_runtime source",
                    block.id
                )));
            }
            if block.block_type == ReportBlockType::Markdown
                && block.dataset.is_none()
                && block.source.is_empty()
            {
                validate_report_markdown_placeholders_have_no_source(
                    block,
                    &markdown_placeholders,
                )?;
                validate_block_interactions(block, &filter_ids, &view_ids)?;
                continue;
            }
            if let Some(dataset_query) = &block.dataset {
                if !block.source.is_empty() {
                    return Err(ReportServiceError::Validation(format!(
                        "Block '{}' must use either dataset or source, not both",
                        block.id
                    )));
                }
                let dataset = definition
                    .datasets
                    .iter()
                    .find(|dataset| dataset.id == dataset_query.id)
                    .ok_or_else(|| {
                        ReportServiceError::Validation(format!(
                            "Block '{}' references unknown dataset '{}'",
                            block.id, dataset_query.id
                        ))
                    })?;
                let compiled = compile_dataset_query(
                    &block.id,
                    dataset,
                    &dataset_query.dimensions,
                    &dataset_query.measures,
                    &dataset_query.order_by,
                    dataset_query.limit,
                )?;
                build_dataset_condition(
                    definition,
                    dataset,
                    &HashMap::new(),
                    &dataset_query.dataset_filters,
                    None,
                )?;
                validate_dataset_block_output(block, &compiled.source)?;
                if let Some(table) = &block.table {
                    validate_report_table_interaction_button_columns(
                        table,
                        &filter_ids,
                        &view_ids,
                        &|field| dataset_output_field_known(&compiled.source, field),
                        &format!("block '{}'", block.id),
                    )?;
                }
                validate_report_markdown_placeholders(block, &markdown_placeholders, &|field| {
                    dataset_output_field_known(&compiled.source, field)
                })?;
                validate_block_interactions(block, &filter_ids, &view_ids)?;
                continue;
            }
            if block.source.kind == ReportSourceKind::WorkflowRuntime {
                validate_workflow_runtime_block(
                    block,
                    &filter_ids,
                    &view_ids,
                    &block_condition_filter_defs,
                )?;
                let fields = workflow_runtime_fields(workflow_runtime_entity(block)?);
                validate_report_markdown_placeholders(block, &markdown_placeholders, &|field| {
                    markdown_output_field_known(field, &|candidate| {
                        workflow_runtime_row_field_known(&fields, candidate)
                    })
                })?;
                continue;
            }
            if block.source.kind == ReportSourceKind::System {
                validate_system_block(block, &filter_ids, &view_ids, &block_condition_filter_defs)?;
                let fields = system_fields(system_entity(block)?);
                let aggregate_output_fields = aggregate_output_fields(block);
                validate_report_markdown_placeholders(block, &markdown_placeholders, &|field| {
                    markdown_output_field_known(field, &|candidate| match block.source.mode {
                        ReportSourceMode::Filter => system_row_field_known(&fields, candidate),
                        ReportSourceMode::Aggregate => aggregate_output_fields.contains(candidate),
                    })
                })?;
                continue;
            }
            if block.source.schema.trim().is_empty() {
                return Err(ReportServiceError::Validation(format!(
                    "Block '{}' must specify an Object Model schema",
                    block.id
                )));
            }

            let schema = self
                .schema_service
                .get_schema_by_name(
                    &block.source.schema,
                    tenant_id,
                    block.source.connection_id.as_deref(),
                )
                .await
                .map_err(map_object_model_error)?;

            let schema_fields: HashSet<_> = schema
                .columns
                .iter()
                .map(|column| column.name.clone())
                .collect();

            // Resolve dimension schemas for any block-level joins. Each join
            // contributes a `<alias>.<field>` namespace recognized by the
            // field-existence checks below.
            let mut join_field_sets: HashMap<String, HashSet<String>> = HashMap::new();
            for join in &block.source.join {
                let alias = join.effective_alias().to_string();
                if !is_schema_field(&schema_fields, &join.parent_field) {
                    return Err(ReportServiceError::Validation(format!(
                        "Block '{}' join parentField '{}' is not a column on '{}'",
                        block.id, join.parent_field, block.source.schema
                    )));
                }
                let dim_schema = self
                    .schema_service
                    .get_schema_by_name(&join.schema, tenant_id, join.connection_id.as_deref())
                    .await
                    .map_err(map_object_model_error)?;
                let dim_fields: HashSet<String> =
                    dim_schema.columns.iter().map(|c| c.name.clone()).collect();
                if !is_schema_field(&dim_fields, &join.field) {
                    return Err(ReportServiceError::Validation(format!(
                        "Block '{}' join field '{}' is not a column on '{}'",
                        block.id, join.field, join.schema
                    )));
                }
                if join_field_sets.insert(alias.clone(), dim_fields).is_some() {
                    return Err(ReportServiceError::Validation(format!(
                        "Block '{}' has duplicate join alias '{}'",
                        block.id, alias
                    )));
                }
            }

            let is_known_field = |field: &str| -> bool {
                if let Some((alias, dim_field)) = field.split_once('.') {
                    return join_field_sets
                        .get(alias)
                        .map(|fields| is_schema_field(fields, dim_field))
                        .unwrap_or(false);
                }
                is_schema_field(&schema_fields, field)
            };
            let aggregate_output_fields = aggregate_output_fields(block);
            let is_table_value_field = |field: &str| -> bool {
                match block.source.mode {
                    ReportSourceMode::Filter => is_known_field(field),
                    ReportSourceMode::Aggregate => aggregate_output_fields.contains(field),
                }
            };
            validate_report_aggregate_specs(
                &format!("block '{}'", block.id),
                &block.source.aggregates,
            )?;
            if block.source.mode == ReportSourceMode::Aggregate
                && block.source.aggregates.is_empty()
            {
                return Err(ReportServiceError::Validation(format!(
                    "Block '{}' aggregate source must define at least one aggregate",
                    block.id
                )));
            }

            if let Some(table) = &block.table {
                for column in &table.columns {
                    if column.is_chart() {
                        self.validate_table_chart_column(
                            tenant_id,
                            block,
                            &schema_fields,
                            &aggregate_output_fields,
                            column,
                        )
                        .await?;
                    } else if column.is_value_lookup() {
                        self.validate_table_value_column(
                            tenant_id,
                            block,
                            &schema_fields,
                            &aggregate_output_fields,
                            column,
                        )
                        .await?;
                    } else if column.is_workflow_button() {
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
                            is_table_value_field,
                            &format!(
                                "Block '{}' workflow button column '{}'",
                                block.id, column.field
                            ),
                        )?;
                        validate_report_workflow_action_row_conditions(
                            action,
                            is_table_value_field,
                            &format!(
                                "Block '{}' workflow button column '{}'",
                                block.id, column.field
                            ),
                        )?;
                    } else if column.is_interaction_buttons() {
                        validate_report_interaction_buttons(
                            &column.interaction_buttons,
                            &filter_ids,
                            &view_ids,
                            &is_table_value_field,
                            &format!(
                                "Block '{}' interaction button column '{}'",
                                block.id, column.field
                            ),
                        )?;
                    } else if !is_table_value_field(&column.field) {
                        return Err(ReportServiceError::Validation(format!(
                            "Block '{}' references unknown table field '{}'",
                            block.id, column.field
                        )));
                    }
                    if let Some(display_field) = &column.display_field
                        && !is_table_value_field(display_field)
                    {
                        return Err(ReportServiceError::Validation(format!(
                            "Block '{}' references unknown table displayField '{}'",
                            block.id, display_field
                        )));
                    }
                    if let Some(editor) = &column.editor {
                        self.validate_editor_config(
                            tenant_id,
                            editor,
                            &block_condition_filter_defs,
                            &format!("block '{}' table column '{}'", block.id, column.field),
                        )
                        .await?;
                    }
                }

                for sort in &table.default_sort {
                    if !is_table_value_field(&sort.field) {
                        return Err(ReportServiceError::Validation(format!(
                            "Block '{}' references unknown table sort field '{}'",
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

            if let Some(card) = &block.card {
                for group in &card.groups {
                    for field in &group.fields {
                        let is_workflow_button = field.kind == ReportCardFieldKind::WorkflowButton
                            || field.workflow_action.is_some();
                        if is_workflow_button {
                            let action = field.workflow_action.as_ref().ok_or_else(|| {
                                ReportServiceError::Validation(format!(
                                    "Block '{}' workflow button card field '{}' must define workflowAction",
                                    block.id, field.field
                                ))
                            })?;
                            validate_report_workflow_action_config(
                                action,
                                &format!("block '{}' card field '{}'", block.id, field.field),
                            )?;
                            validate_report_workflow_action_context_field(
                                action,
                                field.field.as_str(),
                                is_known_field,
                                &format!(
                                    "Block '{}' workflow button card field '{}'",
                                    block.id, field.field
                                ),
                            )?;
                            validate_report_workflow_action_row_conditions(
                                action,
                                is_known_field,
                                &format!(
                                    "Block '{}' workflow button card field '{}'",
                                    block.id, field.field
                                ),
                            )?;
                        } else if !is_known_field(&field.field) {
                            return Err(ReportServiceError::Validation(format!(
                                "Block '{}' references unknown card field '{}'",
                                block.id, field.field
                            )));
                        }
                        if let Some(display_field) = &field.display_field
                            && !is_known_field(display_field)
                        {
                            return Err(ReportServiceError::Validation(format!(
                                "Block '{}' references unknown card displayField '{}'",
                                block.id, display_field
                            )));
                        }
                        if let Some(editor) = &field.editor {
                            self.validate_editor_config(
                                tenant_id,
                                editor,
                                &block_condition_filter_defs,
                                &format!("block '{}' card field '{}'", block.id, field.field),
                            )
                            .await?;
                        }
                    }
                }
            }

            for group_field in &block.source.group_by {
                if !is_known_field(group_field) {
                    return Err(ReportServiceError::Validation(format!(
                        "Block '{}' references unknown groupBy field '{}'",
                        block.id, group_field
                    )));
                }
            }

            for order_by in &block.source.order_by {
                let is_known_order_field = match block.source.mode {
                    ReportSourceMode::Filter => is_known_field(&order_by.field),
                    ReportSourceMode::Aggregate => {
                        aggregate_output_fields.contains(&order_by.field)
                    }
                };
                if !is_known_order_field {
                    return Err(ReportServiceError::Validation(format!(
                        "Block '{}' references unknown orderBy field '{}'",
                        block.id, order_by.field
                    )));
                }
            }

            for aggregate in &block.source.aggregates {
                if let Some(field) = &aggregate.field
                    && !is_known_field(field)
                {
                    return Err(ReportServiceError::Validation(format!(
                        "Block '{}' references unknown aggregate field '{}'",
                        block.id, field
                    )));
                }
                for order_by in &aggregate.order_by {
                    if !is_known_field(&order_by.field) {
                        return Err(ReportServiceError::Validation(format!(
                            "Block '{}' aggregate '{}' references unknown orderBy field '{}'",
                            block.id, aggregate.alias, order_by.field
                        )));
                    }
                }
            }

            validate_report_condition_filter_refs(
                block.source.condition.as_ref(),
                &block_condition_filter_defs,
                &format!("block '{}'", block.id),
            )?;
            validate_report_condition_field_refs(
                block.source.condition.as_ref(),
                &is_known_field,
                &format!("block '{}'", block.id),
            )?;
            validate_report_source_filter_mappings(
                &block.source.filter_mappings,
                &filter_ids,
                &is_known_field,
                "source.filterMappings",
                &format!("block '{}'", block.id),
            )?;
            validate_report_markdown_placeholders(block, &markdown_placeholders, &|field| {
                markdown_output_field_known(field, &|candidate| match block.source.mode {
                    ReportSourceMode::Filter => is_known_field(candidate),
                    ReportSourceMode::Aggregate => aggregate_output_fields.contains(candidate),
                })
            })?;
            self.validate_report_condition_subqueries(
                tenant_id,
                block.source.condition.as_ref(),
                block.source.connection_id.as_deref(),
                &format!("block '{}'", block.id),
            )
            .await?;
            if let Some(table) = &block.table {
                for column in &table.columns {
                    if let Some(source) = &column.source {
                        validate_report_condition_filter_refs(
                            source.condition.as_ref(),
                            &block_condition_filter_defs,
                            &format!("block '{}' table column '{}'", block.id, column.field),
                        )?;
                        self.validate_report_condition_subqueries(
                            tenant_id,
                            source.condition.as_ref(),
                            source.connection_id.as_deref(),
                            &format!("block '{}' table column '{}'", block.id, column.field),
                        )
                        .await?;
                    }
                }
            }

            validate_block_interactions(block, &filter_ids, &view_ids)?;
        }

        let mut layout_node_ids = HashSet::new();
        for (index, node) in definition.layout.iter().enumerate() {
            let node_value = serde_json::to_value(node).map_err(|err| {
                ReportServiceError::Validation(format!(
                    "Could not serialize layout node {} for validation: {}",
                    index, err
                ))
            })?;
            validate_layout_node(
                &node_value,
                &format!("$.layout[{index}]"),
                &block_ids,
                &block_types,
                &filter_ids,
                &mut layout_node_ids,
            )?;
        }

        for (view_index, view) in definition.views.iter().enumerate() {
            for (breadcrumb_index, breadcrumb) in view.breadcrumb.iter().enumerate() {
                if breadcrumb.label.trim().is_empty() {
                    return Err(ReportServiceError::Validation(format!(
                        "Report view '{}' breadcrumb {} must include a label",
                        view.id, breadcrumb_index
                    )));
                }
                if let Some(view_id) = breadcrumb.view_id.as_deref()
                    && !view_ids.contains(view_id)
                {
                    return Err(ReportServiceError::Validation(format!(
                        "Report view '{}' breadcrumb {} references unknown view '{}'",
                        view.id, breadcrumb_index, view_id
                    )));
                }
                for filter_id in &breadcrumb.clear_filters {
                    if !filter_ids.contains(filter_id) {
                        return Err(ReportServiceError::Validation(format!(
                            "Report view '{}' breadcrumb {} references unknown filter '{}'",
                            view.id, breadcrumb_index, filter_id
                        )));
                    }
                }
            }

            let mut view_layout_node_ids = HashSet::new();
            for (node_index, node) in view.layout.iter().enumerate() {
                let node_value = serde_json::to_value(node).map_err(|err| {
                    ReportServiceError::Validation(format!(
                        "Could not serialize report view '{}' layout node {} for validation: {}",
                        view.id, node_index, err
                    ))
                })?;
                validate_layout_node(
                    &node_value,
                    &format!("$.views[{view_index}].layout[{node_index}]"),
                    &block_ids,
                    &block_types,
                    &filter_ids,
                    &mut view_layout_node_ids,
                )?;
            }
        }

        self.validate_workflow_references(tenant_id, definition)
            .await?;

        Ok(())
    }

    async fn validate_workflow_references(
        &self,
        tenant_id: &str,
        definition: &ReportDefinition,
    ) -> Result<(), ReportServiceError> {
        for block in &definition.blocks {
            if block.source.kind == ReportSourceKind::WorkflowRuntime {
                let workflow_id = workflow_runtime_workflow_id(block)?;
                self.validate_workflow_exists(
                    tenant_id,
                    workflow_id,
                    &format!("block '{}' workflow_runtime source", block.id),
                )
                .await?;
            }

            if let Some(table) = &block.table {
                for column in &table.columns {
                    if let Some(action) = &column.workflow_action {
                        self.validate_workflow_action_reference(
                            tenant_id,
                            action,
                            &format!("block '{}' table column '{}'", block.id, column.field),
                        )
                        .await?;
                    }
                }
                for action in &table.actions {
                    self.validate_workflow_action_reference(
                        tenant_id,
                        &action.workflow_action,
                        &format!("block '{}' table action '{}'", block.id, action.id),
                    )
                    .await?;
                }
            }

            if let Some(card) = &block.card {
                let mut actions = Vec::new();
                collect_card_workflow_actions(
                    card,
                    &format!("block '{}' card", block.id),
                    &mut actions,
                );
                for (action, context) in actions {
                    self.validate_workflow_action_reference(tenant_id, action, &context)
                        .await?;
                }
            }
        }

        Ok(())
    }

    async fn validate_workflow_exists(
        &self,
        tenant_id: &str,
        workflow_id: &str,
        context: &str,
    ) -> Result<(), ReportServiceError> {
        let exists = self
            .workflow_repo
            .exists(tenant_id, workflow_id)
            .await
            .map_err(map_sqlx_error)?;
        if !exists {
            return Err(report_validation_error(
                "$",
                "UNKNOWN_WORKFLOW",
                format!("{context} references unknown workflow '{}'", workflow_id),
                Some(
                    "Create the workflow first or update workflowId to an existing workflow."
                        .to_string(),
                ),
            ));
        }
        Ok(())
    }

    async fn validate_workflow_action_reference(
        &self,
        tenant_id: &str,
        action: &ReportWorkflowActionConfig,
        context: &str,
    ) -> Result<(), ReportServiceError> {
        self.validate_workflow_exists(tenant_id, &action.workflow_id, context)
            .await?;
        if let Some(version) = action.version {
            if version <= 0 {
                return Err(report_validation_error(
                    "$",
                    "INVALID_WORKFLOW_VERSION",
                    format!("{context} workflowAction.version must be greater than zero"),
                    Some("Omit version to use the current/latest workflow version, or set a positive version.".to_string()),
                ));
            }
            let exists = self
                .workflow_repo
                .version_exists(tenant_id, &action.workflow_id, version)
                .await
                .map_err(map_sqlx_error)?;
            if !exists {
                return Err(report_validation_error(
                    "$",
                    "UNKNOWN_WORKFLOW_VERSION",
                    format!(
                        "{context} references unknown workflow '{}' version {}",
                        action.workflow_id, version
                    ),
                    Some("Use an existing workflow version or omit version.".to_string()),
                ));
            }
        }
        Ok(())
    }

    async fn validate_dataset_definition(
        &self,
        tenant_id: &str,
        dataset: &ReportDatasetDefinition,
    ) -> Result<(), ReportServiceError> {
        if dataset.label.trim().is_empty() {
            return Err(ReportServiceError::Validation(format!(
                "Dataset '{}' must include a label",
                dataset.id
            )));
        }
        if dataset.source.schema.trim().is_empty() {
            return Err(ReportServiceError::Validation(format!(
                "Dataset '{}' must specify an Object Model schema",
                dataset.id
            )));
        }

        let schema = self
            .schema_service
            .get_schema_by_name(
                &dataset.source.schema,
                tenant_id,
                dataset.source.connection_id.as_deref(),
            )
            .await
            .map_err(map_object_model_error)?;
        let schema_fields: HashSet<_> = schema
            .columns
            .iter()
            .map(|column| column.name.clone())
            .collect();

        if let Some(time_dimension) = &dataset.time_dimension
            && !is_schema_field(&schema_fields, time_dimension)
        {
            return Err(ReportServiceError::Validation(format!(
                "Dataset '{}' timeDimension '{}' is not a column on '{}'",
                dataset.id, time_dimension, dataset.source.schema
            )));
        }

        let mut dimension_ids = HashSet::new();
        for dimension in &dataset.dimensions {
            if dimension.field.trim().is_empty() {
                return Err(ReportServiceError::Validation(format!(
                    "Dataset '{}' dimension fields cannot be empty",
                    dataset.id
                )));
            }
            if dimension.label.trim().is_empty() {
                return Err(ReportServiceError::Validation(format!(
                    "Dataset '{}' dimension '{}' must include a label",
                    dataset.id, dimension.field
                )));
            }
            if !dimension_ids.insert(dimension.field.clone()) {
                return Err(ReportServiceError::Validation(format!(
                    "Dataset '{}' has duplicate dimension '{}'",
                    dataset.id, dimension.field
                )));
            }
            if !is_schema_field(&schema_fields, &dimension.field) {
                return Err(ReportServiceError::Validation(format!(
                    "Dataset '{}' dimension '{}' is not a column on '{}'",
                    dataset.id, dimension.field, dataset.source.schema
                )));
            }
        }

        let mut measure_ids = HashSet::new();
        for measure in &dataset.measures {
            if measure.id.trim().is_empty() {
                return Err(ReportServiceError::Validation(format!(
                    "Dataset '{}' measure IDs cannot be empty",
                    dataset.id
                )));
            }
            if measure.label.trim().is_empty() {
                return Err(ReportServiceError::Validation(format!(
                    "Dataset '{}' measure '{}' must include a label",
                    dataset.id, measure.id
                )));
            }
            if dimension_ids.contains(&measure.id) {
                return Err(ReportServiceError::Validation(format!(
                    "Dataset '{}' measure '{}' conflicts with a dimension field",
                    dataset.id, measure.id
                )));
            }
            if !measure_ids.insert(measure.id.clone()) {
                return Err(ReportServiceError::Validation(format!(
                    "Dataset '{}' has duplicate measure '{}'",
                    dataset.id, measure.id
                )));
            }
            validate_report_aggregate_spec_shape(
                &format!("Dataset '{}' measure '{}'", dataset.id, measure.id),
                measure.op,
                measure.field.as_deref(),
                measure.distinct,
                &measure.order_by,
                measure.expression.as_ref(),
                measure.percentile,
            )?;
            if let Some(field) = &measure.field
                && !is_schema_field(&schema_fields, field)
            {
                return Err(ReportServiceError::Validation(format!(
                    "Dataset '{}' measure '{}' field '{}' is not a column on '{}'",
                    dataset.id, measure.id, field, dataset.source.schema
                )));
            }
            if measure.op != ReportAggregateFn::Count
                && measure.op != ReportAggregateFn::Expr
                && !matches!(
                    measure.op,
                    ReportAggregateFn::PercentileCont | ReportAggregateFn::PercentileDisc
                )
                && measure
                    .field
                    .as_ref()
                    .is_none_or(|field| field.trim().is_empty())
            {
                return Err(ReportServiceError::Validation(format!(
                    "Dataset '{}' measure '{}' requires field for op {:?}",
                    dataset.id, measure.id, measure.op
                )));
            }
            if measure.op == ReportAggregateFn::Expr && measure.expression.is_none() {
                return Err(ReportServiceError::Validation(format!(
                    "Dataset '{}' measure '{}' requires expression for op expr",
                    dataset.id, measure.id
                )));
            }
            for order_by in &measure.order_by {
                if !is_schema_field(&schema_fields, &order_by.field) {
                    return Err(ReportServiceError::Validation(format!(
                        "Dataset '{}' measure '{}' references unknown orderBy field '{}'",
                        dataset.id, measure.id, order_by.field
                    )));
                }
            }
        }

        Ok(())
    }

    async fn validate_filter_options(
        &self,
        tenant_id: &str,
        filter: &ReportFilterDefinition,
        filter_ids: &HashSet<String>,
    ) -> Result<(), ReportServiceError> {
        let Some(options) = filter.options.as_ref().and_then(Value::as_object) else {
            return Ok(());
        };
        if options
            .get("source")
            .and_then(Value::as_str)
            .unwrap_or("static")
            != "object_model"
        {
            return Ok(());
        }

        let schema_name = option_string(options, "schema").ok_or_else(|| {
            ReportServiceError::Validation(format!(
                "Filter '{}' object_model options must include schema",
                filter.id
            ))
        })?;
        let value_field = option_string(options, "field")
            .or_else(|| option_string(options, "valueField"))
            .ok_or_else(|| {
                ReportServiceError::Validation(format!(
                    "Filter '{}' object_model options must include field",
                    filter.id
                ))
            })?;
        let label_field = option_string(options, "labelField").unwrap_or(value_field);
        let connection_id = option_string(options, "connectionId");
        let schema = self
            .schema_service
            .get_schema_by_name(schema_name, tenant_id, connection_id)
            .await
            .map_err(map_object_model_error)?;
        let schema_fields: HashSet<_> = schema
            .columns
            .iter()
            .map(|column| column.name.clone())
            .collect();

        for field in [value_field, label_field] {
            if !is_schema_field(&schema_fields, field) {
                return Err(ReportServiceError::Validation(format!(
                    "Filter '{}' options reference unknown field '{}'",
                    filter.id, field
                )));
            }
        }

        if let Some(condition) = options.get("condition") {
            let condition =
                serde_json::from_value::<Condition>(condition.clone()).map_err(|err| {
                    ReportServiceError::Validation(format!(
                        "Filter '{}' options.condition is invalid: {}",
                        filter.id, err
                    ))
                })?;
            validate_report_condition_field_refs(
                Some(&condition),
                &|field| is_schema_field(&schema_fields, field),
                &format!("filter '{}'", filter.id),
            )?;
            self.validate_report_condition_subqueries(
                tenant_id,
                Some(&condition),
                connection_id,
                &format!("filter '{}'", filter.id),
            )
            .await?;
        }
        if let Some(mappings) = options.get("filterMappings") {
            let mappings = serde_json::from_value::<Vec<ReportFilterTarget>>(mappings.clone())
                .map_err(|err| {
                    ReportServiceError::Validation(format!(
                        "Filter '{}' options.filterMappings is invalid: {}",
                        filter.id, err
                    ))
                })?;
            validate_report_source_filter_mappings(
                &mappings,
                filter_ids,
                &|field| is_schema_field(&schema_fields, field),
                "options.filterMappings",
                &format!("filter '{}'", filter.id),
            )?;
        }

        Ok(())
    }

    async fn validate_editor_config(
        &self,
        tenant_id: &str,
        editor: &ReportEditorConfig,
        filter_defs: &HashMap<String, &ReportFilterDefinition>,
        context: &str,
    ) -> Result<(), ReportServiceError> {
        if editor.kind != ReportEditorKind::Lookup {
            if editor.lookup.is_some() {
                return Err(ReportServiceError::Validation(format!(
                    "{} editor.lookup is only valid when editor.kind is 'lookup'",
                    context
                )));
            }
            return Ok(());
        }

        let lookup = editor.lookup.as_ref().ok_or_else(|| {
            ReportServiceError::Validation(format!(
                "{} editor.kind='lookup' requires editor.lookup",
                context
            ))
        })?;
        self.validate_lookup_config(tenant_id, lookup, filter_defs, context)
            .await
    }

    async fn validate_lookup_config(
        &self,
        tenant_id: &str,
        lookup: &ReportLookupConfig,
        filter_defs: &HashMap<String, &ReportFilterDefinition>,
        context: &str,
    ) -> Result<(), ReportServiceError> {
        if lookup.schema.trim().is_empty() {
            return Err(ReportServiceError::Validation(format!(
                "{} lookup.schema is required",
                context
            )));
        }
        if lookup.value_field.trim().is_empty() {
            return Err(ReportServiceError::Validation(format!(
                "{} lookup.valueField is required",
                context
            )));
        }
        if lookup.label_field.trim().is_empty() {
            return Err(ReportServiceError::Validation(format!(
                "{} lookup.labelField is required",
                context
            )));
        }

        let schema = self
            .schema_service
            .get_schema_by_name(&lookup.schema, tenant_id, lookup.connection_id.as_deref())
            .await
            .map_err(map_object_model_error)?;
        let schema_fields: HashSet<_> = schema
            .columns
            .iter()
            .map(|column| column.name.clone())
            .collect();

        for field in std::iter::once(&lookup.value_field)
            .chain(std::iter::once(&lookup.label_field))
            .chain(lookup.search_fields.iter())
        {
            if !is_schema_field(&schema_fields, field) {
                return Err(ReportServiceError::Validation(format!(
                    "{} lookup references unknown field '{}' on '{}'",
                    context, field, lookup.schema
                )));
            }
        }

        validate_report_condition_filter_refs(
            lookup.condition.as_ref(),
            filter_defs,
            &format!("{} lookup", context),
        )?;
        validate_report_condition_field_refs(
            lookup.condition.as_ref(),
            &|field| is_schema_field(&schema_fields, field),
            &format!("{} lookup", context),
        )?;
        self.validate_report_condition_subqueries(
            tenant_id,
            lookup.condition.as_ref(),
            lookup.connection_id.as_deref(),
            &format!("{} lookup", context),
        )
        .await?;

        for mapping in &lookup.filter_mappings {
            let filter_id = mapping.filter_id.as_deref().ok_or_else(|| {
                ReportServiceError::Validation(format!(
                    "{} lookup filterMappings entries must include filterId",
                    context
                ))
            })?;
            if !filter_defs.contains_key(filter_id) {
                return Err(ReportServiceError::Validation(format!(
                    "{} lookup filterMappings references unknown filter '{}'",
                    context, filter_id
                )));
            }
            if !is_schema_field(&schema_fields, &mapping.field) {
                return Err(ReportServiceError::Validation(format!(
                    "{} lookup filterMappings references unknown field '{}' on '{}'",
                    context, mapping.field, lookup.schema
                )));
            }
        }

        Ok(())
    }

    async fn validate_report_condition_subqueries(
        &self,
        tenant_id: &str,
        condition: Option<&Condition>,
        parent_connection_id: Option<&str>,
        context: &str,
    ) -> Result<(), ReportServiceError> {
        let Some(condition) = condition else {
            return Ok(());
        };
        let mut stack = vec![(condition.clone(), false)];

        while let Some((condition, inside_subquery)) = stack.pop() {
            let Some(arguments) = condition.arguments.as_ref() else {
                continue;
            };
            let op = condition.op.to_ascii_uppercase();
            for (index, argument) in arguments.iter().enumerate() {
                if let Some(subquery) = parse_report_condition_subquery_operand(argument)? {
                    if !matches!(op.as_str(), "IN" | "NOT_IN") || index != 1 {
                        return Err(ReportServiceError::Validation(format!(
                            "{} condition subqueries are only supported as the second argument of IN or NOT_IN",
                            context
                        )));
                    }
                    if inside_subquery {
                        return Err(ReportServiceError::Validation(format!(
                            "{} condition contains a nested subquery; nested subqueries are not supported",
                            context
                        )));
                    }

                    let subquery_connection_id = subquery
                        .get("connectionId")
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|value| !value.is_empty());
                    if let Some(subquery_connection_id) = subquery_connection_id
                        && Some(subquery_connection_id) != parent_connection_id
                    {
                        return Err(ReportServiceError::Validation(format!(
                            "{} condition subquery must use the same Object Store connection as its parent source",
                            context
                        )));
                    }

                    let schema_name = subquery
                        .get("schema")
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .unwrap_or_default();
                    let select_field = subquery
                        .get("select")
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .unwrap_or_default();
                    let schema = self
                        .schema_service
                        .get_schema_by_name(schema_name, tenant_id, parent_connection_id)
                        .await
                        .map_err(map_object_model_error)?;
                    let schema_fields: HashSet<_> = schema
                        .columns
                        .iter()
                        .map(|column| column.name.clone())
                        .collect();
                    if !is_schema_field(&schema_fields, select_field) {
                        return Err(ReportServiceError::Validation(format!(
                            "{} condition subquery select field '{}' is not a column on '{}'",
                            context, select_field, schema_name
                        )));
                    }

                    if let Some(child) = subquery.get("condition").and_then(condition_from_value) {
                        validate_report_condition_field_refs(
                            Some(&child),
                            &|field| is_schema_field(&schema_fields, field),
                            &format!("{} condition subquery '{}'", context, schema_name),
                        )?;
                        stack.push((child, true));
                    }
                    continue;
                }

                if let Some(child) = condition_from_value(argument) {
                    stack.push((child, inside_subquery));
                }
            }
        }

        Ok(())
    }

    async fn validate_table_chart_column(
        &self,
        tenant_id: &str,
        block: &ReportBlockDefinition,
        parent_schema_fields: &HashSet<String>,
        parent_output_fields: &HashSet<String>,
        column: &ReportTableColumn,
    ) -> Result<(), ReportServiceError> {
        let Some(source) = &column.source else {
            return Err(ReportServiceError::Validation(format!(
                "Block '{}' chart table column '{}' must define source",
                block.id, column.field
            )));
        };
        let Some(chart) = &column.chart else {
            return Err(ReportServiceError::Validation(format!(
                "Block '{}' chart table column '{}' must define chart",
                block.id, column.field
            )));
        };
        if source.mode != ReportSourceMode::Aggregate {
            return Err(ReportServiceError::Validation(format!(
                "Block '{}' chart table column '{}' source.mode must be aggregate",
                block.id, column.field
            )));
        }
        if source.schema.trim().is_empty() {
            return Err(ReportServiceError::Validation(format!(
                "Block '{}' chart table column '{}' must specify an Object Model schema",
                block.id, column.field
            )));
        }

        let nested_schema = self
            .schema_service
            .get_schema_by_name(&source.schema, tenant_id, source.connection_id.as_deref())
            .await
            .map_err(map_object_model_error)?;
        let nested_schema_fields: HashSet<_> = nested_schema
            .columns
            .iter()
            .map(|field| field.name.clone())
            .collect();
        let is_nested_field =
            |field: &str| -> bool { is_schema_field(&nested_schema_fields, field) };
        let nested_output_fields =
            aggregate_source_output_fields(&source.group_by, &source.aggregates);
        validate_report_aggregate_specs(
            &format!("block '{}' chart table column '{}'", block.id, column.field),
            &source.aggregates,
        )?;
        if source.aggregates.is_empty() {
            return Err(ReportServiceError::Validation(format!(
                "Block '{}' chart table column '{}' aggregate source must define at least one aggregate",
                block.id, column.field
            )));
        }
        validate_report_condition_field_refs(
            source.condition.as_ref(),
            &is_nested_field,
            &format!("block '{}' chart table column '{}'", block.id, column.field),
        )?;

        for join in &source.join {
            let parent_field_ok = match block.source.mode {
                ReportSourceMode::Filter => {
                    is_schema_field(parent_schema_fields, &join.parent_field)
                }
                ReportSourceMode::Aggregate => parent_output_fields.contains(&join.parent_field),
            };
            if !parent_field_ok {
                return Err(ReportServiceError::Validation(format!(
                    "Block '{}' chart table column '{}' references unknown parent join field '{}'",
                    block.id, column.field, join.parent_field
                )));
            }
            if !is_nested_field(&join.field) {
                return Err(ReportServiceError::Validation(format!(
                    "Block '{}' chart table column '{}' references unknown join field '{}'",
                    block.id, column.field, join.field
                )));
            }
        }

        for group_field in &source.group_by {
            if !is_nested_field(group_field) {
                return Err(ReportServiceError::Validation(format!(
                    "Block '{}' chart table column '{}' references unknown groupBy field '{}'",
                    block.id, column.field, group_field
                )));
            }
        }
        for aggregate in &source.aggregates {
            if let Some(field) = &aggregate.field
                && !is_nested_field(field)
            {
                return Err(ReportServiceError::Validation(format!(
                    "Block '{}' chart table column '{}' aggregate '{}' references unknown field '{}'",
                    block.id, column.field, aggregate.alias, field
                )));
            }
            for order_by in &aggregate.order_by {
                if !is_nested_field(&order_by.field) {
                    return Err(ReportServiceError::Validation(format!(
                        "Block '{}' chart table column '{}' aggregate '{}' references unknown orderBy field '{}'",
                        block.id, column.field, aggregate.alias, order_by.field
                    )));
                }
            }
        }
        for order_by in &source.order_by {
            if !nested_output_fields.contains(&order_by.field) {
                return Err(ReportServiceError::Validation(format!(
                    "Block '{}' chart table column '{}' references unknown orderBy field '{}'",
                    block.id, column.field, order_by.field
                )));
            }
        }
        if !nested_output_fields.contains(&chart.x) {
            return Err(ReportServiceError::Validation(format!(
                "Block '{}' chart table column '{}' references unknown chart x field '{}'",
                block.id, column.field, chart.x
            )));
        }
        for series in &chart.series {
            if !nested_output_fields.contains(&series.field) {
                return Err(ReportServiceError::Validation(format!(
                    "Block '{}' chart table column '{}' references unknown chart series field '{}'",
                    block.id, column.field, series.field
                )));
            }
        }

        Ok(())
    }

    async fn validate_table_value_column(
        &self,
        tenant_id: &str,
        block: &ReportBlockDefinition,
        parent_schema_fields: &HashSet<String>,
        parent_output_fields: &HashSet<String>,
        column: &ReportTableColumn,
    ) -> Result<(), ReportServiceError> {
        let Some(source) = &column.source else {
            return Err(ReportServiceError::Validation(format!(
                "Block '{}' value table column '{}' must define source",
                block.id, column.field
            )));
        };
        if source.mode != ReportSourceMode::Filter {
            return Err(ReportServiceError::Validation(format!(
                "Block '{}' value table column '{}' source.mode must be filter",
                block.id, column.field
            )));
        }
        if !source.group_by.is_empty() || !source.aggregates.is_empty() {
            return Err(ReportServiceError::Validation(format!(
                "Block '{}' value table column '{}' source must not define groupBy or aggregates",
                block.id, column.field
            )));
        }
        let Some(select) = source
            .select
            .as_deref()
            .filter(|select| !select.trim().is_empty())
        else {
            return Err(ReportServiceError::Validation(format!(
                "Block '{}' value table column '{}' source.select is required",
                block.id, column.field
            )));
        };
        if source.join.len() != 1 {
            return Err(ReportServiceError::Validation(format!(
                "Block '{}' value table column '{}' requires exactly one source.join entry",
                block.id, column.field
            )));
        }
        if source.schema.trim().is_empty() {
            return Err(ReportServiceError::Validation(format!(
                "Block '{}' value table column '{}' must specify an Object Model schema",
                block.id, column.field
            )));
        }

        let nested_schema = self
            .schema_service
            .get_schema_by_name(&source.schema, tenant_id, source.connection_id.as_deref())
            .await
            .map_err(map_object_model_error)?;
        let nested_schema_fields: HashSet<_> = nested_schema
            .columns
            .iter()
            .map(|field| field.name.clone())
            .collect();

        if !is_schema_field(&nested_schema_fields, select) {
            return Err(ReportServiceError::Validation(format!(
                "Block '{}' value table column '{}' source.select '{}' is not a column on '{}'",
                block.id, column.field, select, source.schema
            )));
        }
        validate_report_condition_field_refs(
            source.condition.as_ref(),
            &|field| is_schema_field(&nested_schema_fields, field),
            &format!("block '{}' value table column '{}'", block.id, column.field),
        )?;

        let join = &source.join[0];
        let parent_field_ok = match block.source.mode {
            ReportSourceMode::Filter => is_schema_field(parent_schema_fields, &join.parent_field),
            ReportSourceMode::Aggregate => parent_output_fields.contains(&join.parent_field),
        };
        if !parent_field_ok {
            return Err(ReportServiceError::Validation(format!(
                "Block '{}' value table column '{}' references unknown parent join field '{}'",
                block.id, column.field, join.parent_field
            )));
        }
        if !is_schema_field(&nested_schema_fields, &join.field) {
            return Err(ReportServiceError::Validation(format!(
                "Block '{}' value table column '{}' join field '{}' is not a column on '{}'",
                block.id, column.field, join.field, source.schema
            )));
        }
        for order_by in &source.order_by {
            if !is_schema_field(&nested_schema_fields, &order_by.field) {
                return Err(ReportServiceError::Validation(format!(
                    "Block '{}' value table column '{}' references unknown orderBy field '{}'",
                    block.id, column.field, order_by.field
                )));
            }
        }

        Ok(())
    }

    async fn render_block(
        &self,
        tenant_id: &str,
        definition: &ReportDefinition,
        block: &ReportBlockDefinition,
        resolved_filters: &HashMap<String, Value>,
        block_request: Option<&ReportBlockDataRequest>,
    ) -> Result<ReportBlockRenderResult, ReportServiceError> {
        let compiled_block;
        let block = if block.dataset.is_some() {
            compiled_block = compiled_dataset_block(definition, block)?;
            &compiled_block
        } else {
            block
        };

        if let Some(missing) = block_unsatisfied_strict_filter(definition, block, resolved_filters)
        {
            return Ok(ReportBlockRenderResult {
                block_type: block.block_type,
                status: ReportBlockStatus::Empty,
                title: block.title.clone(),
                data: Some(json!({
                    "missing": true,
                    "unsatisfiedFilter": missing,
                    "message": format!(
                        "Required filter '{}' is not set. Open this view through the list/master interaction so the filter is populated.",
                        missing
                    ),
                })),
                error: None,
            });
        }

        let data = match block.block_type {
            ReportBlockType::Table => {
                self.render_table_block(
                    tenant_id,
                    definition,
                    block,
                    resolved_filters,
                    block_request,
                )
                .await?
            }
            ReportBlockType::Chart => {
                self.render_aggregate_block(tenant_id, definition, block, resolved_filters)
                    .await?
            }
            ReportBlockType::Metric => {
                self.render_metric_block(tenant_id, definition, block, resolved_filters)
                    .await?
            }
            ReportBlockType::Actions => {
                self.render_actions_block(
                    tenant_id,
                    definition,
                    block,
                    resolved_filters,
                    block_request,
                )
                .await?
            }
            ReportBlockType::Markdown => {
                self.render_markdown_block(
                    tenant_id,
                    definition,
                    block,
                    resolved_filters,
                    block_request,
                )
                .await?
            }
            ReportBlockType::Card => {
                self.render_card_block(tenant_id, definition, block, resolved_filters)
                    .await?
            }
        };

        let status = if is_empty_data(&data) {
            ReportBlockStatus::Empty
        } else {
            ReportBlockStatus::Ready
        };

        Ok(ReportBlockRenderResult {
            block_type: block.block_type,
            status,
            title: block.title.clone(),
            data: Some(data),
            error: None,
        })
    }

    async fn render_table_block(
        &self,
        tenant_id: &str,
        definition: &ReportDefinition,
        block: &ReportBlockDefinition,
        resolved_filters: &HashMap<String, Value>,
        block_request: Option<&ReportBlockDataRequest>,
    ) -> Result<Value, ReportServiceError> {
        if block.source.kind == ReportSourceKind::System {
            return self
                .render_system_table_block(
                    tenant_id,
                    definition,
                    block,
                    resolved_filters,
                    block_request,
                )
                .await;
        }
        if block.source.kind == ReportSourceKind::WorkflowRuntime {
            return self
                .render_workflow_runtime_table_block(
                    tenant_id,
                    definition,
                    block,
                    resolved_filters,
                    block_request,
                )
                .await;
        }
        if block.source.mode == ReportSourceMode::Aggregate {
            return self
                .render_aggregate_table_block(
                    tenant_id,
                    definition,
                    block,
                    resolved_filters,
                    block_request,
                )
                .await;
        }
        if !block.source.join.is_empty() {
            return self
                .render_joined_filter_table_block(
                    tenant_id,
                    definition,
                    block,
                    resolved_filters,
                    block_request,
                )
                .await;
        }

        let table = block.table.as_ref();
        let page_size = clamp_page_size(
            block_request
                .and_then(|request| request.page.as_ref().map(|page| page.size))
                .or_else(|| {
                    table
                        .and_then(|table| table.pagination.as_ref())
                        .map(|p| p.default_page_size)
                })
                .unwrap_or(50),
        );
        let offset = block_request
            .and_then(|request| request.page.as_ref().map(|page| page.offset))
            .unwrap_or(0)
            .max(0);

        let sort = block_request
            .map(|request| request.sort.clone())
            .filter(|sort| !sort.is_empty())
            .or_else(|| {
                table
                    .map(|table| table.default_sort.clone())
                    .filter(|sort| !sort.is_empty())
            })
            .unwrap_or_else(|| block.source.order_by.clone());

        let filter_request = FilterRequest {
            offset,
            limit: page_size,
            condition: build_block_condition(definition, block, resolved_filters, block_request)?,
            sort_by: if sort.is_empty() {
                None
            } else {
                Some(sort.iter().map(|entry| entry.field.clone()).collect())
            },
            sort_order: if sort.is_empty() {
                None
            } else {
                Some(
                    sort.iter()
                        .map(|entry| normalize_sort_direction(&entry.direction))
                        .collect(),
                )
            },
            score_expression: None,
            order_by: None,
        };

        let (instances, total_count) = self
            .instance_service
            .filter_instances_by_schema(
                tenant_id,
                &block.source.schema,
                filter_request,
                block.source.connection_id.as_deref(),
            )
            .await
            .map_err(map_object_model_error)?;

        let rows: Vec<_> = instances.into_iter().map(flatten_instance).collect();
        let condition_context = ReportConditionRuntimeContext {
            definition,
            block,
            resolved_filters,
            block_request,
        };
        let columns = table_response_columns(table);
        let rows = self
            .hydrate_table_chart_columns(tenant_id, condition_context, table, rows)
            .await?;

        Ok(json!({
            "columns": columns,
            "rows": rows,
            "page": {
                "offset": offset,
                "size": page_size,
                "totalCount": total_count,
                "hasNextPage": offset + page_size < total_count
            }
        }))
    }

    async fn render_workflow_runtime_table_block(
        &self,
        tenant_id: &str,
        definition: &ReportDefinition,
        block: &ReportBlockDefinition,
        resolved_filters: &HashMap<String, Value>,
        block_request: Option<&ReportBlockDataRequest>,
    ) -> Result<Value, ReportServiceError> {
        let entity = workflow_runtime_entity(block)?;
        let table = block.table.as_ref();
        let page_size = clamp_page_size(
            block_request
                .and_then(|request| request.page.as_ref().map(|page| page.size))
                .or_else(|| {
                    table
                        .and_then(|table| table.pagination.as_ref())
                        .map(|p| p.default_page_size)
                })
                .unwrap_or(50),
        );
        let offset = block_request
            .and_then(|request| request.page.as_ref().map(|page| page.offset))
            .unwrap_or(0)
            .max(0);
        let sort = block_request
            .map(|request| request.sort.clone())
            .filter(|sort| !sort.is_empty())
            .or_else(|| {
                table
                    .map(|table| table.default_sort.clone())
                    .filter(|sort| !sort.is_empty())
            })
            .unwrap_or_else(|| block.source.order_by.clone());

        match entity {
            ReportWorkflowRuntimeEntity::Instances => {
                let engine = self.require_execution_engine()?;
                let runtime_client = self.require_runtime_client()?;
                let workflow_id = workflow_runtime_workflow_id(block)?;
                let page = (offset / page_size) as i32;
                let result = engine
                    .list_executions(tenant_id, workflow_id, Some(page), Some(page_size as i32))
                    .await
                    .map_err(map_execution_error_to_report)?;

                let mut rows = Vec::with_capacity(result.content.len());
                for instance in result.content {
                    let actions = if should_check_instance_actions(&instance) {
                        list_instance_actions(runtime_client, workflow_id, &instance.id)
                            .await
                            .map_err(map_workflow_runtime_error_to_report)?
                    } else {
                        Vec::new()
                    };
                    rows.push(workflow_instance_report_row(&instance, &actions));
                }

                rows = apply_workflow_runtime_row_filters(
                    rows,
                    definition,
                    block,
                    resolved_filters,
                    block_request,
                )?;
                sort_rows(&mut rows, &sort);

                Ok(json!({
                    "columns": workflow_runtime_table_columns(table, entity),
                    "rows": rows.into_iter().map(Value::Object).collect::<Vec<_>>(),
                    "page": {
                        "offset": offset,
                        "size": page_size,
                        "totalCount": result.total_elements,
                        "hasNextPage": !result.last,
                    }
                }))
            }
            ReportWorkflowRuntimeEntity::Actions => {
                let actions = self
                    .workflow_runtime_actions_for_block_context(
                        tenant_id,
                        definition,
                        block,
                        resolved_filters,
                        block_request,
                    )
                    .await?;
                let mut rows = actions
                    .into_iter()
                    .map(|action| workflow_action_report_row(&action))
                    .collect::<Vec<_>>();
                sort_rows(&mut rows, &sort);
                let total_count = rows.len() as i64;
                let rows = rows
                    .into_iter()
                    .skip(offset as usize)
                    .take(page_size as usize)
                    .map(Value::Object)
                    .collect::<Vec<_>>();

                Ok(json!({
                    "columns": workflow_runtime_table_columns(table, entity),
                    "rows": rows,
                    "page": {
                        "offset": offset,
                        "size": page_size,
                        "totalCount": total_count,
                        "hasNextPage": offset + page_size < total_count,
                    }
                }))
            }
            _ => Err(ReportServiceError::Validation(format!(
                "Block '{}' workflow_runtime source does not support system entity {:?}",
                block.id, entity
            ))),
        }
    }

    async fn render_system_table_block(
        &self,
        tenant_id: &str,
        definition: &ReportDefinition,
        block: &ReportBlockDefinition,
        resolved_filters: &HashMap<String, Value>,
        block_request: Option<&ReportBlockDataRequest>,
    ) -> Result<Value, ReportServiceError> {
        if block.source.mode == ReportSourceMode::Aggregate {
            return self
                .render_system_aggregate_table_block(
                    tenant_id,
                    definition,
                    block,
                    resolved_filters,
                    block_request,
                )
                .await;
        }

        let entity = system_entity(block)?;
        let table = block.table.as_ref();
        let page_size = clamp_page_size(
            block_request
                .and_then(|request| request.page.as_ref().map(|page| page.size))
                .or_else(|| {
                    table
                        .and_then(|table| table.pagination.as_ref())
                        .map(|p| p.default_page_size)
                })
                .unwrap_or(50),
        );
        let offset = block_request
            .and_then(|request| request.page.as_ref().map(|page| page.offset))
            .unwrap_or(0)
            .max(0);
        let sort = block_request
            .map(|request| request.sort.clone())
            .filter(|sort| !sort.is_empty())
            .or_else(|| {
                table
                    .map(|table| table.default_sort.clone())
                    .filter(|sort| !sort.is_empty())
            })
            .unwrap_or_else(|| block.source.order_by.clone());

        let mut rows = self
            .system_rows_for_block(
                tenant_id,
                definition,
                block,
                resolved_filters,
                block_request,
            )
            .await?;
        sort_rows(&mut rows, &sort);
        let total_count = rows.len() as i64;
        let rows = rows
            .into_iter()
            .skip(offset as usize)
            .take(page_size as usize)
            .map(Value::Object)
            .collect::<Vec<_>>();

        Ok(json!({
            "columns": system_table_columns(table, entity),
            "rows": rows,
            "page": {
                "offset": offset,
                "size": page_size,
                "totalCount": total_count,
                "hasNextPage": offset + page_size < total_count,
            }
        }))
    }

    async fn render_system_aggregate_table_block(
        &self,
        tenant_id: &str,
        definition: &ReportDefinition,
        block: &ReportBlockDefinition,
        resolved_filters: &HashMap<String, Value>,
        block_request: Option<&ReportBlockDataRequest>,
    ) -> Result<Value, ReportServiceError> {
        let table = block.table.as_ref();
        let page_size = clamp_page_size(
            block_request
                .and_then(|request| request.page.as_ref().map(|page| page.size))
                .or_else(|| {
                    table
                        .and_then(|table| table.pagination.as_ref())
                        .map(|p| p.default_page_size)
                })
                .unwrap_or(50),
        );
        let offset = block_request
            .and_then(|request| request.page.as_ref().map(|page| page.offset))
            .unwrap_or(0)
            .max(0);
        let sort = block_request
            .map(|request| request.sort.clone())
            .filter(|sort| !sort.is_empty())
            .or_else(|| {
                table
                    .map(|table| table.default_sort.clone())
                    .filter(|sort| !sort.is_empty())
            })
            .unwrap_or_else(|| block.source.order_by.clone());

        let request = build_table_aggregate_request(
            definition,
            block,
            resolved_filters,
            block_request,
            &sort,
            page_size,
            offset,
        )?;
        let rows = self
            .system_rows_for_block(
                tenant_id,
                definition,
                block,
                resolved_filters,
                block_request,
            )
            .await?;
        let result = aggregate_virtual_rows(&block.id, &rows, request)?;
        let source_columns = result.columns.clone();
        let columns = table_output_columns(table, &source_columns);
        let rows = project_aggregate_table_rows(table, &source_columns, result.rows)?;

        Ok(json!({
            "columns": columns,
            "rows": rows,
            "page": {
                "offset": offset,
                "size": page_size,
                "totalCount": result.group_count,
                "hasNextPage": offset + page_size < result.group_count,
            }
        }))
    }

    async fn render_system_aggregate_block(
        &self,
        tenant_id: &str,
        definition: &ReportDefinition,
        block: &ReportBlockDefinition,
        resolved_filters: &HashMap<String, Value>,
    ) -> Result<Value, ReportServiceError> {
        let request = build_aggregate_request(definition, block, resolved_filters)?;
        let rows = self
            .system_rows_for_block(tenant_id, definition, block, resolved_filters, None)
            .await?;
        let result = aggregate_virtual_rows(&block.id, &rows, request)?;

        Ok(json!({
            "columns": result.columns,
            "rows": result.rows,
            "groupCount": result.group_count,
        }))
    }

    async fn system_rows_for_block(
        &self,
        tenant_id: &str,
        definition: &ReportDefinition,
        block: &ReportBlockDefinition,
        resolved_filters: &HashMap<String, Value>,
        block_request: Option<&ReportBlockDataRequest>,
    ) -> Result<Vec<serde_json::Map<String, Value>>, ReportServiceError> {
        let condition = build_block_condition(definition, block, resolved_filters, block_request)?;
        let mut rows = self
            .fetch_system_rows(tenant_id, block, condition.as_ref())
            .await?;

        if let Some(condition) = &condition {
            rows = rows
                .into_iter()
                .filter_map(
                    |row| match condition_matches_row(condition, &row, &block.id) {
                        Ok(true) => Some(Ok(row)),
                        Ok(false) => None,
                        Err(error) => Some(Err(error)),
                    },
                )
                .collect::<Result<Vec<_>, _>>()?;
        }

        Ok(rows)
    }

    async fn fetch_system_rows(
        &self,
        tenant_id: &str,
        block: &ReportBlockDefinition,
        condition: Option<&Condition>,
    ) -> Result<Vec<serde_json::Map<String, Value>>, ReportServiceError> {
        match system_entity(block)? {
            ReportWorkflowRuntimeEntity::RuntimeExecutionMetricBuckets => {
                self.runtime_execution_metric_rows(tenant_id, block, condition)
                    .await
            }
            ReportWorkflowRuntimeEntity::RuntimeSystemSnapshot => {
                Ok(vec![runtime_system_snapshot_row()])
            }
            ReportWorkflowRuntimeEntity::ConnectionRateLimitStatus => {
                self.connection_rate_limit_status_rows(tenant_id, block)
                    .await
            }
            ReportWorkflowRuntimeEntity::ConnectionRateLimitEvents => {
                self.connection_rate_limit_event_rows(tenant_id, block, condition)
                    .await
            }
            ReportWorkflowRuntimeEntity::ConnectionRateLimitTimeline => {
                self.connection_rate_limit_timeline_rows(tenant_id, block, condition)
                    .await
            }
            ReportWorkflowRuntimeEntity::Instances | ReportWorkflowRuntimeEntity::Actions => {
                Err(ReportServiceError::Validation(format!(
                    "Block '{}' system source does not support workflow_runtime entity {:?}",
                    block.id, block.source.entity
                )))
            }
        }
    }

    async fn runtime_execution_metric_rows(
        &self,
        tenant_id: &str,
        block: &ReportBlockDefinition,
        condition: Option<&Condition>,
    ) -> Result<Vec<serde_json::Map<String, Value>>, ReportServiceError> {
        let runtime_client = self.require_runtime_client()?;
        let now = Utc::now();
        let (start_time, end_time) = extract_time_bounds(condition, &["bucketTime"]);
        let end_time = end_time.unwrap_or(now);
        let start_time = start_time.unwrap_or(end_time - Duration::days(30));
        let granularity = parse_metrics_granularity(block.source.granularity.as_deref())?;

        let result = runtime_client
            .get_tenant_metrics(
                GetTenantMetricsOptions::new(tenant_id)
                    .with_start_time(start_time)
                    .with_end_time(end_time)
                    .with_granularity(granularity),
            )
            .await
            .map_err(|err| {
                ReportServiceError::Database(format!(
                    "Failed to fetch runtime execution metrics: {}",
                    err
                ))
            })?;

        let result_tenant_id = result.tenant_id.clone();
        let result_granularity = format!("{:?}", result.granularity).to_lowercase();

        Ok(result
            .buckets
            .into_iter()
            .map(|bucket| {
                serde_json::Map::from_iter([
                    (
                        "tenantId".to_string(),
                        Value::String(result_tenant_id.clone()),
                    ),
                    (
                        "bucketTime".to_string(),
                        Value::String(bucket.bucket_time.to_rfc3339()),
                    ),
                    (
                        "granularity".to_string(),
                        Value::String(result_granularity.clone()),
                    ),
                    (
                        "invocationCount".to_string(),
                        json!(bucket.invocation_count),
                    ),
                    ("successCount".to_string(), json!(bucket.success_count)),
                    ("failureCount".to_string(), json!(bucket.failure_count)),
                    ("cancelledCount".to_string(), json!(bucket.cancelled_count)),
                    (
                        "avgDurationSeconds".to_string(),
                        option_f64_value(bucket.avg_duration_seconds),
                    ),
                    (
                        "minDurationSeconds".to_string(),
                        option_f64_value(bucket.min_duration_seconds),
                    ),
                    (
                        "maxDurationSeconds".to_string(),
                        option_f64_value(bucket.max_duration_seconds),
                    ),
                    (
                        "avgMemoryBytes".to_string(),
                        bucket
                            .avg_memory_bytes
                            .map(Value::from)
                            .unwrap_or(Value::Null),
                    ),
                    (
                        "maxMemoryBytes".to_string(),
                        bucket
                            .max_memory_bytes
                            .map(Value::from)
                            .unwrap_or(Value::Null),
                    ),
                    (
                        "successRatePercent".to_string(),
                        option_f64_value(bucket.success_rate_percent),
                    ),
                ])
            })
            .collect())
    }

    async fn connection_rate_limit_status_rows(
        &self,
        tenant_id: &str,
        block: &ReportBlockDefinition,
    ) -> Result<Vec<serde_json::Map<String, Value>>, ReportServiceError> {
        let interval = block.source.interval.as_deref().unwrap_or("24h");
        let service = self.connections.rate_limit_service();
        let statuses = service
            .list_all_rate_limits(tenant_id, Some(interval))
            .await
            .map_err(map_rate_limit_error)?;

        Ok(statuses.into_iter().map(rate_limit_status_row).collect())
    }

    async fn connection_rate_limit_event_rows(
        &self,
        tenant_id: &str,
        block: &ReportBlockDefinition,
        condition: Option<&Condition>,
    ) -> Result<Vec<serde_json::Map<String, Value>>, ReportServiceError> {
        let service = self.connections.rate_limit_service();
        let connection_id = system_connection_id(block, condition);
        let (from, to) = extract_time_bounds(condition, &["createdAt"]);
        let from = from.or_else(|| Some(Utc::now() - Duration::days(30)));
        let event_type = extract_eq_string_condition(condition, "eventType");
        let limit = block.source.limit.unwrap_or(1000).clamp(1, 1000);

        let mut events = Vec::new();
        if let Some(connection_id) = connection_id {
            let response = service
                .get_rate_limit_history(
                    &connection_id,
                    tenant_id,
                    &runtara_connections::types::RateLimitHistoryQuery {
                        limit,
                        offset: 0,
                        event_type,
                        from,
                        to,
                    },
                )
                .await
                .map_err(map_rate_limit_error)?;
            events.extend(response.data);
        } else {
            let statuses = service
                .list_all_rate_limits(tenant_id, Some("24h"))
                .await
                .map_err(map_rate_limit_error)?;
            for status in statuses {
                let response = service
                    .get_rate_limit_history(
                        &status.connection_id,
                        tenant_id,
                        &runtara_connections::types::RateLimitHistoryQuery {
                            limit,
                            offset: 0,
                            event_type: event_type.clone(),
                            from,
                            to,
                        },
                    )
                    .await
                    .map_err(map_rate_limit_error)?;
                events.extend(response.data);
            }
        }

        events.sort_by(|left, right| right.created_at.cmp(&left.created_at));
        events.truncate(limit as usize);
        Ok(events.into_iter().map(rate_limit_event_row).collect())
    }

    async fn connection_rate_limit_timeline_rows(
        &self,
        tenant_id: &str,
        block: &ReportBlockDefinition,
        condition: Option<&Condition>,
    ) -> Result<Vec<serde_json::Map<String, Value>>, ReportServiceError> {
        let Some(connection_id) = system_connection_id(block, condition) else {
            return Ok(Vec::new());
        };

        let now = Utc::now();
        let (start_time, end_time) =
            extract_time_bounds(condition, &["bucket", "bucketTime", "createdAt"]);
        let end_time = end_time.unwrap_or(now);
        let start_time = start_time.unwrap_or(end_time - Duration::hours(24));
        let granularity = block
            .source
            .granularity
            .clone()
            .unwrap_or_else(|| infer_rate_limit_timeline_granularity(start_time, end_time));
        let tag = extract_eq_string_condition(condition, "tag");

        let service = self.connections.rate_limit_service();
        let response = service
            .get_rate_limit_timeline(
                &connection_id,
                tenant_id,
                &runtara_connections::types::RateLimitTimelineQuery {
                    start_time: Some(start_time),
                    end_time: Some(end_time),
                    granularity: granularity.clone(),
                    tag,
                },
            )
            .await
            .map_err(map_rate_limit_error)?;

        Ok(response
            .data
            .buckets
            .into_iter()
            .map(|bucket| {
                serde_json::Map::from_iter([
                    (
                        "connectionId".to_string(),
                        Value::String(connection_id.clone()),
                    ),
                    (
                        "bucket".to_string(),
                        Value::String(bucket.bucket.to_rfc3339()),
                    ),
                    (
                        "bucketTime".to_string(),
                        Value::String(bucket.bucket.to_rfc3339()),
                    ),
                    (
                        "granularity".to_string(),
                        Value::String(granularity.clone()),
                    ),
                    ("requestCount".to_string(), json!(bucket.request_count)),
                    (
                        "rateLimitedCount".to_string(),
                        json!(bucket.rate_limited_count),
                    ),
                    ("retryCount".to_string(), json!(bucket.retry_count)),
                ])
            })
            .collect())
    }

    async fn render_actions_block(
        &self,
        tenant_id: &str,
        definition: &ReportDefinition,
        block: &ReportBlockDefinition,
        resolved_filters: &HashMap<String, Value>,
        block_request: Option<&ReportBlockDataRequest>,
    ) -> Result<Value, ReportServiceError> {
        let entity = workflow_runtime_entity(block)?;
        if entity != ReportWorkflowRuntimeEntity::Actions {
            return Err(ReportServiceError::Validation(format!(
                "Block '{}' action renderer requires workflow_runtime entity 'actions'",
                block.id
            )));
        }

        let rows = self
            .workflow_runtime_actions_for_block_context(
                tenant_id,
                definition,
                block,
                resolved_filters,
                block_request,
            )
            .await?
            .into_iter()
            .map(|action| workflow_action_report_row(&action))
            .collect::<Vec<_>>();

        let actions = rows.iter().cloned().map(Value::Object).collect::<Vec<_>>();

        Ok(json!({
            "actions": actions,
            "rows": actions,
            "page": {
                "offset": 0,
                "size": rows.len() as i64,
                "totalCount": rows.len() as i64,
                "hasNextPage": false,
            }
        }))
    }

    async fn render_markdown_block(
        &self,
        tenant_id: &str,
        definition: &ReportDefinition,
        block: &ReportBlockDefinition,
        resolved_filters: &HashMap<String, Value>,
        block_request: Option<&ReportBlockDataRequest>,
    ) -> Result<Value, ReportServiceError> {
        let markdown = block.markdown.as_ref().ok_or_else(|| {
            ReportServiceError::Validation(format!(
                "Block '{}' markdown block must define markdown.content",
                block.id
            ))
        })?;
        let mut data = serde_json::Map::new();
        data.insert(
            "content".to_string(),
            Value::String(markdown.content.clone()),
        );
        if block.dataset.is_some() || !block.source.is_empty() {
            data.insert(
                "source".to_string(),
                self.render_table_block(
                    tenant_id,
                    definition,
                    block,
                    resolved_filters,
                    block_request,
                )
                .await?,
            );
        }
        Ok(Value::Object(data))
    }

    async fn workflow_runtime_actions_for_block_context(
        &self,
        tenant_id: &str,
        definition: &ReportDefinition,
        block: &ReportBlockDefinition,
        resolved_filters: &HashMap<String, Value>,
        block_request: Option<&ReportBlockDataRequest>,
    ) -> Result<Vec<WorkflowRuntimeAction>, ReportServiceError> {
        let actions = self
            .workflow_runtime_actions_for_source(tenant_id, block, 0, 100)
            .await?;
        let Some(condition) =
            build_block_condition(definition, block, resolved_filters, block_request)?
        else {
            return Ok(actions);
        };

        actions
            .into_iter()
            .filter_map(|action| {
                let row = workflow_action_report_row(&action);
                match condition_matches_row(&condition, &row, &block.id) {
                    Ok(true) => Some(Ok(action)),
                    Ok(false) => None,
                    Err(error) => Some(Err(error)),
                }
            })
            .collect()
    }

    async fn workflow_runtime_actions_for_source(
        &self,
        tenant_id: &str,
        block: &ReportBlockDefinition,
        _offset: i64,
        _page_size: i64,
    ) -> Result<Vec<WorkflowRuntimeAction>, ReportServiceError> {
        let workflow_id = workflow_runtime_workflow_id(block)?;
        let engine = self.require_execution_engine()?;
        let runtime_client = self.require_runtime_client()?;

        if let Some(instance_id) = block
            .source
            .instance_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            let execution = engine
                .get_execution_with_metadata(workflow_id, instance_id, tenant_id)
                .await
                .map_err(map_execution_error_to_report)?;
            if !should_check_instance_actions(&execution.instance) {
                return Ok(Vec::new());
            }
            return list_instance_actions(runtime_client, workflow_id, instance_id)
                .await
                .map_err(map_workflow_runtime_error_to_report);
        }

        let actions = list_workflow_actions(
            engine,
            runtime_client,
            tenant_id,
            workflow_id,
            Some(0),
            Some(100),
        )
        .await
        .map_err(map_workflow_runtime_error_to_report)?
        .actions;

        Ok(actions)
    }

    fn require_execution_engine(&self) -> Result<&ExecutionEngine, ReportServiceError> {
        self.engine.as_deref().ok_or_else(|| {
            ReportServiceError::Validation(
                "Workflow runtime report sources require the execution engine".to_string(),
            )
        })
    }

    fn require_runtime_client(&self) -> Result<&RuntimeClient, ReportServiceError> {
        self.runtime_client.as_deref().ok_or_else(|| {
            ReportServiceError::Validation(
                "Workflow runtime report sources require a configured runtime client".to_string(),
            )
        })
    }

    async fn render_joined_filter_table_block(
        &self,
        tenant_id: &str,
        definition: &ReportDefinition,
        block: &ReportBlockDefinition,
        resolved_filters: &HashMap<String, Value>,
        block_request: Option<&ReportBlockDataRequest>,
    ) -> Result<Value, ReportServiceError> {
        let table = block.table.as_ref();
        let page_size = clamp_page_size(
            block_request
                .and_then(|request| request.page.as_ref().map(|page| page.size))
                .or_else(|| {
                    table
                        .and_then(|table| table.pagination.as_ref())
                        .map(|p| p.default_page_size)
                })
                .unwrap_or(50),
        );
        let offset = block_request
            .and_then(|request| request.page.as_ref().map(|page| page.offset))
            .unwrap_or(0)
            .max(0);
        let sort = block_request
            .map(|request| request.sort.clone())
            .filter(|sort| !sort.is_empty())
            .or_else(|| {
                table
                    .map(|table| table.default_sort.clone())
                    .filter(|sort| !sort.is_empty())
            })
            .unwrap_or_else(|| block.source.order_by.clone());

        let condition = build_block_condition(definition, block, resolved_filters, block_request)?;
        let alias_to_join = build_alias_index(&block.source.join, &block.id)?;
        let alias_set: HashSet<&str> = alias_to_join.keys().map(|alias| alias.as_str()).collect();
        let primary_condition =
            primary_pushdown_condition(condition.clone(), &alias_set, &block.id)?;

        let filter_request = FilterRequest {
            offset: 0,
            limit: MAX_JOIN_POST_FILTER_ROWS,
            condition: primary_condition,
            sort_by: None,
            sort_order: None,
            score_expression: None,
            order_by: None,
        };

        let (instances, total_candidates) = self
            .instance_service
            .filter_instances_by_schema(
                tenant_id,
                &block.source.schema,
                filter_request,
                block.source.connection_id.as_deref(),
            )
            .await
            .map_err(map_object_model_error)?;

        if total_candidates > MAX_JOIN_POST_FILTER_ROWS {
            return Err(ReportServiceError::Validation(format!(
                "Block '{}' joined table query matched {} primary rows before post-filtering; cap is {}. Add a more selective primary-schema condition.",
                block.id, total_candidates, MAX_JOIN_POST_FILTER_ROWS
            )));
        }

        let mut rows = instances
            .into_iter()
            .filter_map(|instance| match flatten_instance(instance) {
                Value::Object(row) => Some(row),
                _ => None,
            })
            .collect::<Vec<_>>();
        let join_data = self
            .resolve_filter_join_data(tenant_id, block, &alias_to_join, &rows)
            .await?;
        rows = enrich_filter_join_rows(&alias_to_join, &join_data, rows);

        if let Some(condition) = &condition {
            rows = rows
                .into_iter()
                .filter_map(
                    |row| match condition_matches_row(condition, &row, &block.id) {
                        Ok(true) => Some(Ok(row)),
                        Ok(false) => None,
                        Err(err) => Some(Err(err)),
                    },
                )
                .collect::<Result<Vec<_>, _>>()?;
        }

        sort_rows(&mut rows, &sort);
        let total_count = rows.len() as i64;
        let rows = rows
            .into_iter()
            .skip(offset as usize)
            .take(page_size as usize)
            .map(Value::Object)
            .collect::<Vec<_>>();
        let condition_context = ReportConditionRuntimeContext {
            definition,
            block,
            resolved_filters,
            block_request,
        };
        let rows = self
            .hydrate_table_chart_columns(tenant_id, condition_context, table, rows)
            .await?;

        let columns = table_response_columns(table);

        Ok(json!({
            "columns": columns,
            "rows": rows,
            "page": {
                "offset": offset,
                "size": page_size,
                "totalCount": total_count,
                "hasNextPage": offset + page_size < total_count
            },
            "diagnostics": [{
                "severity": "warning",
                "code": "JOIN_BROADCAST_POST_FILTER",
                "message": format!("Block '{}' used bounded broadcast join post-filtering.", block.id)
            }]
        }))
    }

    async fn render_aggregate_table_block(
        &self,
        tenant_id: &str,
        definition: &ReportDefinition,
        block: &ReportBlockDefinition,
        resolved_filters: &HashMap<String, Value>,
        block_request: Option<&ReportBlockDataRequest>,
    ) -> Result<Value, ReportServiceError> {
        let table = block.table.as_ref();
        let page_size = clamp_page_size(
            block_request
                .and_then(|request| request.page.as_ref().map(|page| page.size))
                .or_else(|| {
                    table
                        .and_then(|table| table.pagination.as_ref())
                        .map(|p| p.default_page_size)
                })
                .unwrap_or(50),
        );
        let offset = block_request
            .and_then(|request| request.page.as_ref().map(|page| page.offset))
            .unwrap_or(0)
            .max(0);
        let sort = block_request
            .map(|request| request.sort.clone())
            .filter(|sort| !sort.is_empty())
            .or_else(|| {
                table
                    .map(|table| table.default_sort.clone())
                    .filter(|sort| !sort.is_empty())
            })
            .unwrap_or_else(|| block.source.order_by.clone());

        let request = build_table_aggregate_request(
            definition,
            block,
            resolved_filters,
            block_request,
            &sort,
            page_size,
            offset,
        )?;
        let result = self
            .aggregate_with_optional_joins(tenant_id, block, request)
            .await?;

        let source_columns = result.columns.clone();
        let condition_context = ReportConditionRuntimeContext {
            definition,
            block,
            resolved_filters,
            block_request,
        };
        let mut row_maps = aggregate_rows_to_maps(&source_columns, &result.rows);
        let columns = table_output_columns(table, &source_columns);
        let mut rows = project_aggregate_table_rows(table, &source_columns, result.rows)?;

        if let Some(table) = table {
            let chart_columns: Vec<_> = table
                .columns
                .iter()
                .enumerate()
                .filter(|(_, column)| column.is_chart())
                .collect();
            if !chart_columns.is_empty() {
                for (row_index, row) in rows.iter_mut().enumerate() {
                    let Some(row_map) = row_maps.get_mut(row_index) else {
                        continue;
                    };
                    for (column_index, chart_column) in &chart_columns {
                        let cell = self
                            .render_table_chart_cell(
                                tenant_id,
                                condition_context,
                                chart_column,
                                row_map,
                            )
                            .await?;
                        row_map.insert(chart_column.field.clone(), cell.clone());
                        if let Some(slot) = row.get_mut(*column_index) {
                            *slot = cell;
                        }
                    }
                }
            }
        }

        Ok(json!({
            "columns": columns,
            "rows": rows,
            "page": {
                "offset": offset,
                "size": page_size,
                "totalCount": result.group_count,
                "hasNextPage": offset + page_size < result.group_count
            }
        }))
    }

    async fn hydrate_table_chart_columns(
        &self,
        tenant_id: &str,
        condition_context: ReportConditionRuntimeContext<'_>,
        table: Option<&ReportTableConfig>,
        rows: Vec<Value>,
    ) -> Result<Vec<Value>, ReportServiceError> {
        let Some(table) = table else {
            return Ok(rows);
        };
        let chart_columns: Vec<_> = table
            .columns
            .iter()
            .filter(|column| column.is_chart())
            .collect();
        let value_columns: Vec<_> = table
            .columns
            .iter()
            .filter(|column| column.is_value_lookup())
            .collect();
        let lookup_display_columns: Vec<_> = table
            .columns
            .iter()
            .filter(|column| auto_lookup_display_field(column).is_some())
            .collect();
        if chart_columns.is_empty() && value_columns.is_empty() && lookup_display_columns.is_empty()
        {
            return Ok(rows);
        }

        let mut hydrated_rows = rows
            .into_iter()
            .filter_map(|row| match row {
                Value::Object(object) => Some(object),
                _ => None,
            })
            .collect::<Vec<_>>();

        if !value_columns.is_empty() {
            self.hydrate_table_value_columns(
                tenant_id,
                condition_context,
                &value_columns,
                &mut hydrated_rows,
            )
            .await?;
        }
        if !lookup_display_columns.is_empty() {
            self.hydrate_table_lookup_display_columns(
                tenant_id,
                condition_context,
                &lookup_display_columns,
                &mut hydrated_rows,
            )
            .await?;
        }

        let mut output_rows = Vec::with_capacity(hydrated_rows.len());
        for mut object in hydrated_rows {
            for column in &chart_columns {
                let cell = self
                    .render_table_chart_cell(tenant_id, condition_context, column, &object)
                    .await?;
                object.insert(column.field.clone(), cell);
            }
            output_rows.push(Value::Object(object));
        }

        Ok(output_rows)
    }

    // This hydrator can remove rows for inner joins, so it needs Vec::retain.
    #[allow(clippy::ptr_arg)]
    async fn hydrate_table_value_columns(
        &self,
        tenant_id: &str,
        condition_context: ReportConditionRuntimeContext<'_>,
        value_columns: &[&ReportTableColumn],
        rows: &mut Vec<serde_json::Map<String, Value>>,
    ) -> Result<(), ReportServiceError> {
        let condition_filter_defs = block_condition_filter_definitions(
            condition_context.definition,
            condition_context.block,
        );
        let condition_filter_values = block_condition_filter_values(
            condition_context.block,
            condition_context.resolved_filters,
            condition_context.block_request,
        );
        let mut keep_rows = vec![true; rows.len()];
        for column in value_columns {
            let Some(source) = &column.source else {
                continue;
            };
            let select = source.select.as_deref().ok_or_else(|| {
                ReportServiceError::Validation(format!(
                    "Value table column '{}' requires source.select",
                    column.field
                ))
            })?;
            let join = source.join.first().ok_or_else(|| {
                ReportServiceError::Validation(format!(
                    "Value table column '{}' requires source.join",
                    column.field
                ))
            })?;
            let mut seen_keys = HashSet::new();
            let mut parent_keys = Vec::new();
            for row in rows.iter() {
                let Some(value) = row.get(&join.parent_field) else {
                    continue;
                };
                if value_is_empty(value) {
                    continue;
                }
                if seen_keys.insert(value_to_lookup_key(value)) {
                    parent_keys.push(value.clone());
                }
            }

            let source_condition = resolve_optional_report_condition(
                source.condition.as_ref(),
                &condition_filter_defs,
                &condition_filter_values,
                &format!(
                    "block '{}' value table column '{}'",
                    condition_context.block.id, column.field
                ),
            )?;
            let values_by_key = if parent_keys.is_empty() {
                HashMap::new()
            } else {
                self.lookup_table_value_column(
                    tenant_id,
                    source,
                    join,
                    select,
                    source_condition,
                    &parent_keys,
                )
                .await?
            };

            for (index, row) in rows.iter_mut().enumerate() {
                let value = row
                    .get(&join.parent_field)
                    .and_then(|parent_value| values_by_key.get(&value_to_lookup_key(parent_value)))
                    .cloned()
                    .unwrap_or(Value::Null);
                if value_is_empty(&value) && matches!(join.kind, ReportJoinKind::Inner) {
                    keep_rows[index] = false;
                }
                row.insert(column.field.clone(), value);
            }
        }

        let mut index = 0;
        rows.retain(|_| {
            let keep = keep_rows[index];
            index += 1;
            keep
        });
        Ok(())
    }

    async fn hydrate_table_lookup_display_columns(
        &self,
        tenant_id: &str,
        condition_context: ReportConditionRuntimeContext<'_>,
        lookup_columns: &[&ReportTableColumn],
        rows: &mut [serde_json::Map<String, Value>],
    ) -> Result<(), ReportServiceError> {
        let condition_filter_defs = block_condition_filter_definitions(
            condition_context.definition,
            condition_context.block,
        );
        let condition_filter_values = block_condition_filter_values(
            condition_context.block,
            condition_context.resolved_filters,
            condition_context.block_request,
        );

        for column in lookup_columns {
            let Some(lookup) = lookup_editor_for_table_column(column) else {
                continue;
            };
            let Some(display_field) = auto_lookup_display_field(column) else {
                continue;
            };
            let mut seen_keys = HashSet::new();
            let mut parent_keys = Vec::new();
            for row in rows.iter() {
                let Some(value) = row.get(&column.field) else {
                    continue;
                };
                if value_is_empty(value) {
                    continue;
                }
                if seen_keys.insert(value_to_lookup_key(value)) {
                    parent_keys.push(value.clone());
                }
            }
            if parent_keys.is_empty() {
                continue;
            }

            let mut conditions = Vec::new();
            if let Some(condition) = resolve_optional_report_condition(
                lookup.condition.as_ref(),
                &condition_filter_defs,
                &condition_filter_values,
                &format!(
                    "block '{}' table column '{}' lookup",
                    condition_context.block.id, column.field
                ),
            )? {
                conditions.push(condition);
            }
            append_source_mapping_conditions(
                &mut conditions,
                &lookup.filter_mappings,
                &condition_filter_values,
            );
            conditions.push(Condition {
                op: "IN".to_string(),
                arguments: Some(vec![
                    Value::String(lookup.value_field.clone()),
                    Value::Array(parent_keys),
                ]),
            });

            let labels_by_key = if lookup.value_field == "id" {
                self.lookup_display_labels_by_filter(
                    tenant_id,
                    &lookup.schema,
                    lookup.connection_id.as_deref(),
                    &lookup.value_field,
                    &lookup.label_field,
                    conditions,
                )
                .await?
            } else {
                let query = ObjectModelOptionQuery {
                    context: format!(
                        "block '{}' table column '{}' lookup",
                        condition_context.block.id, column.field
                    ),
                    schema: lookup.schema.clone(),
                    connection_id: lookup.connection_id.as_deref(),
                    value_field: lookup.value_field.clone(),
                    label_field: lookup.label_field.clone(),
                    conditions,
                    search_fields: vec![],
                    search_query: String::new(),
                };
                let (options, page) = self
                    .query_object_model_options(tenant_id, query, 0, MAX_BROADCAST_JOIN_DIM_ROWS)
                    .await?;
                if page.total_count > MAX_BROADCAST_JOIN_DIM_ROWS {
                    return Err(ReportServiceError::Validation(format!(
                        "Lookup display for table column '{}' on '{}' would broadcast {} rows; cap is {}. Add a more selective lookup.condition.",
                        column.field, lookup.schema, page.total_count, MAX_BROADCAST_JOIN_DIM_ROWS
                    )));
                }

                options
                    .into_iter()
                    .map(|option| {
                        (
                            value_to_lookup_key(&option.value),
                            Value::String(option.label),
                        )
                    })
                    .collect::<HashMap<_, _>>()
            };

            for row in rows.iter_mut() {
                let Some(value) = row.get(&column.field) else {
                    continue;
                };
                if let Some(label) = labels_by_key.get(&value_to_lookup_key(value)) {
                    row.insert(display_field.clone(), label.clone());
                }
            }
        }

        Ok(())
    }

    async fn lookup_display_labels_by_filter(
        &self,
        tenant_id: &str,
        schema: &str,
        connection_id: Option<&str>,
        value_field: &str,
        label_field: &str,
        conditions: Vec<Condition>,
    ) -> Result<HashMap<String, Value>, ReportServiceError> {
        let request = FilterRequest {
            offset: 0,
            limit: MAX_BROADCAST_JOIN_DIM_ROWS,
            condition: combine_conditions(conditions),
            sort_by: Some(vec![label_field.to_string(), "id".to_string()]),
            sort_order: Some(vec!["asc".to_string(), "asc".to_string()]),
            score_expression: None,
            order_by: None,
        };
        let (instances, total) = self
            .instance_service
            .filter_instances_by_schema(tenant_id, schema, request, connection_id)
            .await
            .map_err(map_object_model_error)?;

        if total > MAX_BROADCAST_JOIN_DIM_ROWS {
            return Err(ReportServiceError::Validation(format!(
                "Lookup display on '{}' would broadcast {} rows; cap is {}. Add a more selective lookup.condition.",
                schema, total, MAX_BROADCAST_JOIN_DIM_ROWS
            )));
        }

        Ok(lookup_display_labels_from_instances(
            instances,
            value_field,
            label_field,
        ))
    }

    async fn lookup_table_value_column(
        &self,
        tenant_id: &str,
        source: &ReportTableColumnSource,
        join: &ReportTableColumnJoin,
        select: &str,
        source_condition: Option<Condition>,
        parent_keys: &[Value],
    ) -> Result<HashMap<String, Value>, ReportServiceError> {
        let join_condition = Condition {
            op: "IN".to_string(),
            arguments: Some(vec![
                Value::String(join.field.clone()),
                Value::Array(parent_keys.to_vec()),
            ]),
        };
        let condition = combine_conditions(
            source_condition
                .into_iter()
                .chain(std::iter::once(join_condition))
                .collect(),
        );
        let sort_by = if source.order_by.is_empty() {
            Some(vec![join.field.clone(), "id".to_string()])
        } else {
            Some(
                source
                    .order_by
                    .iter()
                    .map(|order| order.field.clone())
                    .collect(),
            )
        };
        let sort_order = if source.order_by.is_empty() {
            Some(vec!["asc".to_string(), "asc".to_string()])
        } else {
            Some(
                source
                    .order_by
                    .iter()
                    .map(|order| normalize_sort_direction(&order.direction))
                    .collect(),
            )
        };
        let request = FilterRequest {
            offset: 0,
            limit: MAX_BROADCAST_JOIN_DIM_ROWS,
            condition,
            sort_by,
            sort_order,
            score_expression: None,
            order_by: None,
        };
        let (instances, total) = self
            .instance_service
            .filter_instances_by_schema(
                tenant_id,
                &source.schema,
                request,
                source.connection_id.as_deref(),
            )
            .await
            .map_err(map_object_model_error)?;

        if total > MAX_BROADCAST_JOIN_DIM_ROWS {
            return Err(ReportServiceError::Validation(format!(
                "Value table column lookup on '{}' would broadcast {} rows; cap is {}. Add a more selective source.condition.",
                source.schema, total, MAX_BROADCAST_JOIN_DIM_ROWS
            )));
        }

        let mut values_by_key = HashMap::new();
        for instance in instances {
            let Value::Object(row) = flatten_instance(instance) else {
                continue;
            };
            let Some(key) = row.get(&join.field) else {
                continue;
            };
            if value_is_empty(key) {
                continue;
            }
            let Some(value) = row.get(select) else {
                continue;
            };
            values_by_key
                .entry(value_to_lookup_key(key))
                .or_insert_with(|| value.clone());
        }

        Ok(values_by_key)
    }

    async fn render_table_chart_cell(
        &self,
        tenant_id: &str,
        condition_context: ReportConditionRuntimeContext<'_>,
        column: &ReportTableColumn,
        row: &serde_json::Map<String, Value>,
    ) -> Result<Value, ReportServiceError> {
        let Some(source) = &column.source else {
            return Ok(Value::Null);
        };
        let condition = build_table_column_condition(condition_context, source, row)?;
        let request = build_column_aggregate_request(source, condition)?;
        let result = self
            .instance_service
            .aggregate_instances_by_schema(
                tenant_id,
                &source.schema,
                request,
                source.connection_id.as_deref(),
            )
            .await
            .map_err(map_object_model_error)?;

        Ok(json!({
            "columns": result.columns,
            "rows": result.rows,
        }))
    }

    async fn render_aggregate_block(
        &self,
        tenant_id: &str,
        definition: &ReportDefinition,
        block: &ReportBlockDefinition,
        resolved_filters: &HashMap<String, Value>,
    ) -> Result<Value, ReportServiceError> {
        if block.source.kind == ReportSourceKind::System {
            return self
                .render_system_aggregate_block(tenant_id, definition, block, resolved_filters)
                .await;
        }
        let request = build_aggregate_request(definition, block, resolved_filters)?;
        let result = self
            .aggregate_with_optional_joins(tenant_id, block, request)
            .await?;

        Ok(json!({
            "columns": result.columns,
            "rows": result.rows,
            "groupCount": result.group_count,
        }))
    }

    /// Run an aggregate that may reference joined dimension schemas via
    /// `<alias>.<field>` qualified field names. Implements broadcast-hash
    /// join: each declared dimension is resolved client-side first (with any
    /// `<alias>.<field>` condition terms applied), the primary aggregate is
    /// filtered by the resolved parent-field keys, and result rows are
    /// enriched with the joined dimension columns.
    ///
    /// When `block.source.join` is empty this is a passthrough to the regular
    /// aggregate.
    async fn aggregate_with_optional_joins(
        &self,
        tenant_id: &str,
        block: &ReportBlockDefinition,
        request: AggregateRequest,
    ) -> Result<runtara_object_store::AggregateResult, ReportServiceError> {
        let joins = &block.source.join;
        if joins.is_empty() {
            return self
                .instance_service
                .aggregate_instances_by_schema(
                    tenant_id,
                    &block.source.schema,
                    request,
                    block.source.connection_id.as_deref(),
                )
                .await
                .map_err(map_object_model_error);
        }

        let alias_to_join = build_alias_index(joins, &block.id)?;
        validate_join_request(&request, &alias_to_join, &block.id)?;

        let alias_set: HashSet<&str> = alias_to_join.keys().map(|s| s.as_str()).collect();
        let (primary_condition, by_alias) =
            split_qualified_condition(request.condition.clone(), &alias_set, &block.id)?;

        let mut join_data: HashMap<String, JoinResolution> = HashMap::new();
        for (alias, join) in &alias_to_join {
            let alias_terms = by_alias.get(alias).cloned().unwrap_or_default();
            let resolution = self
                .resolve_join(tenant_id, join, &alias_terms, &request.group_by)
                .await?;
            join_data.insert(alias.clone(), resolution);
        }

        let mut primary_conditions: Vec<Condition> = Vec::new();
        if let Some(c) = primary_condition {
            primary_conditions.push(c);
        }
        let mut empty_inner_join = false;
        for (alias, join) in &alias_to_join {
            let data = &join_data[alias];
            if data.parent_keys.is_empty() {
                if matches!(join.kind, ReportJoinKind::Inner) {
                    empty_inner_join = true;
                    break;
                }
                continue;
            }
            primary_conditions.push(Condition {
                op: "IN".to_string(),
                arguments: Some(vec![
                    Value::String(join.parent_field.clone()),
                    Value::Array(data.parent_keys.clone()),
                ]),
            });
        }

        if empty_inner_join {
            return Ok(empty_join_result(&request.group_by, &request.aggregates));
        }

        let primary_group_by: Vec<String> = request
            .group_by
            .iter()
            .filter(|field| field_alias_prefix(field).is_none())
            .cloned()
            .collect();

        let primary_order_by: Vec<AggregateOrderBy> = request
            .order_by
            .iter()
            .filter(|order| field_alias_prefix(&order.column).is_none())
            .cloned()
            .collect();

        let primary_request = AggregateRequest {
            condition: combine_conditions(primary_conditions),
            group_by: primary_group_by,
            aggregates: request.aggregates.clone(),
            order_by: primary_order_by,
            limit: request.limit,
            offset: request.offset,
        };

        let primary_result = self
            .instance_service
            .aggregate_instances_by_schema(
                tenant_id,
                &block.source.schema,
                primary_request,
                block.source.connection_id.as_deref(),
            )
            .await
            .map_err(map_object_model_error)?;

        Ok(enrich_aggregate_result(
            primary_result,
            &request.group_by,
            &alias_to_join,
            &join_data,
        ))
    }

    async fn resolve_filter_join_data(
        &self,
        tenant_id: &str,
        block: &ReportBlockDefinition,
        alias_to_join: &HashMap<String, &ReportSourceJoin>,
        primary_rows: &[serde_json::Map<String, Value>],
    ) -> Result<HashMap<String, JoinResolution>, ReportServiceError> {
        let mut join_data = HashMap::new();
        for (alias, join) in alias_to_join {
            let mut seen_keys = HashSet::new();
            let mut parent_keys = Vec::new();
            for row in primary_rows {
                let Some(value) = row.get(&join.parent_field) else {
                    continue;
                };
                if value_is_empty(value) {
                    continue;
                }
                let key = value_to_lookup_key(value);
                if seen_keys.insert(key) {
                    parent_keys.push(value.clone());
                }
            }

            if parent_keys.is_empty() {
                join_data.insert(
                    alias.clone(),
                    JoinResolution {
                        parent_keys,
                        by_key: HashMap::new(),
                    },
                );
                continue;
            }

            let filter = FilterRequest {
                offset: 0,
                limit: MAX_BROADCAST_JOIN_DIM_ROWS,
                condition: Some(Condition {
                    op: "IN".to_string(),
                    arguments: Some(vec![
                        Value::String(join.field.clone()),
                        Value::Array(parent_keys.clone()),
                    ]),
                }),
                sort_by: None,
                sort_order: None,
                score_expression: None,
                order_by: None,
            };

            let (dim_instances, total) = self
                .instance_service
                .filter_instances_by_schema(
                    tenant_id,
                    &join.schema,
                    filter,
                    join.connection_id.as_deref(),
                )
                .await
                .map_err(map_object_model_error)?;

            if total > MAX_BROADCAST_JOIN_DIM_ROWS {
                return Err(ReportServiceError::Validation(format!(
                    "Block '{}' join '{}' would broadcast {} rows from '{}'; cap is {}. Add a more selective condition.",
                    block.id, alias, total, join.schema, MAX_BROADCAST_JOIN_DIM_ROWS
                )));
            }

            let by_key = dim_instances
                .into_iter()
                .filter_map(|instance| match flatten_instance(instance) {
                    Value::Object(row) => {
                        let key = row.get(&join.field).cloned()?;
                        Some((key, row))
                    }
                    _ => None,
                })
                .filter(|(key, _)| !value_is_empty(key))
                .fold(HashMap::new(), |mut by_key, (key, row)| {
                    by_key.entry(value_to_lookup_key(&key)).or_insert(row);
                    by_key
                });

            join_data.insert(
                alias.clone(),
                JoinResolution {
                    parent_keys,
                    by_key,
                },
            );
        }

        Ok(join_data)
    }

    /// Query the dimension schema and build a lookup keyed by the join's `field`.
    async fn resolve_join(
        &self,
        tenant_id: &str,
        join: &ReportSourceJoin,
        alias_terms: &[Condition],
        _group_by: &[String],
    ) -> Result<JoinResolution, ReportServiceError> {
        let alias = join.effective_alias();

        let stripped_conditions: Vec<Condition> = alias_terms
            .iter()
            .map(|c| strip_alias_from_condition(c.clone(), alias))
            .collect();

        let dim_condition = combine_conditions(stripped_conditions);

        let filter = FilterRequest {
            offset: 0,
            limit: MAX_BROADCAST_JOIN_DIM_ROWS,
            condition: dim_condition,
            sort_by: None,
            sort_order: None,
            score_expression: None,
            order_by: None,
        };

        let (dim_instances, total) = self
            .instance_service
            .filter_instances_by_schema(
                tenant_id,
                &join.schema,
                filter,
                join.connection_id.as_deref(),
            )
            .await
            .map_err(map_object_model_error)?;

        if total > MAX_BROADCAST_JOIN_DIM_ROWS {
            return Err(ReportServiceError::Validation(format!(
                "Join '{}' would broadcast {} rows from '{}'; cap is {}. Add a more \
                 selective condition on '{}' fields.",
                alias, total, join.schema, MAX_BROADCAST_JOIN_DIM_ROWS, alias
            )));
        }

        let dim_rows: Vec<serde_json::Map<String, Value>> = dim_instances
            .into_iter()
            .filter_map(|i| match flatten_instance(i) {
                Value::Object(map) => Some(map),
                _ => None,
            })
            .collect();

        let mut parent_keys: Vec<Value> = Vec::with_capacity(dim_rows.len());
        let mut seen_keys: HashSet<String> = HashSet::with_capacity(dim_rows.len());
        let mut by_key: HashMap<String, serde_json::Map<String, Value>> =
            HashMap::with_capacity(dim_rows.len());

        for row in dim_rows {
            let Some(key_value) = row.get(&join.field) else {
                continue;
            };
            if value_is_empty(key_value) {
                continue;
            }
            let key_str = value_to_lookup_key(key_value);
            if seen_keys.insert(key_str.clone()) {
                parent_keys.push(key_value.clone());
            }
            by_key.entry(key_str).or_insert(row);
        }

        Ok(JoinResolution {
            parent_keys,
            by_key,
        })
    }

    async fn render_metric_block(
        &self,
        tenant_id: &str,
        definition: &ReportDefinition,
        block: &ReportBlockDefinition,
        resolved_filters: &HashMap<String, Value>,
    ) -> Result<Value, ReportServiceError> {
        let aggregate_data = self
            .render_aggregate_block(tenant_id, definition, block, resolved_filters)
            .await?;
        let metric = block.metric.as_ref();
        let value_field = metric.map(|m| m.value_field.as_str()).unwrap_or("value");
        let columns = aggregate_data
            .get("columns")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let rows = aggregate_data
            .get("rows")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let value_index = columns
            .iter()
            .position(|column| column.as_str() == Some(value_field))
            .unwrap_or(0);
        let value = rows
            .first()
            .and_then(Value::as_array)
            .and_then(|row| row.get(value_index))
            .cloned()
            .unwrap_or(Value::Null);

        Ok(json!({
            "value": value,
            "valueField": value_field,
            "label": metric.and_then(|m| m.label.clone()),
            "format": metric.and_then(|m| m.format.clone()),
            "columns": columns,
            "rows": rows,
        }))
    }

    /// Card blocks render the first row of a single-row filter source as a
    /// vertical key→value layout. The shape returned mirrors a one-row table
    /// (`columns` + first-row `row`), with `missing: true` when the filter
    /// matches no rows so the frontend can show an empty-card placeholder.
    async fn render_card_block(
        &self,
        tenant_id: &str,
        definition: &ReportDefinition,
        block: &ReportBlockDefinition,
        resolved_filters: &HashMap<String, Value>,
    ) -> Result<Value, ReportServiceError> {
        if block.source.kind != ReportSourceKind::ObjectModel {
            return Err(ReportServiceError::Validation(format!(
                "Block '{}' card blocks only support object_model sources",
                block.id
            )));
        }
        if block.source.mode != ReportSourceMode::Filter {
            return Err(ReportServiceError::Validation(format!(
                "Block '{}' card blocks only support filter-mode sources",
                block.id
            )));
        }

        let filter_request = FilterRequest {
            offset: 0,
            limit: 1,
            condition: build_block_condition(definition, block, resolved_filters, None)?,
            sort_by: if block.source.order_by.is_empty() {
                None
            } else {
                Some(
                    block
                        .source
                        .order_by
                        .iter()
                        .map(|entry| entry.field.clone())
                        .collect(),
                )
            },
            sort_order: if block.source.order_by.is_empty() {
                None
            } else {
                Some(
                    block
                        .source
                        .order_by
                        .iter()
                        .map(|entry| normalize_sort_direction(&entry.direction))
                        .collect(),
                )
            },
            score_expression: None,
            order_by: None,
        };

        let (instances, _total_count) = self
            .instance_service
            .filter_instances_by_schema(
                tenant_id,
                &block.source.schema,
                filter_request,
                block.source.connection_id.as_deref(),
            )
            .await
            .map_err(map_object_model_error)?;

        let row = instances.into_iter().next().map(flatten_instance);
        let missing = row.is_none();
        Ok(json!({
            "row": row.unwrap_or(Value::Null),
            "missing": missing,
        }))
    }
}

fn validate_layout_node(
    node: &Value,
    path: &str,
    block_ids: &HashSet<String>,
    block_types: &HashMap<String, ReportBlockType>,
    filter_ids: &HashSet<String>,
    layout_node_ids: &mut HashSet<String>,
) -> Result<(), ReportServiceError> {
    let Some(object) = node.as_object() else {
        return Err(ReportServiceError::Validation(format!(
            "Report layout node at {path} must be an object"
        )));
    };
    let node_type = object.get("type").and_then(Value::as_str).ok_or_else(|| {
        ReportServiceError::Validation(format!("Report layout node at {path} must include type"))
    })?;
    let node_id = object.get("id").and_then(Value::as_str).ok_or_else(|| {
        ReportServiceError::Validation(format!("Report layout node at {path} must include id"))
    })?;
    if node_id.trim().is_empty() {
        return Err(ReportServiceError::Validation(format!(
            "Report layout node at {path} has an empty id"
        )));
    }
    if !layout_node_ids.insert(node_id.to_string()) {
        return Err(ReportServiceError::Validation(format!(
            "Duplicate report layout node ID '{}'",
            node_id
        )));
    }
    validate_layout_visibility(object, path, filter_ids)?;

    match node_type {
        "block" => {
            let block_id = object
                .get("blockId")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    ReportServiceError::Validation(format!(
                        "Block layout node '{}' must include blockId",
                        node_id
                    ))
                })?;
            validate_layout_block_ref(block_id, block_ids, "layout block")?;
        }
        "metric_row" => {
            let blocks = object
                .get("blocks")
                .and_then(Value::as_array)
                .ok_or_else(|| {
                    ReportServiceError::Validation(format!(
                        "Metric row layout node '{}' must include blocks",
                        node_id
                    ))
                })?;
            if blocks.is_empty() {
                return Err(ReportServiceError::Validation(format!(
                    "Metric row layout node '{}' must include at least one block",
                    node_id
                )));
            }
            for block in blocks {
                let Some(block_id) = block.as_str() else {
                    return Err(ReportServiceError::Validation(format!(
                        "Metric row layout node '{}' blocks entries must be block IDs",
                        node_id
                    )));
                };
                validate_layout_block_ref(block_id, block_ids, "metric row")?;
                if block_types.get(block_id) != Some(&ReportBlockType::Metric) {
                    return Err(ReportServiceError::Validation(format!(
                        "Metric row layout node '{}' references non-metric block '{}'",
                        node_id, block_id
                    )));
                }
            }
        }
        "section" => {
            if let Some(children) = object.get("children") {
                validate_layout_children(
                    children,
                    &format!("{path}.children"),
                    block_ids,
                    block_types,
                    filter_ids,
                    layout_node_ids,
                )?;
            }
        }
        "columns" => {
            let columns = object
                .get("columns")
                .and_then(Value::as_array)
                .ok_or_else(|| {
                    ReportServiceError::Validation(format!(
                        "Columns layout node '{}' must include columns",
                        node_id
                    ))
                })?;
            if columns.is_empty() {
                return Err(ReportServiceError::Validation(format!(
                    "Columns layout node '{}' must include at least one column",
                    node_id
                )));
            }
            for (column_index, column) in columns.iter().enumerate() {
                let Some(column_object) = column.as_object() else {
                    return Err(ReportServiceError::Validation(format!(
                        "Columns layout node '{}' column {} must be an object",
                        node_id, column_index
                    )));
                };
                if column_object.get("id").and_then(Value::as_str).is_none() {
                    return Err(ReportServiceError::Validation(format!(
                        "Columns layout node '{}' column {} must include id",
                        node_id, column_index
                    )));
                }
                if let Some(children) = column_object.get("children") {
                    validate_layout_children(
                        children,
                        &format!("{path}.columns[{column_index}].children"),
                        block_ids,
                        block_types,
                        filter_ids,
                        layout_node_ids,
                    )?;
                }
            }
        }
        "grid" => {
            if let Some(columns) = object.get("columns").and_then(Value::as_i64)
                && columns <= 0
            {
                return Err(ReportServiceError::Validation(format!(
                    "Grid layout node '{}' columns must be positive",
                    node_id
                )));
            }
            let items = object
                .get("items")
                .and_then(Value::as_array)
                .ok_or_else(|| {
                    ReportServiceError::Validation(format!(
                        "Grid layout node '{}' must include items",
                        node_id
                    ))
                })?;
            for (item_index, item) in items.iter().enumerate() {
                let Some(item_object) = item.as_object() else {
                    return Err(ReportServiceError::Validation(format!(
                        "Grid layout node '{}' item {} must be an object",
                        node_id, item_index
                    )));
                };
                let block_id = item_object
                    .get("blockId")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        ReportServiceError::Validation(format!(
                            "Grid layout node '{}' item {} must include blockId",
                            node_id, item_index
                        ))
                    })?;
                validate_layout_block_ref(block_id, block_ids, "grid")?;
                for field in ["colSpan", "rowSpan"] {
                    if let Some(value) = item_object.get(field).and_then(Value::as_i64)
                        && value <= 0
                    {
                        return Err(ReportServiceError::Validation(format!(
                            "Grid layout node '{}' item {} {} must be positive",
                            node_id, item_index, field
                        )));
                    }
                }
            }
        }
        _ => {
            return Err(ReportServiceError::Validation(format!(
                "Report layout node '{}' has unsupported type '{}'",
                node_id, node_type
            )));
        }
    }

    Ok(())
}

fn validate_report_view_navigation(
    views: &[ReportViewDefinition],
    view_ids: &HashSet<String>,
    filter_ids: &HashSet<String>,
) -> Result<(), ReportServiceError> {
    let views_by_id = views
        .iter()
        .map(|view| (view.id.as_str(), view))
        .collect::<HashMap<_, _>>();

    for view in views {
        if let Some(parent_view_id) = view.parent_view_id.as_deref() {
            if parent_view_id.trim().is_empty() {
                return Err(ReportServiceError::Validation(format!(
                    "Report view '{}' parentViewId cannot be empty",
                    view.id
                )));
            }
            if parent_view_id == view.id {
                return Err(ReportServiceError::Validation(format!(
                    "Report view '{}' cannot use itself as parentViewId",
                    view.id
                )));
            }
            if !view_ids.contains(parent_view_id) {
                return Err(ReportServiceError::Validation(format!(
                    "Report view '{}' references unknown parentViewId '{}'",
                    view.id, parent_view_id
                )));
            }
        }

        for filter_id in &view.clear_filters_on_back {
            if !filter_ids.contains(filter_id) {
                return Err(ReportServiceError::Validation(format!(
                    "Report view '{}' clearFiltersOnBack references unknown filter '{}'",
                    view.id, filter_id
                )));
            }
        }
    }

    for view in views {
        let mut seen = HashSet::from([view.id.as_str()]);
        let mut current = view;
        while let Some(parent_view_id) = current.parent_view_id.as_deref() {
            let Some(parent) = views_by_id.get(parent_view_id).copied() else {
                break;
            };
            if !seen.insert(parent.id.as_str()) {
                return Err(ReportServiceError::Validation(format!(
                    "Report view '{}' parentViewId chain contains a cycle",
                    view.id
                )));
            }
            current = parent;
        }
    }

    Ok(())
}

fn validate_block_interactions(
    block: &ReportBlockDefinition,
    filter_ids: &HashSet<String>,
    view_ids: &HashSet<String>,
) -> Result<(), ReportServiceError> {
    let mut interaction_ids = HashSet::new();
    for interaction in &block.interactions {
        if interaction.id.trim().is_empty() {
            return Err(ReportServiceError::Validation(format!(
                "Block '{}' interaction IDs cannot be empty",
                block.id
            )));
        }
        if !interaction_ids.insert(interaction.id.clone()) {
            return Err(ReportServiceError::Validation(format!(
                "Block '{}' has duplicate interaction ID '{}'",
                block.id, interaction.id
            )));
        }
        if interaction.trigger.event.trim().is_empty() {
            return Err(ReportServiceError::Validation(format!(
                "Block '{}' interaction '{}' must include trigger.event",
                block.id, interaction.id
            )));
        }
        for action in &interaction.actions {
            validate_report_interaction_action(
                action,
                filter_ids,
                view_ids,
                &format!("Block '{}' interaction '{}'", block.id, interaction.id),
            )?;
        }
    }

    Ok(())
}

fn validate_report_interaction_action(
    action: &ReportInteractionAction,
    filter_ids: &HashSet<String>,
    view_ids: &HashSet<String>,
    context: &str,
) -> Result<(), ReportServiceError> {
    match action.action_type.as_str() {
        "set_filter" => {
            let Some(filter_id) = action.filter_id.as_deref() else {
                return Err(ReportServiceError::Validation(format!(
                    "{context} set_filter action must include filterId"
                )));
            };
            if !filter_ids.contains(filter_id) {
                return Err(ReportServiceError::Validation(format!(
                    "{context} references unknown filter '{filter_id}'"
                )));
            }
            if action.value_from.is_none() && action.value.is_none() {
                return Err(ReportServiceError::Validation(format!(
                    "{context} set_filter action must include value or valueFrom"
                )));
            }
        }
        "clear_filter" => {
            let Some(filter_id) = action.filter_id.as_deref() else {
                return Err(ReportServiceError::Validation(format!(
                    "{context} clear_filter action must include filterId"
                )));
            };
            if !filter_ids.contains(filter_id) {
                return Err(ReportServiceError::Validation(format!(
                    "{context} references unknown filter '{filter_id}'"
                )));
            }
        }
        "clear_filters" => {
            for filter_id in &action.filter_ids {
                if !filter_ids.contains(filter_id) {
                    return Err(ReportServiceError::Validation(format!(
                        "{context} references unknown filter '{filter_id}'"
                    )));
                }
            }
        }
        "navigate_view" => {
            let Some(view_id) = action.view_id.as_deref() else {
                return Err(ReportServiceError::Validation(format!(
                    "{context} navigate_view action must include viewId"
                )));
            };
            if !view_ids.contains(view_id) {
                return Err(ReportServiceError::Validation(format!(
                    "{context} references unknown view '{view_id}'"
                )));
            }
        }
        _ => {}
    }

    Ok(())
}

fn validate_report_table_interaction_button_columns(
    table: &ReportTableConfig,
    filter_ids: &HashSet<String>,
    view_ids: &HashSet<String>,
    is_known_field: &dyn Fn(&str) -> bool,
    context: &str,
) -> Result<(), ReportServiceError> {
    for column in &table.columns {
        if column.is_interaction_buttons() {
            validate_report_interaction_buttons(
                &column.interaction_buttons,
                filter_ids,
                view_ids,
                is_known_field,
                &format!("{context} interaction button column '{}'", column.field),
            )?;
        }
    }

    Ok(())
}

fn validate_report_interaction_buttons(
    buttons: &[ReportTableInteractionButtonConfig],
    filter_ids: &HashSet<String>,
    view_ids: &HashSet<String>,
    is_known_field: &dyn Fn(&str) -> bool,
    context: &str,
) -> Result<(), ReportServiceError> {
    if buttons.is_empty() {
        return Err(ReportServiceError::Validation(format!(
            "{context} must define interactionButtons"
        )));
    }

    let row_field_known = |field: &str| {
        is_known_field(field)
            || is_report_row_metadata_field(field)
            || field
                .split_once('.')
                .is_some_and(|(base, _)| is_known_field(base) || is_report_row_metadata_field(base))
    };

    let mut button_ids = HashSet::new();
    for button in buttons {
        if button.id.trim().is_empty() {
            return Err(ReportServiceError::Validation(format!(
                "{context} button IDs cannot be empty"
            )));
        }
        if !button_ids.insert(button.id.clone()) {
            return Err(ReportServiceError::Validation(format!(
                "{context} has duplicate button ID '{}'",
                button.id
            )));
        }
        if button.actions.is_empty() {
            return Err(ReportServiceError::Validation(format!(
                "{context} button '{}' must define at least one action",
                button.id
            )));
        }
        for action in &button.actions {
            validate_report_interaction_action(
                action,
                filter_ids,
                view_ids,
                &format!("{context} button '{}'", button.id),
            )?;
        }
        if let Some(condition) = &button.visible_when {
            validate_report_workflow_action_row_condition(
                condition,
                &row_field_known,
                context,
                &format!("interactionButtons['{}'].visibleWhen", button.id),
            )?;
        }
        if let Some(condition) = &button.hidden_when {
            validate_report_workflow_action_row_condition(
                condition,
                &row_field_known,
                context,
                &format!("interactionButtons['{}'].hiddenWhen", button.id),
            )?;
        }
        if let Some(condition) = &button.disabled_when {
            validate_report_workflow_action_row_condition(
                condition,
                &row_field_known,
                context,
                &format!("interactionButtons['{}'].disabledWhen", button.id),
            )?;
        }
    }

    Ok(())
}

fn validate_report_markdown_block_shape(
    block: &ReportBlockDefinition,
) -> Result<Vec<ReportMarkdownSourcePlaceholder>, ReportServiceError> {
    if block.table.is_some()
        || block.chart.is_some()
        || block.metric.is_some()
        || block.actions.is_some()
        || block.card.is_some()
    {
        return Err(report_validation_error(
            "$",
            "INVALID_MARKDOWN_BLOCK_CONFIG",
            format!(
                "Block '{}' markdown blocks must not define table, chart, metric, actions, or card config",
                block.id
            ),
            Some("Put narrative content in block.markdown.content and arrange it with layout block nodes.".to_string()),
        ));
    }
    let markdown = block.markdown.as_ref().ok_or_else(|| {
        report_validation_error(
            "$",
            "MISSING_MARKDOWN_CONFIG",
            format!(
                "Block '{}' markdown block must define markdown.content",
                block.id
            ),
            Some("Use {\"type\":\"markdown\",\"markdown\":{\"content\":\"...\"}}.".to_string()),
        )
    })?;
    if markdown.content.len() > 250_000 {
        return Err(report_validation_error(
            "$",
            "MARKDOWN_CONTENT_TOO_LARGE",
            format!("Block '{}' markdown.content is too large", block.id),
            Some("Keep markdown.content under 250000 bytes.".to_string()),
        ));
    }
    report_markdown_source_placeholders(&markdown.content, &format!("block '{}'", block.id))
}

fn report_markdown_source_placeholders(
    content: &str,
    context: &str,
) -> Result<Vec<ReportMarkdownSourcePlaceholder>, ReportServiceError> {
    let token_re =
        Regex::new(r"\{\{\s*([^{}]+?)\s*\}\}").expect("report markdown token regex must compile");
    let source_re = Regex::new(r"^source(?:\[[0-9]+\])?\.([A-Za-z0-9_.-]+)$")
        .expect("report markdown source placeholder regex must compile");
    let mut placeholders = Vec::new();
    for capture in token_re.captures_iter(content) {
        let expression = capture
            .get(1)
            .map(|value| value.as_str().trim())
            .unwrap_or_default();
        if let Some(source_capture) = source_re.captures(expression) {
            let field_path = source_capture
                .get(1)
                .map(|value| value.as_str().trim().to_string())
                .unwrap_or_default();
            if field_path.is_empty() {
                return Err(report_validation_error(
                    "$",
                    "INVALID_MARKDOWN_SOURCE_PLACEHOLDER",
                    format!("{context} has an empty markdown source placeholder"),
                    Some(
                        "Use {{source.field}} or {{source[0].field}} with a non-empty field path."
                            .to_string(),
                    ),
                ));
            }
            placeholders.push(ReportMarkdownSourcePlaceholder { field_path });
            continue;
        }
        return Err(report_validation_error(
            "$",
            "UNSUPPORTED_MARKDOWN_PLACEHOLDER",
            format!(
                "{context} uses unsupported markdown placeholder '{{{{{expression}}}}}'"
            ),
            Some(
                "Markdown blocks support only {{source.field}} and {{source[0].field}} interpolation."
                    .to_string(),
            ),
        ));
    }
    Ok(placeholders)
}

fn validate_report_markdown_placeholders_have_no_source(
    block: &ReportBlockDefinition,
    placeholders: &[ReportMarkdownSourcePlaceholder],
) -> Result<(), ReportServiceError> {
    if !placeholders.is_empty() {
        return Err(report_validation_error(
            "$",
            "MISSING_MARKDOWN_SOURCE",
            format!(
                "Block '{}' markdown placeholders reference source data but the block has no source or dataset",
                block.id
            ),
            Some(
                "Add block.source or block.dataset, or remove {{source...}} placeholders."
                    .to_string(),
            ),
        ));
    }
    Ok(())
}

fn validate_report_markdown_placeholders(
    block: &ReportBlockDefinition,
    placeholders: &[ReportMarkdownSourcePlaceholder],
    is_known_field: &dyn Fn(&str) -> bool,
) -> Result<(), ReportServiceError> {
    for placeholder in placeholders {
        if !is_known_field(&placeholder.field_path) {
            return Err(report_validation_error(
                "$",
                "UNKNOWN_MARKDOWN_SOURCE_FIELD",
                format!(
                    "Block '{}' markdown references unknown source field '{}'",
                    block.id, placeholder.field_path
                ),
                Some("Use a field produced by the markdown block source or dataset.".to_string()),
            ));
        }
    }
    Ok(())
}

fn markdown_output_field_known(field_path: &str, is_known_field: &dyn Fn(&str) -> bool) -> bool {
    is_known_field(field_path)
        || field_path
            .split_once('.')
            .map(|(first, _)| is_known_field(first))
            .unwrap_or(false)
}

fn dataset_output_field_known(source: &ReportSource, field_path: &str) -> bool {
    let output_fields = aggregate_source_output_fields(&source.group_by, &source.aggregates);
    markdown_output_field_known(field_path, &|candidate| output_fields.contains(candidate))
}

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

fn validate_system_block(
    block: &ReportBlockDefinition,
    filter_ids: &HashSet<String>,
    view_ids: &HashSet<String>,
    filter_defs: &HashMap<String, &ReportFilterDefinition>,
) -> Result<(), ReportServiceError> {
    let entity = system_entity(block)?;
    if !block.source.schema.trim().is_empty() {
        return Err(ReportServiceError::Validation(format!(
            "Block '{}' system source must not set schema",
            block.id
        )));
    }
    if block.source.workflow_id.is_some() || block.source.instance_id.is_some() {
        return Err(ReportServiceError::Validation(format!(
            "Block '{}' system source must not set workflowId or instanceId",
            block.id
        )));
    }
    if !block.source.join.is_empty() {
        return Err(ReportServiceError::Validation(format!(
            "Block '{}' system source does not support joins",
            block.id
        )));
    }
    if matches!(
        block.block_type,
        ReportBlockType::Actions | ReportBlockType::Card
    ) {
        return Err(ReportServiceError::Validation(format!(
            "Block '{}' system source only supports table, chart, metric, and markdown blocks",
            block.id
        )));
    }

    let fields = system_fields(entity);
    let aggregate_output_fields = aggregate_output_fields(block);
    validate_report_aggregate_specs(&format!("block '{}'", block.id), &block.source.aggregates)?;
    if block.source.mode == ReportSourceMode::Aggregate && block.source.aggregates.is_empty() {
        return Err(ReportServiceError::Validation(format!(
            "Block '{}' aggregate source must define at least one aggregate",
            block.id
        )));
    }
    validate_report_condition_filter_refs(
        block.source.condition.as_ref(),
        filter_defs,
        &format!("block '{}'", block.id),
    )?;
    validate_report_condition_field_refs(
        block.source.condition.as_ref(),
        &|field| system_row_field_known(&fields, field),
        &format!("block '{}'", block.id),
    )?;
    validate_report_source_filter_mappings(
        &block.source.filter_mappings,
        filter_ids,
        &|field| system_row_field_known(&fields, field),
        "source.filterMappings",
        &format!("block '{}'", block.id),
    )?;
    let is_table_value_field = |field: &str| -> bool {
        match block.source.mode {
            ReportSourceMode::Filter => system_row_field_known(&fields, field),
            ReportSourceMode::Aggregate => aggregate_output_fields.contains(field),
        }
    };

    if let Some(table) = &block.table {
        for column in &table.columns {
            if column.is_interaction_buttons() {
                validate_report_interaction_buttons(
                    &column.interaction_buttons,
                    filter_ids,
                    view_ids,
                    &is_table_value_field,
                    &format!(
                        "Block '{}' interaction button column '{}'",
                        block.id, column.field
                    ),
                )?;
                continue;
            }
            if column.is_chart() || column.is_value_lookup() || column.is_workflow_button() {
                return Err(ReportServiceError::Validation(format!(
                    "Block '{}' system table columns cannot use nested sources or workflow buttons",
                    block.id
                )));
            }
            if !is_table_value_field(&column.field) {
                return Err(ReportServiceError::Validation(format!(
                    "Block '{}' references unknown system table field '{}'",
                    block.id, column.field
                )));
            }
            if let Some(display_field) = &column.display_field
                && !is_table_value_field(display_field)
            {
                return Err(ReportServiceError::Validation(format!(
                    "Block '{}' references unknown system table displayField '{}'",
                    block.id, display_field
                )));
            }
        }
        for sort in &table.default_sort {
            if !is_table_value_field(&sort.field) {
                return Err(ReportServiceError::Validation(format!(
                    "Block '{}' references unknown system table sort field '{}'",
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

    for group_field in &block.source.group_by {
        if !system_row_field_known(&fields, group_field) {
            return Err(ReportServiceError::Validation(format!(
                "Block '{}' references unknown system groupBy field '{}'",
                block.id, group_field
            )));
        }
    }
    for aggregate in &block.source.aggregates {
        if let Some(field) = &aggregate.field
            && !system_row_field_known(&fields, field)
        {
            return Err(ReportServiceError::Validation(format!(
                "Block '{}' references unknown system aggregate field '{}'",
                block.id, field
            )));
        }
        for order_by in &aggregate.order_by {
            if !system_row_field_known(&fields, &order_by.field) {
                return Err(ReportServiceError::Validation(format!(
                    "Block '{}' aggregate '{}' references unknown system orderBy field '{}'",
                    block.id, aggregate.alias, order_by.field
                )));
            }
        }
    }
    for order_by in &block.source.order_by {
        let known = match block.source.mode {
            ReportSourceMode::Filter => system_row_field_known(&fields, &order_by.field),
            ReportSourceMode::Aggregate => aggregate_output_fields.contains(&order_by.field),
        };
        if !known {
            return Err(ReportServiceError::Validation(format!(
                "Block '{}' references unknown system orderBy field '{}'",
                block.id, order_by.field
            )));
        }
    }
    if let Some(chart) = &block.chart {
        if !aggregate_output_fields.contains(&chart.x) {
            return Err(ReportServiceError::Validation(format!(
                "Block '{}' references unknown system chart x field '{}'",
                block.id, chart.x
            )));
        }
        for series in &chart.series {
            if !aggregate_output_fields.contains(&series.field) {
                return Err(ReportServiceError::Validation(format!(
                    "Block '{}' references unknown system chart series field '{}'",
                    block.id, series.field
                )));
            }
        }
    }
    if let Some(metric) = &block.metric
        && !aggregate_output_fields.contains(&metric.value_field)
    {
        return Err(ReportServiceError::Validation(format!(
            "Block '{}' references unknown system metric valueField '{}'",
            block.id, metric.value_field
        )));
    }

    validate_block_interactions(block, filter_ids, view_ids)
}

fn validate_report_workflow_action_config(
    action: &ReportWorkflowActionConfig,
    context: &str,
) -> Result<(), ReportServiceError> {
    if action.workflow_id.trim().is_empty() {
        return Err(ReportServiceError::Validation(format!(
            "{context} workflowAction.workflowId must not be empty"
        )));
    }
    if action.version.is_some_and(|version| version <= 0) {
        return Err(ReportServiceError::Validation(format!(
            "{context} workflowAction.version must be greater than zero"
        )));
    }
    if let Some(input_key) = &action.context.input_key
        && input_key.trim().is_empty()
    {
        return Err(ReportServiceError::Validation(format!(
            "{context} workflowAction.context.inputKey must not be empty"
        )));
    }
    Ok(())
}

fn collect_card_workflow_actions<'a>(
    card: &'a ReportCardConfig,
    context: &str,
    actions: &mut Vec<(&'a ReportWorkflowActionConfig, String)>,
) {
    for group in &card.groups {
        let group_context = match group.title.as_deref() {
            Some(title) if !title.trim().is_empty() => format!("{context} group '{}'", title),
            _ => context.to_string(),
        };
        for field in &group.fields {
            let field_context = format!("{group_context} field '{}'", field.field);
            if let Some(action) = &field.workflow_action {
                actions.push((action, field_context.clone()));
            }
            if let Some(subcard) = &field.subcard {
                collect_card_workflow_actions(subcard, &field_context, actions);
            }
        }
    }
}

fn validate_report_table_action_config(
    action: &ReportTableActionConfig,
    context: &str,
) -> Result<(), ReportServiceError> {
    if action.id.trim().is_empty() {
        return Err(ReportServiceError::Validation(format!(
            "{context} id must not be empty"
        )));
    }
    validate_report_workflow_action_config(&action.workflow_action, context)?;
    if action.workflow_action.context.mode != ReportWorkflowActionContextMode::Selection {
        return Err(ReportServiceError::Validation(format!(
            "{context} workflowAction.context.mode must be 'selection'"
        )));
    }
    Ok(())
}

fn validate_report_workflow_action_context_field(
    action: &ReportWorkflowActionConfig,
    fallback_field: &str,
    is_known_field: impl Fn(&str) -> bool,
    context: &str,
) -> Result<(), ReportServiceError> {
    let field = match action.context.mode {
        ReportWorkflowActionContextMode::Row => return Ok(()),
        ReportWorkflowActionContextMode::Selection => {
            return Err(ReportServiceError::Validation(format!(
                "{context} workflowAction.context.mode 'selection' is only supported by table actions"
            )));
        }
        ReportWorkflowActionContextMode::Field => action
            .context
            .field
            .as_deref()
            .unwrap_or(fallback_field)
            .trim(),
        ReportWorkflowActionContextMode::Value => fallback_field.trim(),
    };

    if field.is_empty() {
        return Err(ReportServiceError::Validation(format!(
            "{context} workflowAction context field must not be empty"
        )));
    }
    if !is_known_field(field) {
        return Err(ReportServiceError::Validation(format!(
            "{context} references unknown workflowAction context field '{}'",
            field
        )));
    }
    Ok(())
}

fn validate_report_workflow_action_row_conditions(
    action: &ReportWorkflowActionConfig,
    is_known_field: impl Fn(&str) -> bool,
    context: &str,
) -> Result<(), ReportServiceError> {
    let is_known_field = |field: &str| {
        is_known_field(field)
            || is_report_row_metadata_field(field)
            || field
                .split_once('.')
                .is_some_and(|(base, _)| is_known_field(base) || is_report_row_metadata_field(base))
    };

    if let Some(condition) = &action.visible_when {
        validate_report_workflow_action_row_condition(
            condition,
            &is_known_field,
            context,
            "workflowAction.visibleWhen",
        )?;
    }
    if let Some(condition) = &action.hidden_when {
        validate_report_workflow_action_row_condition(
            condition,
            &is_known_field,
            context,
            "workflowAction.hiddenWhen",
        )?;
    }
    if let Some(condition) = &action.disabled_when {
        validate_report_workflow_action_row_condition(
            condition,
            &is_known_field,
            context,
            "workflowAction.disabledWhen",
        )?;
    }

    Ok(())
}

fn validate_report_workflow_action_row_condition(
    condition: &Condition,
    is_known_field: &dyn Fn(&str) -> bool,
    context: &str,
    condition_path: &str,
) -> Result<(), ReportServiceError> {
    let op = condition.op.trim().to_ascii_uppercase();
    if op.is_empty() {
        return Err(ReportServiceError::Validation(format!(
            "{context} {condition_path}.op must not be empty"
        )));
    }

    let args = condition.arguments.as_deref().unwrap_or(&[]);
    match op.as_str() {
        "AND" | "OR" => {
            for (index, argument) in args.iter().enumerate() {
                let child = workflow_action_row_condition_child(
                    argument,
                    context,
                    &format!("{condition_path}.arguments[{index}]"),
                )?;
                validate_report_workflow_action_row_condition(
                    &child,
                    is_known_field,
                    context,
                    &format!("{condition_path}.arguments[{index}]"),
                )?;
            }
        }
        "NOT" => {
            if args.len() != 1 {
                return Err(ReportServiceError::Validation(format!(
                    "{context} {condition_path} NOT condition requires one argument"
                )));
            }
            let child = workflow_action_row_condition_child(
                &args[0],
                context,
                &format!("{condition_path}.arguments[0]"),
            )?;
            validate_report_workflow_action_row_condition(
                &child,
                is_known_field,
                context,
                &format!("{condition_path}.arguments[0]"),
            )?;
        }
        "EQ" | "NE" | "GT" | "GTE" | "LT" | "LTE" | "CONTAINS" => {
            if args.len() != 2 {
                return Err(ReportServiceError::Validation(format!(
                    "{context} {condition_path} {op} condition requires two arguments"
                )));
            }
            validate_report_workflow_action_condition_field(
                &args[0],
                is_known_field,
                context,
                &format!("{condition_path}.arguments[0]"),
            )?;
        }
        "IN" | "NOT_IN" => {
            if args.len() != 2 {
                return Err(ReportServiceError::Validation(format!(
                    "{context} {condition_path} {op} condition requires two arguments"
                )));
            }
            validate_report_workflow_action_condition_field(
                &args[0],
                is_known_field,
                context,
                &format!("{condition_path}.arguments[0]"),
            )?;
            if !args[1].is_array() {
                return Err(ReportServiceError::Validation(format!(
                    "{context} {condition_path}.arguments[1] must be an array for {op}"
                )));
            }
        }
        "IS_DEFINED" | "IS_EMPTY" | "IS_NOT_EMPTY" => {
            if args.len() != 1 {
                return Err(ReportServiceError::Validation(format!(
                    "{context} {condition_path} {op} condition requires one argument"
                )));
            }
            validate_report_workflow_action_condition_field(
                &args[0],
                is_known_field,
                context,
                &format!("{condition_path}.arguments[0]"),
            )?;
        }
        _ => {
            return Err(ReportServiceError::Validation(format!(
                "{context} {condition_path} uses unsupported row condition op '{}'",
                condition.op
            )));
        }
    }

    Ok(())
}

fn workflow_action_row_condition_child(
    argument: &Value,
    context: &str,
    path: &str,
) -> Result<Condition, ReportServiceError> {
    serde_json::from_value::<Condition>(argument.clone()).map_err(|err| {
        ReportServiceError::Validation(format!(
            "{context} {path} must be a condition object: {err}"
        ))
    })
}

fn validate_report_workflow_action_condition_field(
    argument: &Value,
    is_known_field: &dyn Fn(&str) -> bool,
    context: &str,
    path: &str,
) -> Result<(), ReportServiceError> {
    let Some(field) = argument
        .as_str()
        .map(str::trim)
        .filter(|field| !field.is_empty())
    else {
        return Err(ReportServiceError::Validation(format!(
            "{context} {path} must be a non-empty row field name"
        )));
    };
    if !is_known_field(field) {
        return Err(ReportServiceError::Validation(format!(
            "{context} references unknown workflowAction row condition field '{}'",
            field
        )));
    }
    Ok(())
}

fn is_report_row_metadata_field(field: &str) -> bool {
    matches!(
        field,
        "id" | "schemaId" | "schemaName" | "tenantId" | "createdAt" | "updatedAt"
    )
}

fn workflow_runtime_row_field_known(fields: &HashSet<&'static str>, field: &str) -> bool {
    fields.contains(field)
        || field
            .split_once('.')
            .is_some_and(|(base, _)| fields.contains(base))
}

fn validate_layout_visibility(
    object: &serde_json::Map<String, Value>,
    path: &str,
    filter_ids: &HashSet<String>,
) -> Result<(), ReportServiceError> {
    validate_show_when_value(
        object.get("showWhen"),
        &format!("layout node at {path}"),
        filter_ids,
    )
}

fn validate_show_when_value(
    show_when: Option<&Value>,
    context: &str,
    filter_ids: &HashSet<String>,
) -> Result<(), ReportServiceError> {
    let Some(show_when) = show_when else {
        return Ok(());
    };
    let Some(show_when) = show_when.as_object() else {
        return Err(ReportServiceError::Validation(format!(
            "Report {context} showWhen must be an object"
        )));
    };
    let filter_id = show_when
        .get("filter")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            ReportServiceError::Validation(format!("Report {context} showWhen must include filter"))
        })?;
    if !filter_ids.contains(filter_id) {
        return Err(ReportServiceError::Validation(format!(
            "Report {context} showWhen references unknown filter '{}'",
            filter_id
        )));
    }
    if let Some(exists) = show_when.get("exists")
        && !exists.is_boolean()
    {
        return Err(ReportServiceError::Validation(format!(
            "Report {context} showWhen.exists must be boolean"
        )));
    }
    Ok(())
}

fn workflow_runtime_fields(entity: ReportWorkflowRuntimeEntity) -> HashSet<&'static str> {
    match entity {
        ReportWorkflowRuntimeEntity::Instances => [
            "id",
            "instanceId",
            "workflowId",
            "workflowName",
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

fn validate_layout_children(
    children: &Value,
    path: &str,
    block_ids: &HashSet<String>,
    block_types: &HashMap<String, ReportBlockType>,
    filter_ids: &HashSet<String>,
    layout_node_ids: &mut HashSet<String>,
) -> Result<(), ReportServiceError> {
    let Some(children) = children.as_array() else {
        return Err(ReportServiceError::Validation(format!(
            "Report layout children at {path} must be an array"
        )));
    };
    for (index, child) in children.iter().enumerate() {
        validate_layout_node(
            child,
            &format!("{path}[{index}]"),
            block_ids,
            block_types,
            filter_ids,
            layout_node_ids,
        )?;
    }
    Ok(())
}

fn validate_layout_block_ref(
    block_id: &str,
    block_ids: &HashSet<String>,
    context: &str,
) -> Result<(), ReportServiceError> {
    if block_ids.contains(block_id) {
        Ok(())
    } else {
        Err(ReportServiceError::Validation(format!(
            "Report {context} references unknown block '{}'",
            block_id
        )))
    }
}

fn requested_blocks<'a>(
    definition: &'a ReportDefinition,
    requested: Option<&[ReportBlockDataRequest]>,
) -> Vec<&'a ReportBlockDefinition> {
    match requested {
        Some(requested) if !requested.is_empty() => {
            let ids: HashSet<_> = requested.iter().map(|block| block.id.as_str()).collect();
            definition
                .blocks
                .iter()
                .filter(|block| ids.contains(block.id.as_str()))
                .collect()
        }
        _ => definition
            .blocks
            .iter()
            .filter(|block| !block.lazy)
            .collect(),
    }
}

fn resolve_filters(
    definition: &ReportDefinition,
    runtime_filters: &HashMap<String, Value>,
) -> HashMap<String, Value> {
    definition
        .filters
        .iter()
        .map(|filter| {
            let raw = runtime_filters
                .get(&filter.id)
                .cloned()
                .or_else(|| filter.default.clone())
                .unwrap_or(Value::Null);
            (filter.id.clone(), resolve_filter_value(filter, raw))
        })
        .collect()
}

fn resolve_filter_value(filter: &ReportFilterDefinition, raw: Value) -> Value {
    if filter.filter_type != ReportFilterType::TimeRange {
        return raw;
    }

    let preset = match &raw {
        Value::String(value) => Some(value.as_str()),
        Value::Object(map) => map.get("preset").and_then(Value::as_str),
        _ => None,
    };

    if let Some(preset) = preset {
        let (from, to, label) = resolve_time_preset(preset);
        return json!({
            "from": from.to_rfc3339(),
            "to": to.to_rfc3339(),
            "label": label,
            "preset": preset,
        });
    }

    raw
}

fn resolve_time_preset(preset: &str) -> (DateTime<Utc>, DateTime<Utc>, String) {
    let now = Utc::now();
    let today =
        DateTime::<Utc>::from_naive_utc_and_offset(now.date_naive().and_time(NaiveTime::MIN), Utc);

    match preset {
        "today" => (today, today + Duration::days(1), "Today".to_string()),
        "yesterday" => (today - Duration::days(1), today, "Yesterday".to_string()),
        "last_7_days" => (
            today - Duration::days(6),
            today + Duration::days(1),
            "Last 7 days".to_string(),
        ),
        "this_month" => {
            let start = DateTime::<Utc>::from_naive_utc_and_offset(
                now.date_naive()
                    .with_day(1)
                    .unwrap_or(now.date_naive())
                    .and_time(NaiveTime::MIN),
                Utc,
            );
            (start, today + Duration::days(1), "This month".to_string())
        }
        _ => (
            today - Duration::days(29),
            today + Duration::days(1),
            "Last 30 days".to_string(),
        ),
    }
}

#[derive(Debug)]
struct ReportConditionFilterRef {
    filter_id: String,
    path: String,
}

fn report_filter_definitions_by_id(
    filters: &[ReportFilterDefinition],
) -> HashMap<String, &ReportFilterDefinition> {
    filters
        .iter()
        .map(|filter| (filter.id.clone(), filter))
        .collect()
}

fn block_condition_filter_definitions<'a>(
    definition: &'a ReportDefinition,
    block: &'a ReportBlockDefinition,
) -> HashMap<String, &'a ReportFilterDefinition> {
    let mut filter_defs = report_filter_definitions_by_id(&definition.filters);
    for filter in &block.filters {
        filter_defs.insert(filter.id.clone(), filter);
    }
    filter_defs
}

fn block_condition_filter_values(
    block: &ReportBlockDefinition,
    resolved_filters: &HashMap<String, Value>,
    block_request: Option<&ReportBlockDataRequest>,
) -> HashMap<String, Value> {
    let mut values = resolved_filters.clone();
    for filter in &block.filters {
        let raw = block_request
            .and_then(|request| request.block_filters.get(&filter.id).cloned())
            .or_else(|| filter.default.clone())
            .unwrap_or(Value::Null);
        values.insert(filter.id.clone(), resolve_filter_value(filter, raw));
    }
    values
}

fn validate_filter_option_condition_filter_refs(
    filter: &ReportFilterDefinition,
    filter_defs: &HashMap<String, &ReportFilterDefinition>,
) -> Result<(), ReportServiceError> {
    let Some(condition) = filter
        .options
        .as_ref()
        .and_then(Value::as_object)
        .and_then(|options| options.get("condition"))
    else {
        return Ok(());
    };
    let parsed = serde_json::from_value::<Condition>(condition.clone()).map_err(|err| {
        ReportServiceError::Validation(format!(
            "Filter '{}' options.condition is invalid: {}",
            filter.id, err
        ))
    })?;
    validate_report_condition_filter_refs(
        Some(&parsed),
        filter_defs,
        &format!("filter '{}'", filter.id),
    )
}

fn validate_report_condition_filter_refs(
    condition: Option<&Condition>,
    filter_defs: &HashMap<String, &ReportFilterDefinition>,
    context: &str,
) -> Result<(), ReportServiceError> {
    let Some(condition) = condition else {
        return Ok(());
    };
    let Some(arguments) = condition.arguments.as_ref() else {
        return Ok(());
    };
    for argument in arguments {
        if let Some(reference) = parse_report_condition_filter_ref(argument)? {
            let filter = filter_defs.get(&reference.filter_id).ok_or_else(|| {
                ReportServiceError::Validation(format!(
                    "{} condition references unknown filter '{}'",
                    context, reference.filter_id
                ))
            })?;
            validate_condition_filter_ref_path(filter, &reference, context)?;
        }
        if let Some(subquery) = parse_report_condition_subquery_operand(argument)? {
            if let Some(condition) = subquery.get("condition") {
                let child =
                    serde_json::from_value::<Condition>(condition.clone()).map_err(|err| {
                        ReportServiceError::Validation(format!(
                            "{} condition subquery.condition is invalid: {}",
                            context, err
                        ))
                    })?;
                validate_report_condition_filter_refs(Some(&child), filter_defs, context)?;
            }
            continue;
        }
        if let Some(child) = condition_from_value(argument) {
            validate_report_condition_filter_refs(Some(&child), filter_defs, context)?;
        }
    }
    Ok(())
}

fn validate_report_condition_field_refs(
    condition: Option<&Condition>,
    is_known_field: &dyn Fn(&str) -> bool,
    context: &str,
) -> Result<(), ReportServiceError> {
    if let Some(condition) = condition {
        validate_report_condition_field_refs_at(condition, is_known_field, context, "condition")?;
    }
    Ok(())
}

fn validate_report_condition_field_refs_at(
    condition: &Condition,
    is_known_field: &dyn Fn(&str) -> bool,
    context: &str,
    path: &str,
) -> Result<(), ReportServiceError> {
    let op = condition.op.to_ascii_uppercase();
    let args = condition.arguments.as_deref().ok_or_else(|| {
        report_validation_error(
            "$",
            "INVALID_CONDITION_ARGUMENTS",
            format!(
            "{} {} operator '{}' requires arguments",
            context, path, condition.op
            ),
            Some("Conditions must use { op, arguments } with the field name as the first operand for comparison operators.".to_string()),
        )
    })?;

    match op.as_str() {
        "AND" | "OR" => {
            if args.is_empty() {
                return Err(report_validation_error(
                    "$",
                    "INVALID_CONDITION_ARGUMENTS",
                    format!(
                        "{} {} operator '{}' requires at least one condition argument",
                        context, path, condition.op
                    ),
                    Some("AND/OR arguments must be condition objects.".to_string()),
                ));
            }
            for (index, argument) in args.iter().enumerate() {
                let child = condition_from_value(argument).ok_or_else(|| {
                    report_validation_error(
                        "$",
                        "INVALID_CONDITION_ARGUMENTS",
                        format!(
                            "{} {} operator '{}' argument {} must be a condition object",
                            context, path, condition.op, index
                        ),
                        Some("Use nested condition objects inside logical operators.".to_string()),
                    )
                })?;
                validate_report_condition_field_refs_at(
                    &child,
                    is_known_field,
                    context,
                    &format!("{path}.arguments[{index}]"),
                )?;
            }
        }
        "NOT" => {
            if args.len() != 1 {
                return Err(report_validation_error(
                    "$",
                    "INVALID_CONDITION_ARGUMENTS",
                    format!(
                        "{} {} operator '{}' requires exactly one condition argument",
                        context, path, condition.op
                    ),
                    Some("NOT must wrap exactly one condition object.".to_string()),
                ));
            }
            let child = condition_from_value(&args[0]).ok_or_else(|| {
                report_validation_error(
                    "$",
                    "INVALID_CONDITION_ARGUMENTS",
                    format!(
                        "{} {} operator '{}' argument 0 must be a condition object",
                        context, path, condition.op
                    ),
                    Some("NOT must wrap exactly one condition object.".to_string()),
                )
            })?;
            validate_report_condition_field_refs_at(
                &child,
                is_known_field,
                context,
                &format!("{path}.arguments[0]"),
            )?;
        }
        "EQ" | "NE" | "GT" | "LT" | "GTE" | "LTE" | "CONTAINS" | "IN" | "NOT_IN" => {
            validate_report_condition_arg_count(context, path, &condition.op, args, 2)?;
            validate_report_condition_field_arg(
                context,
                path,
                &condition.op,
                args,
                is_known_field,
            )?;
        }
        "IS_EMPTY" | "IS_NOT_EMPTY" | "IS_DEFINED" => {
            validate_report_condition_arg_count(context, path, &condition.op, args, 1)?;
            validate_report_condition_field_arg(
                context,
                path,
                &condition.op,
                args,
                is_known_field,
            )?;
        }
        "SIMILARITY_GTE" | "COSINE_DISTANCE_LTE" | "L2_DISTANCE_LTE" => {
            validate_report_condition_arg_count(context, path, &condition.op, args, 3)?;
            validate_report_condition_field_arg(
                context,
                path,
                &condition.op,
                args,
                is_known_field,
            )?;
        }
        _ => {
            return Err(report_validation_error(
                "$",
                "UNSUPPORTED_CONDITION_OPERATOR",
                format!(
                    "{} {} uses unsupported condition operator '{}'",
                    context, path, condition.op
                ),
                Some("Use Object Model condition operators such as EQ, NE, GT, GTE, LT, LTE, IN, NOT_IN, CONTAINS, IS_DEFINED, IS_EMPTY, or IS_NOT_EMPTY.".to_string()),
            ));
        }
    }

    Ok(())
}

fn validate_report_condition_arg_count(
    context: &str,
    path: &str,
    op: &str,
    args: &[Value],
    expected: usize,
) -> Result<(), ReportServiceError> {
    if args.len() != expected {
        return Err(report_validation_error(
            "$",
            "INVALID_CONDITION_ARGUMENTS",
            format!(
                "{} {} operator '{}' requires exactly {} argument{}",
                context,
                path,
                op,
                expected,
                if expected == 1 { "" } else { "s" }
            ),
            Some("Check the condition operator arity and operand order.".to_string()),
        ));
    }
    Ok(())
}

fn validate_report_condition_field_arg(
    context: &str,
    path: &str,
    op: &str,
    args: &[Value],
    is_known_field: &dyn Fn(&str) -> bool,
) -> Result<(), ReportServiceError> {
    let field = args
        .first()
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|field| !field.is_empty())
        .ok_or_else(|| {
            report_validation_error(
                "$",
                "INVALID_CONDITION_FIELD",
                format!(
                    "{} {} operator '{}' first argument must be a non-empty field name",
                    context, path, op
                ),
                Some(
                    "The first operand must be a field available from the report source."
                        .to_string(),
                ),
            )
        })?;
    if !is_known_field(field) {
        return Err(report_validation_error(
            "$",
            "UNKNOWN_CONDITION_FIELD",
            format!("{} {} references unknown field '{}'", context, path, field),
            Some("Use a field from the source schema, joined schema alias, dataset output, workflow runtime entity, or system entity for this condition.".to_string()),
        ));
    }
    Ok(())
}

fn validate_report_source_filter_mappings(
    mappings: &[ReportFilterTarget],
    filter_ids: &HashSet<String>,
    is_known_field: &dyn Fn(&str) -> bool,
    path: &str,
    context: &str,
) -> Result<(), ReportServiceError> {
    for (index, mapping) in mappings.iter().enumerate() {
        let mapping_context = format!("{context} {path}[{index}]");
        let filter_id = mapping
            .filter_id
            .as_deref()
            .map(str::trim)
            .filter(|filter_id| !filter_id.is_empty())
            .ok_or_else(|| {
                report_validation_error(
                    "$",
                    "MISSING_FILTER_MAPPING_FILTER",
                    format!("{mapping_context} must include filterId"),
                    Some(
                        "Each filterMappings entry must name the report filter it reads from."
                            .to_string(),
                    ),
                )
            })?;
        if !filter_ids.contains(filter_id) {
            return Err(report_validation_error(
                "$",
                "UNKNOWN_FILTER",
                format!(
                    "{mapping_context} references unknown filter '{}'",
                    filter_id
                ),
                Some("Declare the filter in definition.filters before referencing it.".to_string()),
            ));
        }
        if mapping.field.trim().is_empty() {
            return Err(report_validation_error(
                "$",
                "MISSING_FILTER_MAPPING_FIELD",
                format!("{mapping_context} field must not be empty"),
                Some(
                    "Set field to a source field that should receive the filter value.".to_string(),
                ),
            ));
        }
        if !is_known_field(&mapping.field) {
            return Err(report_validation_error(
                "$",
                "UNKNOWN_FILTER_MAPPING_FIELD",
                format!(
                    "{mapping_context} references unknown field '{}'",
                    mapping.field
                ),
                Some(
                    "Use a field available from the target source schema or virtual entity."
                        .to_string(),
                ),
            ));
        }
        if !is_known_filter_mapping_op(&mapping.op) {
            return Err(report_validation_error(
                "$",
                "UNSUPPORTED_FILTER_MAPPING_OPERATOR",
                format!("{mapping_context} uses unsupported op '{}'", mapping.op),
                Some("Supported filter mapping operators are eq, ne, gt, gte, lt, lte, in, between, contains, and search.".to_string()),
            ));
        }
    }
    Ok(())
}

fn is_known_filter_mapping_op(op: &str) -> bool {
    matches!(
        op.to_ascii_lowercase().as_str(),
        "eq" | "ne" | "gt" | "gte" | "lt" | "lte" | "in" | "between" | "contains" | "search"
    )
}

fn resolve_report_condition_values(
    condition: &Condition,
    filter_defs: &HashMap<String, &ReportFilterDefinition>,
    values: &HashMap<String, Value>,
    context: &str,
) -> Result<Option<Condition>, ReportServiceError> {
    let Some(arguments) = condition.arguments.as_ref() else {
        return Ok(Some(condition.clone()));
    };
    let op = condition.op.to_ascii_uppercase();

    if op == "AND" || op == "OR" {
        return resolve_logical_report_condition(condition, filter_defs, values, context);
    }

    if op == "NOT" {
        let mut resolved_arguments = Vec::with_capacity(arguments.len());
        for argument in arguments {
            let Some(resolved) =
                resolve_report_condition_argument(argument, filter_defs, values, context)?
            else {
                return Ok(None);
            };
            resolved_arguments.push(resolved);
        }
        return Ok(Some(Condition {
            op: condition.op.clone(),
            arguments: Some(resolved_arguments),
        }));
    }

    let mut resolved_arguments = Vec::with_capacity(arguments.len());
    for argument in arguments {
        let Some(resolved) =
            resolve_report_condition_argument(argument, filter_defs, values, context)?
        else {
            return Ok(None);
        };
        resolved_arguments.push(resolved);
    }

    if matches!(op.as_str(), "IN" | "NOT_IN")
        && resolved_arguments.get(1).is_some_and(|argument| {
            !argument.is_array() && !is_report_condition_subquery_operand(argument)
        })
    {
        return Err(ReportServiceError::Validation(format!(
            "{} condition operator '{}' requires an array value; use a multi_select filter with path 'values'",
            context, condition.op
        )));
    }

    Ok(Some(Condition {
        op: condition.op.clone(),
        arguments: Some(resolved_arguments),
    }))
}

fn resolve_optional_report_condition(
    condition: Option<&Condition>,
    filter_defs: &HashMap<String, &ReportFilterDefinition>,
    values: &HashMap<String, Value>,
    context: &str,
) -> Result<Option<Condition>, ReportServiceError> {
    condition
        .map(|condition| resolve_report_condition_values(condition, filter_defs, values, context))
        .transpose()
        .map(Option::flatten)
}

fn resolve_logical_report_condition(
    condition: &Condition,
    filter_defs: &HashMap<String, &ReportFilterDefinition>,
    values: &HashMap<String, Value>,
    context: &str,
) -> Result<Option<Condition>, ReportServiceError> {
    let mut resolved_arguments = Vec::new();
    let Some(arguments) = condition.arguments.as_ref() else {
        return Ok(Some(condition.clone()));
    };

    for argument in arguments {
        let Some(resolved) =
            resolve_report_condition_argument(argument, filter_defs, values, context)?
        else {
            continue;
        };
        resolved_arguments.push(resolved);
    }

    match resolved_arguments.len() {
        0 => Ok(None),
        1 => {
            let only = resolved_arguments.remove(0);
            if let Some(condition) = condition_from_value(&only) {
                Ok(Some(condition))
            } else {
                Ok(Some(Condition {
                    op: condition.op.clone(),
                    arguments: Some(vec![only]),
                }))
            }
        }
        _ => Ok(Some(Condition {
            op: condition.op.clone(),
            arguments: Some(resolved_arguments),
        })),
    }
}

fn resolve_report_condition_argument(
    argument: &Value,
    filter_defs: &HashMap<String, &ReportFilterDefinition>,
    values: &HashMap<String, Value>,
    context: &str,
) -> Result<Option<Value>, ReportServiceError> {
    if let Some(reference) = parse_report_condition_filter_ref(argument)? {
        return resolve_report_condition_filter_ref(&reference, filter_defs, values, context);
    }

    if parse_report_condition_subquery_operand(argument)?.is_some() {
        return resolve_report_condition_subquery_argument(argument, filter_defs, values, context);
    }

    if let Some(condition) = condition_from_value(argument) {
        let Some(resolved) =
            resolve_report_condition_values(&condition, filter_defs, values, context)?
        else {
            return Ok(None);
        };
        return serde_json::to_value(resolved).map(Some).map_err(|err| {
            ReportServiceError::Validation(format!(
                "{} condition could not serialize resolved child condition: {}",
                context, err
            ))
        });
    }

    Ok(Some(argument.clone()))
}

fn resolve_report_condition_subquery_argument(
    argument: &Value,
    filter_defs: &HashMap<String, &ReportFilterDefinition>,
    values: &HashMap<String, Value>,
    context: &str,
) -> Result<Option<Value>, ReportServiceError> {
    let Some(subquery) = parse_report_condition_subquery_operand(argument)? else {
        return Ok(Some(argument.clone()));
    };
    let mut subquery = subquery.clone();
    if let Some(condition) = subquery.get("condition").cloned() {
        let condition = serde_json::from_value::<Condition>(condition).map_err(|err| {
            ReportServiceError::Validation(format!(
                "{} condition subquery.condition is invalid: {}",
                context, err
            ))
        })?;
        match resolve_report_condition_values(&condition, filter_defs, values, context)? {
            Some(resolved) => {
                let resolved_value = serde_json::to_value(resolved).map_err(|err| {
                    ReportServiceError::Validation(format!(
                        "{} condition could not serialize resolved subquery condition: {}",
                        context, err
                    ))
                })?;
                subquery.insert("condition".to_string(), resolved_value);
            }
            None => {
                subquery.remove("condition");
            }
        }
    }

    Ok(Some(json!({ "subquery": Value::Object(subquery) })))
}

fn resolve_report_condition_filter_ref(
    reference: &ReportConditionFilterRef,
    filter_defs: &HashMap<String, &ReportFilterDefinition>,
    values: &HashMap<String, Value>,
    context: &str,
) -> Result<Option<Value>, ReportServiceError> {
    let filter = filter_defs.get(&reference.filter_id).ok_or_else(|| {
        ReportServiceError::Validation(format!(
            "{} condition references unknown filter '{}'",
            context, reference.filter_id
        ))
    })?;
    validate_condition_filter_ref_path(filter, reference, context)?;

    let raw = values
        .get(&reference.filter_id)
        .cloned()
        .or_else(|| {
            filter
                .default
                .clone()
                .map(|default| resolve_filter_value(filter, default))
        })
        .unwrap_or(Value::Null);
    let value = extract_condition_filter_ref_value(&raw, &reference.path);

    if value_is_empty(&value) {
        if filter.required {
            return Err(ReportServiceError::Validation(format!(
                "{} condition references required filter '{}' path '{}' but no value was provided",
                context, reference.filter_id, reference.path
            )));
        }
        return Ok(None);
    }

    Ok(Some(value))
}

fn extract_condition_filter_ref_value(value: &Value, path: &str) -> Value {
    match path {
        "value" => value
            .as_object()
            .and_then(|object| object.get("value"))
            .cloned()
            .unwrap_or_else(|| value.clone()),
        "values" => {
            if value.is_array() {
                value.clone()
            } else {
                value
                    .as_object()
                    .and_then(|object| object.get("values"))
                    .cloned()
                    .unwrap_or(Value::Null)
            }
        }
        "from" | "to" | "min" | "max" => value
            .as_object()
            .and_then(|object| object.get(path))
            .cloned()
            .unwrap_or(Value::Null),
        _ => Value::Null,
    }
}

fn parse_report_condition_filter_ref(
    value: &Value,
) -> Result<Option<ReportConditionFilterRef>, ReportServiceError> {
    let Some(object) = value.as_object() else {
        return Ok(None);
    };
    let Some(filter_value) = object.get("filter") else {
        return Ok(None);
    };
    let Some(filter_id) = filter_value.as_str().map(str::trim) else {
        return Err(ReportServiceError::Validation(
            "Report condition filter refs must include a string filter".to_string(),
        ));
    };
    if filter_id.is_empty() {
        return Err(ReportServiceError::Validation(
            "Report condition filter refs must include a non-empty filter".to_string(),
        ));
    }
    let Some(path) = object.get("path").and_then(Value::as_str).map(str::trim) else {
        return Err(ReportServiceError::Validation(format!(
            "Report condition filter ref '{}' must include path",
            filter_id
        )));
    };
    if !is_known_condition_filter_ref_path(path) {
        return Err(ReportServiceError::Validation(format!(
            "Report condition filter ref '{}' uses unsupported path '{}'",
            filter_id, path
        )));
    }
    Ok(Some(ReportConditionFilterRef {
        filter_id: filter_id.to_string(),
        path: path.to_string(),
    }))
}

fn parse_report_condition_subquery_operand(
    value: &Value,
) -> Result<Option<&serde_json::Map<String, Value>>, ReportServiceError> {
    let Some(object) = value.as_object() else {
        return Ok(None);
    };
    let Some(subquery_value) = object.get("subquery") else {
        return Ok(None);
    };
    if object.len() != 1 {
        return Err(ReportServiceError::Validation(
            "Report condition subquery operands must contain only the 'subquery' key".to_string(),
        ));
    }
    let Some(subquery) = subquery_value.as_object() else {
        return Err(ReportServiceError::Validation(
            "Report condition subquery operands must use an object value".to_string(),
        ));
    };
    for key in subquery.keys() {
        if !matches!(
            key.as_str(),
            "schema" | "select" | "condition" | "connectionId"
        ) {
            return Err(ReportServiceError::Validation(format!(
                "Report condition subquery uses unsupported key '{}'",
                key
            )));
        }
    }
    let Some(schema) = subquery
        .get("schema")
        .and_then(Value::as_str)
        .map(str::trim)
    else {
        return Err(ReportServiceError::Validation(
            "Report condition subqueries must include schema".to_string(),
        ));
    };
    if schema.is_empty() {
        return Err(ReportServiceError::Validation(
            "Report condition subquery schema cannot be empty".to_string(),
        ));
    }
    let Some(select) = subquery
        .get("select")
        .and_then(Value::as_str)
        .map(str::trim)
    else {
        return Err(ReportServiceError::Validation(
            "Report condition subqueries must include select".to_string(),
        ));
    };
    if select.is_empty() {
        return Err(ReportServiceError::Validation(
            "Report condition subquery select cannot be empty".to_string(),
        ));
    }
    if let Some(condition) = subquery.get("condition")
        && condition_from_value(condition).is_none()
    {
        return Err(ReportServiceError::Validation(
            "Report condition subquery.condition must be a condition object".to_string(),
        ));
    }
    Ok(Some(subquery))
}

fn is_report_condition_subquery_operand(value: &Value) -> bool {
    value
        .as_object()
        .is_some_and(|object| object.contains_key("subquery"))
}

fn validate_condition_filter_ref_path(
    filter: &ReportFilterDefinition,
    reference: &ReportConditionFilterRef,
    context: &str,
) -> Result<(), ReportServiceError> {
    let allowed_paths = condition_filter_ref_paths_for_type(&filter.filter_type);
    if allowed_paths.contains(&reference.path.as_str()) {
        return Ok(());
    }

    Err(ReportServiceError::Validation(format!(
        "{} condition references filter '{}' path '{}' but {} filters support {}",
        context,
        reference.filter_id,
        reference.path,
        report_filter_type_name(&filter.filter_type),
        allowed_paths.join(", ")
    )))
}

fn condition_filter_ref_paths_for_type(filter_type: &ReportFilterType) -> &'static [&'static str] {
    match filter_type {
        ReportFilterType::MultiSelect => &["values"],
        ReportFilterType::TimeRange => &["from", "to"],
        ReportFilterType::NumberRange => &["min", "max"],
        ReportFilterType::Select
        | ReportFilterType::Radio
        | ReportFilterType::Checkbox
        | ReportFilterType::Text
        | ReportFilterType::Search => &["value"],
    }
}

fn report_filter_type_name(filter_type: &ReportFilterType) -> &'static str {
    match filter_type {
        ReportFilterType::Select => "select",
        ReportFilterType::MultiSelect => "multi_select",
        ReportFilterType::Radio => "radio",
        ReportFilterType::Checkbox => "checkbox",
        ReportFilterType::TimeRange => "time_range",
        ReportFilterType::NumberRange => "number_range",
        ReportFilterType::Text => "text",
        ReportFilterType::Search => "search",
    }
}

fn is_known_condition_filter_ref_path(path: &str) -> bool {
    matches!(path, "value" | "values" | "from" | "to" | "min" | "max")
}

fn condition_from_value(value: &Value) -> Option<Condition> {
    let object = value.as_object()?;
    if !(object.contains_key("op") || object.contains_key("arguments")) {
        return None;
    }
    serde_json::from_value(value.clone()).ok()
}

fn static_filter_options_response(
    filter: &ReportFilterDefinition,
    offset: i64,
    limit: i64,
    query: &Option<String>,
) -> ReportFilterOptionsResponse {
    let query = query.as_deref().map(str::trim).unwrap_or("").to_lowercase();
    let values = filter
        .options
        .as_ref()
        .and_then(Value::as_object)
        .and_then(|options| options.get("values"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut options = values
        .into_iter()
        .filter_map(static_filter_option)
        .filter(|option| {
            query.is_empty()
                || option.label.to_lowercase().contains(&query)
                || option.value.to_string().to_lowercase().contains(&query)
        })
        .collect::<Vec<_>>();
    let total_count = options.len() as i64;
    let limit = limit.clamp(1, MAX_TABLE_PAGE_SIZE);
    let offset = offset.max(0);
    options = options
        .into_iter()
        .skip(offset as usize)
        .take(limit as usize)
        .collect();

    ReportFilterOptionsResponse {
        success: true,
        filter: ReportFilterOptionsMetadata {
            id: filter.id.clone(),
        },
        options,
        page: ReportFilterOptionsPage {
            offset,
            size: limit,
            total_count,
            has_next_page: offset + limit < total_count,
        },
    }
}

fn static_filter_option(value: Value) -> Option<ReportFilterOption> {
    match value {
        Value::Object(object) => {
            let value = object.get("value").cloned()?;
            let label = object
                .get("label")
                .map(filter_option_label)
                .unwrap_or_else(|| filter_option_label(&value));
            Some(ReportFilterOption {
                label,
                value,
                count: None,
            })
        }
        value => Some(ReportFilterOption {
            label: filter_option_label(&value),
            value,
            count: None,
        }),
    }
}

fn filter_option_label(value: &Value) -> String {
    match value {
        Value::Null => "(blank)".to_string(),
        Value::String(value) => value.clone(),
        _ => value.to_string(),
    }
}

fn option_string<'a>(options: &'a serde_json::Map<String, Value>, key: &str) -> Option<&'a str> {
    options
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn option_bool(options: &serde_json::Map<String, Value>, key: &str) -> Option<bool> {
    options.get(key).and_then(Value::as_bool)
}

fn option_string_set(
    options: &serde_json::Map<String, Value>,
    key: &str,
) -> Option<HashSet<String>> {
    let values = options
        .get(key)
        .and_then(Value::as_array)?
        .iter()
        .filter_map(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .collect::<HashSet<_>>();
    Some(values)
}

fn append_option_context_conditions(
    conditions: &mut Vec<Condition>,
    definition: &ReportDefinition,
    current_filter: &ReportFilterDefinition,
    options: &serde_json::Map<String, Value>,
    resolved_filters: &HashMap<String, Value>,
) {
    let depends_on = option_string_set(options, "dependsOn");
    for filter in &definition.filters {
        if filter.id == current_filter.id {
            continue;
        }
        if let Some(depends_on) = &depends_on
            && !depends_on.contains(&filter.id)
        {
            continue;
        }
        let Some(value) = resolved_filters.get(&filter.id) else {
            continue;
        };
        for target in &filter.applies_to {
            if target.block_id.is_some() {
                continue;
            }
            if let Some(condition) = condition_from_filter_target(target, value) {
                conditions.push(condition);
            }
        }
    }

    if let Some(mappings) = options.get("filterMappings")
        && let Ok(mappings) = serde_json::from_value::<Vec<ReportFilterTarget>>(mappings.clone())
    {
        append_source_mapping_conditions(conditions, &mappings, resolved_filters);
    }
}

/// Returns the id of the first filter that
///   (a) is referenced by the block's source `condition` (or any column-level
///       lookup source), and
///   (b) has `strict_when_referenced: true`, and
///   (c) has no value in `resolved_filters` (and no default that would supply
///       one).
/// `None` means the block can be rendered normally.
fn block_unsatisfied_strict_filter(
    definition: &ReportDefinition,
    block: &ReportBlockDefinition,
    resolved_filters: &HashMap<String, Value>,
) -> Option<String> {
    let strict_filters: HashMap<&str, &ReportFilterDefinition> = definition
        .filters
        .iter()
        .filter(|filter| filter.strict_when_referenced)
        .map(|filter| (filter.id.as_str(), filter))
        .collect();
    if strict_filters.is_empty() {
        return None;
    }

    let mut conditions: Vec<&Condition> = Vec::new();
    if let Some(condition) = block.source.condition.as_ref() {
        conditions.push(condition);
    }
    if let Some(table) = block.table.as_ref() {
        for column in &table.columns {
            if let Some(source) = column.source.as_ref()
                && let Some(condition) = source.condition.as_ref()
            {
                conditions.push(condition);
            }
        }
    }

    for condition in conditions {
        if let Some(unset) =
            condition_unsatisfied_strict_filter(condition, &strict_filters, resolved_filters)
        {
            return Some(unset);
        }
    }
    None
}

fn condition_unsatisfied_strict_filter(
    condition: &Condition,
    strict_filters: &HashMap<&str, &ReportFilterDefinition>,
    resolved_filters: &HashMap<String, Value>,
) -> Option<String> {
    let arguments = condition.arguments.as_ref()?;
    for argument in arguments {
        if let Ok(Some(reference)) = parse_report_condition_filter_ref(argument)
            && let Some(filter) = strict_filters.get(reference.filter_id.as_str())
        {
            let raw = resolved_filters
                .get(&reference.filter_id)
                .cloned()
                .or_else(|| {
                    filter
                        .default
                        .clone()
                        .map(|default| resolve_filter_value(filter, default))
                })
                .unwrap_or(Value::Null);
            let value = extract_condition_filter_ref_value(&raw, &reference.path);
            if value_is_empty(&value) {
                return Some(reference.filter_id);
            }
        }
        if let Some(child) = condition_from_value(argument)
            && let Some(unset) =
                condition_unsatisfied_strict_filter(&child, strict_filters, resolved_filters)
        {
            return Some(unset);
        }
    }
    None
}

fn build_block_condition(
    definition: &ReportDefinition,
    block: &ReportBlockDefinition,
    resolved_filters: &HashMap<String, Value>,
    block_request: Option<&ReportBlockDataRequest>,
) -> Result<Option<Condition>, ReportServiceError> {
    let mut conditions = Vec::new();
    let condition_filter_defs = block_condition_filter_definitions(definition, block);
    let condition_filter_values =
        block_condition_filter_values(block, resolved_filters, block_request);

    let context = format!("block '{}'", block.id);
    if let Some(condition) = resolve_optional_report_condition(
        block.source.condition.as_ref(),
        &condition_filter_defs,
        &condition_filter_values,
        &context,
    )? {
        conditions.push(condition);
    }

    append_filter_conditions(
        &mut conditions,
        &definition.filters,
        &block.id,
        resolved_filters,
    );

    append_source_mapping_conditions(
        &mut conditions,
        &block.source.filter_mappings,
        resolved_filters,
    );

    if let Some(block_request) = block_request {
        append_filter_conditions(
            &mut conditions,
            &block.filters,
            &block.id,
            &block_request.block_filters,
        );
        append_table_search_condition(&mut conditions, block, block_request);
    }

    Ok(combine_conditions(conditions))
}

fn append_table_search_condition(
    conditions: &mut Vec<Condition>,
    block: &ReportBlockDefinition,
    block_request: &ReportBlockDataRequest,
) {
    let Some(search) = &block_request.search else {
        return;
    };
    let query = search.query.trim();
    if query.is_empty() {
        return;
    }

    let mut search_conditions = searchable_table_fields(block, search)
        .into_iter()
        .map(|field| {
            binary_condition(
                "CONTAINS",
                Value::String(field),
                Value::String(query.to_string()),
            )
        })
        .collect::<Vec<_>>();

    match search_conditions.len() {
        0 => {}
        1 => conditions.push(search_conditions.remove(0)),
        _ => conditions.push(Condition {
            op: "OR".to_string(),
            arguments: Some(
                search_conditions
                    .into_iter()
                    .filter_map(|condition| serde_json::to_value(condition).ok())
                    .collect(),
            ),
        }),
    }
}

fn searchable_table_fields(
    block: &ReportBlockDefinition,
    search: &ReportTableSearchRequest,
) -> Vec<String> {
    let requested_fields = search
        .fields
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    let mut seen = HashSet::new();

    if block.source.mode == ReportSourceMode::Aggregate {
        return block
            .source
            .group_by
            .iter()
            .filter(|field| {
                requested_fields.is_empty() || requested_fields.contains(field.as_str())
            })
            .filter_map(|field| {
                if seen.insert(field.clone()) {
                    Some(field.clone())
                } else {
                    None
                }
            })
            .collect();
    }

    let Some(table) = block.table.as_ref() else {
        return Vec::new();
    };

    table
        .columns
        .iter()
        .filter(|column| !column.is_chart())
        .flat_map(|column| {
            std::iter::once(column.field.as_str()).chain(column.display_field.as_deref())
        })
        .filter(|field| requested_fields.is_empty() || requested_fields.contains(*field))
        .filter_map(|field| {
            if seen.insert(field.to_string()) {
                Some(field.to_string())
            } else {
                None
            }
        })
        .collect()
}

fn append_filter_conditions(
    conditions: &mut Vec<Condition>,
    filters: &[ReportFilterDefinition],
    block_id: &str,
    values: &HashMap<String, Value>,
) {
    for filter in filters {
        let Some(value) = values.get(&filter.id) else {
            continue;
        };
        for target in &filter.applies_to {
            if target
                .block_id
                .as_deref()
                .is_some_and(|target_block_id| target_block_id != block_id)
            {
                continue;
            }
            if let Some(condition) = condition_from_filter_target(target, value) {
                conditions.push(condition);
            }
        }
    }
}

fn append_source_mapping_conditions(
    conditions: &mut Vec<Condition>,
    mappings: &[ReportFilterTarget],
    values: &HashMap<String, Value>,
) {
    for mapping in mappings {
        let Some(filter_id) = mapping.filter_id.as_deref() else {
            continue;
        };
        let Some(value) = values.get(filter_id) else {
            continue;
        };
        if let Some(condition) = condition_from_filter_target(mapping, value) {
            conditions.push(condition);
        }
    }
}

fn condition_from_filter_target(target: &ReportFilterTarget, value: &Value) -> Option<Condition> {
    if value_is_empty(value) {
        return None;
    }

    let field = Value::String(target.field.clone());
    match target.op.to_ascii_lowercase().as_str() {
        "between" => between_condition(&target.field, value),
        "in" => Some(Condition {
            op: "IN".to_string(),
            arguments: Some(vec![field, ensure_array(value)]),
        }),
        "ne" => Some(binary_condition("NE", field, value.clone())),
        "gt" => Some(binary_condition("GT", field, value.clone())),
        "gte" => Some(binary_condition("GTE", field, value.clone())),
        "lt" => Some(binary_condition("LT", field, value.clone())),
        "lte" => Some(binary_condition("LTE", field, value.clone())),
        "contains" | "search" => Some(binary_condition("CONTAINS", field, value.clone())),
        _ => Some(binary_condition("EQ", field, value.clone())),
    }
}

fn binary_condition(op: &str, field: Value, value: Value) -> Condition {
    Condition {
        op: op.to_string(),
        arguments: Some(vec![field, value]),
    }
}

fn option_search_condition(
    schema: &ObjectSchema,
    fallback_fields: &[String],
    search_query: &str,
) -> Option<Condition> {
    let search_query = search_query.trim();
    if search_query.is_empty() || fallback_fields.is_empty() {
        return None;
    }

    let tsvector_fields = schema
        .columns
        .iter()
        .filter_map(|column| match &column.column_type {
            ObjectColumnType::Tsvector { .. } => Some(column.name.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();

    let (op, fields) = if tsvector_fields.is_empty() {
        ("CONTAINS", fallback_fields.to_vec())
    } else {
        ("MATCH", tsvector_fields)
    };

    let mut seen = HashSet::new();
    let mut search_conditions = fields
        .into_iter()
        .filter(|field| seen.insert(field.clone()))
        .map(|field| {
            binary_condition(
                op,
                Value::String(field),
                Value::String(search_query.to_string()),
            )
        })
        .collect::<Vec<_>>();

    match search_conditions.len() {
        0 => None,
        1 => Some(search_conditions.remove(0)),
        _ => Some(Condition {
            op: "OR".to_string(),
            arguments: Some(
                search_conditions
                    .into_iter()
                    .filter_map(|condition| serde_json::to_value(condition).ok())
                    .collect(),
            ),
        }),
    }
}

fn between_condition(field: &str, value: &Value) -> Option<Condition> {
    let (from, to) = if let Some(object) = value.as_object() {
        (
            object
                .get("from")
                .or_else(|| object.get("min"))
                .cloned()
                .unwrap_or(Value::Null),
            object
                .get("to")
                .or_else(|| object.get("max"))
                .cloned()
                .unwrap_or(Value::Null),
        )
    } else if let Some(array) = value.as_array() {
        (
            array.first().cloned().unwrap_or(Value::Null),
            array.get(1).cloned().unwrap_or(Value::Null),
        )
    } else {
        return None;
    };

    let mut conditions = Vec::new();
    if !value_is_empty(&from) {
        conditions.push(binary_condition(
            "GTE",
            Value::String(field.to_string()),
            from,
        ));
    }
    if !value_is_empty(&to) {
        conditions.push(binary_condition("LT", Value::String(field.to_string()), to));
    }

    combine_conditions(conditions)
}

fn combine_conditions(conditions: Vec<Condition>) -> Option<Condition> {
    match conditions.len() {
        0 => None,
        1 => conditions.into_iter().next(),
        _ => Some(Condition {
            op: "AND".to_string(),
            arguments: Some(
                conditions
                    .into_iter()
                    .filter_map(|condition| serde_json::to_value(condition).ok())
                    .collect(),
            ),
        }),
    }
}

fn workflow_runtime_entity(
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

fn system_entity(
    block: &ReportBlockDefinition,
) -> Result<ReportWorkflowRuntimeEntity, ReportServiceError> {
    let entity = block.source.entity.ok_or_else(|| {
        ReportServiceError::Validation(format!(
            "Block '{}' system source must specify entity",
            block.id
        ))
    })?;
    match entity {
        ReportWorkflowRuntimeEntity::RuntimeExecutionMetricBuckets
        | ReportWorkflowRuntimeEntity::RuntimeSystemSnapshot
        | ReportWorkflowRuntimeEntity::ConnectionRateLimitStatus
        | ReportWorkflowRuntimeEntity::ConnectionRateLimitEvents
        | ReportWorkflowRuntimeEntity::ConnectionRateLimitTimeline => Ok(entity),
        ReportWorkflowRuntimeEntity::Instances | ReportWorkflowRuntimeEntity::Actions => {
            Err(ReportServiceError::Validation(format!(
                "Block '{}' system source does not support workflow_runtime entity {:?}",
                block.id, entity
            )))
        }
    }
}

fn workflow_runtime_workflow_id(block: &ReportBlockDefinition) -> Result<&str, ReportServiceError> {
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

fn should_check_instance_actions(instance: &WorkflowInstanceDto) -> bool {
    !instance.status.is_terminal() && instance.has_pending_input
}

fn workflow_instance_report_row(
    instance: &WorkflowInstanceDto,
    actions: &[WorkflowRuntimeAction],
) -> serde_json::Map<String, Value> {
    let mut row = serde_json::Map::new();
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
    row.insert("status".to_string(), json!(instance.status));
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

fn workflow_action_report_row(action: &WorkflowRuntimeAction) -> serde_json::Map<String, Value> {
    let mut row = serde_json::Map::new();
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

fn workflow_runtime_table_columns(
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

fn system_fields(entity: ReportWorkflowRuntimeEntity) -> HashSet<&'static str> {
    match entity {
        ReportWorkflowRuntimeEntity::RuntimeExecutionMetricBuckets => [
            "tenantId",
            "bucketTime",
            "granularity",
            "invocationCount",
            "successCount",
            "failureCount",
            "cancelledCount",
            "avgDurationSeconds",
            "minDurationSeconds",
            "maxDurationSeconds",
            "avgMemoryBytes",
            "maxMemoryBytes",
            "successRatePercent",
        ]
        .into_iter()
        .collect(),
        ReportWorkflowRuntimeEntity::RuntimeSystemSnapshot => [
            "capturedAt",
            "cpuArchitecture",
            "cpuPhysicalCores",
            "cpuLogicalCores",
            "memoryTotalBytes",
            "memoryAvailableBytes",
            "memoryAvailableForWorkflowsBytes",
            "memoryUsedBytes",
            "memoryUsedPercent",
            "diskPath",
            "diskTotalBytes",
            "diskAvailableBytes",
            "diskUsedBytes",
            "diskUsedPercent",
        ]
        .into_iter()
        .collect(),
        ReportWorkflowRuntimeEntity::ConnectionRateLimitStatus => [
            "connectionId",
            "connectionTitle",
            "integrationId",
            "config",
            "state",
            "metrics",
            "periodStats",
            "configRequestsPerSecond",
            "configBurstSize",
            "configRetryOnLimit",
            "configMaxRetries",
            "configMaxWaitMs",
            "stateAvailable",
            "stateCurrentTokens",
            "stateLastRefillMs",
            "stateLearnedLimit",
            "stateCallsInWindow",
            "stateTotalCalls",
            "stateWindowStartMs",
            "capacityPercent",
            "utilizationPercent",
            "isRateLimited",
            "retryAfterMs",
            "periodInterval",
            "periodTotalRequests",
            "periodRateLimitedCount",
            "periodRetryCount",
            "periodRateLimitedPercent",
        ]
        .into_iter()
        .collect(),
        ReportWorkflowRuntimeEntity::ConnectionRateLimitEvents => [
            "id",
            "connectionId",
            "eventType",
            "createdAt",
            "metadata",
            "tag",
        ]
        .into_iter()
        .collect(),
        ReportWorkflowRuntimeEntity::ConnectionRateLimitTimeline => [
            "connectionId",
            "bucket",
            "bucketTime",
            "granularity",
            "requestCount",
            "rateLimitedCount",
            "retryCount",
        ]
        .into_iter()
        .collect(),
        ReportWorkflowRuntimeEntity::Instances | ReportWorkflowRuntimeEntity::Actions => {
            HashSet::new()
        }
    }
}

fn system_row_field_known(fields: &HashSet<&'static str>, field: &str) -> bool {
    fields.contains(field)
        || field
            .split_once('.')
            .is_some_and(|(root, _)| fields.contains(root))
}

fn system_table_columns(
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
        ReportWorkflowRuntimeEntity::RuntimeExecutionMetricBuckets => &[
            ("bucketTime", "Bucket", Some("datetime")),
            ("invocationCount", "Invocations", Some("number")),
            ("successCount", "Successes", Some("number")),
            ("failureCount", "Failures", Some("number")),
            ("cancelledCount", "Cancelled", Some("number")),
            ("successRatePercent", "Success Rate", Some("percent")),
            ("avgDurationSeconds", "Avg Duration", Some("number")),
            ("maxMemoryBytes", "Max Memory", Some("bytes")),
        ],
        ReportWorkflowRuntimeEntity::RuntimeSystemSnapshot => &[
            ("capturedAt", "Captured", Some("datetime")),
            ("cpuArchitecture", "CPU Architecture", None),
            ("cpuPhysicalCores", "Physical Cores", Some("number")),
            ("cpuLogicalCores", "Logical Cores", Some("number")),
            ("memoryUsedPercent", "Memory Used", Some("percent")),
            ("diskUsedPercent", "Disk Used", Some("percent")),
        ],
        ReportWorkflowRuntimeEntity::ConnectionRateLimitStatus => &[
            ("connectionTitle", "Connection", None),
            ("integrationId", "Integration", None),
            ("isRateLimited", "Rate Limited", Some("boolean")),
            ("capacityPercent", "Capacity", Some("percent")),
            ("utilizationPercent", "Utilization", Some("percent")),
            ("periodTotalRequests", "Requests", Some("number")),
            ("periodRateLimitedCount", "Limited", Some("number")),
            ("periodRetryCount", "Retries", Some("number")),
        ],
        ReportWorkflowRuntimeEntity::ConnectionRateLimitEvents => &[
            ("createdAt", "Created", Some("datetime")),
            ("connectionId", "Connection", None),
            ("eventType", "Event", None),
            ("tag", "Tag", None),
        ],
        ReportWorkflowRuntimeEntity::ConnectionRateLimitTimeline => &[
            ("bucketTime", "Bucket", Some("datetime")),
            ("requestCount", "Requests", Some("number")),
            ("rateLimitedCount", "Limited", Some("number")),
            ("retryCount", "Retries", Some("number")),
        ],
        ReportWorkflowRuntimeEntity::Instances | ReportWorkflowRuntimeEntity::Actions => &[],
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

fn option_f64_value(value: Option<f64>) -> Value {
    value
        .and_then(serde_json::Number::from_f64)
        .map(Value::Number)
        .unwrap_or(Value::Null)
}

fn f64_value(value: f64) -> Value {
    serde_json::Number::from_f64(value)
        .map(Value::Number)
        .unwrap_or(Value::Null)
}

fn runtime_system_snapshot_row() -> serde_json::Map<String, Value> {
    let mut sys = sysinfo::System::new();
    sys.refresh_memory();

    let total_memory = sys.total_memory();
    let available_memory = sys.available_memory();
    let available_for_workflows = (available_memory as f64 * 0.8) as u64;
    let used_memory = total_memory.saturating_sub(available_memory);
    let memory_used_percent = percent(used_memory as f64, total_memory as f64);

    let data_dir = std::env::var("DATA_DIR").unwrap_or_else(|_| ".data".to_string());
    let data_path = PathBuf::from(&data_dir);
    let canonical_path = data_path.canonicalize().unwrap_or(data_path);
    let disks = sysinfo::Disks::new_with_refreshed_list();
    let disk_info = disks
        .iter()
        .filter(|disk| canonical_path.starts_with(disk.mount_point()))
        .max_by_key(|disk| disk.mount_point().as_os_str().len())
        .map(|disk| {
            (
                disk.total_space(),
                disk.available_space(),
                canonical_path.display().to_string(),
            )
        })
        .or_else(|| {
            disks
                .iter()
                .next()
                .map(|disk| (disk.total_space(), disk.available_space(), data_dir.clone()))
        })
        .unwrap_or((0, 0, data_dir));
    let disk_used = disk_info.0.saturating_sub(disk_info.1);
    let disk_used_percent = percent(disk_used as f64, disk_info.0 as f64);

    serde_json::Map::from_iter([
        (
            "capturedAt".to_string(),
            Value::String(Utc::now().to_rfc3339()),
        ),
        (
            "cpuArchitecture".to_string(),
            Value::String(std::env::consts::ARCH.to_string()),
        ),
        (
            "cpuPhysicalCores".to_string(),
            json!(num_cpus::get_physical()),
        ),
        ("cpuLogicalCores".to_string(), json!(num_cpus::get())),
        ("memoryTotalBytes".to_string(), json!(total_memory)),
        ("memoryAvailableBytes".to_string(), json!(available_memory)),
        (
            "memoryAvailableForWorkflowsBytes".to_string(),
            json!(available_for_workflows),
        ),
        ("memoryUsedBytes".to_string(), json!(used_memory)),
        (
            "memoryUsedPercent".to_string(),
            f64_value(memory_used_percent),
        ),
        ("diskPath".to_string(), Value::String(disk_info.2)),
        ("diskTotalBytes".to_string(), json!(disk_info.0)),
        ("diskAvailableBytes".to_string(), json!(disk_info.1)),
        ("diskUsedBytes".to_string(), json!(disk_used)),
        ("diskUsedPercent".to_string(), f64_value(disk_used_percent)),
    ])
}

fn percent(numerator: f64, denominator: f64) -> f64 {
    if denominator <= 0.0 {
        0.0
    } else {
        (numerator / denominator) * 100.0
    }
}

fn rate_limit_status_row(
    status: runtara_connections::types::RateLimitStatusDto,
) -> serde_json::Map<String, Value> {
    let config_value = status
        .config
        .as_ref()
        .and_then(|config| serde_json::to_value(config).ok())
        .unwrap_or(Value::Null);
    let state_value = serde_json::to_value(&status.state).unwrap_or(Value::Null);
    let metrics_value = serde_json::to_value(&status.metrics).unwrap_or(Value::Null);
    let period_value = status
        .period_stats
        .as_ref()
        .and_then(|stats| serde_json::to_value(stats).ok())
        .unwrap_or(Value::Null);

    let mut row = serde_json::Map::new();
    row.insert(
        "connectionId".to_string(),
        Value::String(status.connection_id),
    );
    row.insert(
        "connectionTitle".to_string(),
        Value::String(status.connection_title),
    );
    row.insert(
        "integrationId".to_string(),
        status
            .integration_id
            .map(Value::String)
            .unwrap_or(Value::Null),
    );
    row.insert("config".to_string(), config_value);
    row.insert("state".to_string(), state_value);
    row.insert("metrics".to_string(), metrics_value);
    row.insert("periodStats".to_string(), period_value);

    if let Some(config) = status.config {
        row.insert(
            "configRequestsPerSecond".to_string(),
            json!(config.requests_per_second),
        );
        row.insert("configBurstSize".to_string(), json!(config.burst_size));
        row.insert(
            "configRetryOnLimit".to_string(),
            Value::Bool(config.retry_on_limit),
        );
        row.insert("configMaxRetries".to_string(), json!(config.max_retries));
        row.insert("configMaxWaitMs".to_string(), json!(config.max_wait_ms));
    } else {
        row.insert("configRequestsPerSecond".to_string(), Value::Null);
        row.insert("configBurstSize".to_string(), Value::Null);
        row.insert("configRetryOnLimit".to_string(), Value::Null);
        row.insert("configMaxRetries".to_string(), Value::Null);
        row.insert("configMaxWaitMs".to_string(), Value::Null);
    }

    row.insert(
        "stateAvailable".to_string(),
        Value::Bool(status.state.available),
    );
    row.insert(
        "stateCurrentTokens".to_string(),
        option_f64_value(status.state.current_tokens),
    );
    row.insert(
        "stateLastRefillMs".to_string(),
        status
            .state
            .last_refill_ms
            .map(Value::from)
            .unwrap_or(Value::Null),
    );
    row.insert(
        "stateLearnedLimit".to_string(),
        status
            .state
            .learned_limit
            .map(Value::from)
            .unwrap_or(Value::Null),
    );
    row.insert(
        "stateCallsInWindow".to_string(),
        status
            .state
            .calls_in_window
            .map(Value::from)
            .unwrap_or(Value::Null),
    );
    row.insert(
        "stateTotalCalls".to_string(),
        status
            .state
            .total_calls
            .map(Value::from)
            .unwrap_or(Value::Null),
    );
    row.insert(
        "stateWindowStartMs".to_string(),
        status
            .state
            .window_start_ms
            .map(Value::from)
            .unwrap_or(Value::Null),
    );
    row.insert(
        "capacityPercent".to_string(),
        option_f64_value(status.metrics.capacity_percent),
    );
    row.insert(
        "utilizationPercent".to_string(),
        option_f64_value(status.metrics.utilization_percent),
    );
    row.insert(
        "isRateLimited".to_string(),
        Value::Bool(status.metrics.is_rate_limited),
    );
    row.insert(
        "retryAfterMs".to_string(),
        status
            .metrics
            .retry_after_ms
            .map(Value::from)
            .unwrap_or(Value::Null),
    );

    if let Some(period) = status.period_stats {
        row.insert("periodInterval".to_string(), Value::String(period.interval));
        row.insert(
            "periodTotalRequests".to_string(),
            json!(period.total_requests),
        );
        row.insert(
            "periodRateLimitedCount".to_string(),
            json!(period.rate_limited_count),
        );
        row.insert("periodRetryCount".to_string(), json!(period.retry_count));
        row.insert(
            "periodRateLimitedPercent".to_string(),
            f64_value(period.rate_limited_percent),
        );
    } else {
        row.insert("periodInterval".to_string(), Value::Null);
        row.insert("periodTotalRequests".to_string(), Value::Null);
        row.insert("periodRateLimitedCount".to_string(), Value::Null);
        row.insert("periodRetryCount".to_string(), Value::Null);
        row.insert("periodRateLimitedPercent".to_string(), Value::Null);
    }

    row
}

fn rate_limit_event_row(
    event: runtara_connections::types::RateLimitEventDto,
) -> serde_json::Map<String, Value> {
    let tag = event
        .metadata
        .as_ref()
        .and_then(|metadata| metadata.get("tag"))
        .cloned()
        .unwrap_or(Value::Null);
    serde_json::Map::from_iter([
        ("id".to_string(), json!(event.id)),
        (
            "connectionId".to_string(),
            Value::String(event.connection_id),
        ),
        ("eventType".to_string(), Value::String(event.event_type)),
        (
            "createdAt".to_string(),
            Value::String(event.created_at.to_rfc3339()),
        ),
        (
            "metadata".to_string(),
            event.metadata.unwrap_or(Value::Null),
        ),
        ("tag".to_string(), tag),
    ])
}

fn parse_metrics_granularity(
    granularity: Option<&str>,
) -> Result<MetricsGranularity, ReportServiceError> {
    match granularity
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("hourly")
        .to_ascii_lowercase()
        .as_str()
    {
        "hour" | "hourly" => Ok(MetricsGranularity::Hourly),
        "day" | "daily" => Ok(MetricsGranularity::Daily),
        other => Err(ReportServiceError::Validation(format!(
            "Unsupported system metrics granularity '{}'. Use hourly or daily.",
            other
        ))),
    }
}

fn infer_rate_limit_timeline_granularity(
    start_time: DateTime<Utc>,
    end_time: DateTime<Utc>,
) -> String {
    let duration = end_time - start_time;
    if duration <= Duration::hours(2) {
        "minute".to_string()
    } else if duration <= Duration::days(7) {
        "hourly".to_string()
    } else {
        "daily".to_string()
    }
}

fn map_rate_limit_error(
    error: runtara_connections::service::rate_limits::ServiceError,
) -> ReportServiceError {
    match error {
        runtara_connections::service::rate_limits::ServiceError::NotFound(message) => {
            ReportServiceError::Validation(message)
        }
        runtara_connections::service::rate_limits::ServiceError::DatabaseError(message)
        | runtara_connections::service::rate_limits::ServiceError::RedisError(message) => {
            ReportServiceError::Database(message)
        }
    }
}

fn system_connection_id(
    block: &ReportBlockDefinition,
    condition: Option<&Condition>,
) -> Option<String> {
    block
        .source
        .connection_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .or_else(|| extract_eq_string_condition(condition, "connectionId"))
}

fn extract_eq_string_condition(condition: Option<&Condition>, field: &str) -> Option<String> {
    let condition = condition?;
    let op = condition.op.to_ascii_uppercase();
    let args = condition.arguments.as_deref().unwrap_or(&[]);
    if matches!(op.as_str(), "AND" | "OR") {
        return args.iter().find_map(|argument| {
            condition_from_value(argument)
                .as_ref()
                .and_then(|child| extract_eq_string_condition(Some(child), field))
        });
    }
    if op == "EQ" && args.len() == 2 && args.first().and_then(Value::as_str) == Some(field) {
        return args.get(1).and_then(condition_scalar_to_string);
    }
    if op == "IN" && args.len() == 2 && args.first().and_then(Value::as_str) == Some(field) {
        return args
            .get(1)
            .and_then(Value::as_array)
            .and_then(|values| values.iter().find_map(condition_scalar_to_string));
    }
    None
}

fn condition_scalar_to_string(value: &Value) -> Option<String> {
    match value {
        Value::String(value) => Some(value.clone()),
        Value::Number(value) => Some(value.to_string()),
        Value::Bool(value) => Some(value.to_string()),
        _ => None,
    }
}

fn extract_time_bounds(
    condition: Option<&Condition>,
    fields: &[&str],
) -> (Option<DateTime<Utc>>, Option<DateTime<Utc>>) {
    let mut lower: Option<DateTime<Utc>> = None;
    let mut upper: Option<DateTime<Utc>> = None;
    if let Some(condition) = condition {
        collect_time_bounds(condition, fields, &mut lower, &mut upper);
    }
    (lower, upper)
}

fn collect_time_bounds(
    condition: &Condition,
    fields: &[&str],
    lower: &mut Option<DateTime<Utc>>,
    upper: &mut Option<DateTime<Utc>>,
) {
    let op = condition.op.to_ascii_uppercase();
    let args = condition.arguments.as_deref().unwrap_or(&[]);

    for argument in args {
        if let Some(child) = condition_from_value(argument) {
            collect_time_bounds(&child, fields, lower, upper);
        }
    }

    if args.len() != 2 {
        return;
    }
    let Some(field) = args.first().and_then(Value::as_str) else {
        return;
    };
    if !fields.contains(&field) {
        return;
    }
    let Some(bound) = args.get(1).and_then(parse_datetime_value) else {
        return;
    };
    match op.as_str() {
        "GT" | "GTE" => {
            if lower.is_none_or(|current| bound > current) {
                *lower = Some(bound);
            }
        }
        "LT" | "LTE" => {
            if upper.is_none_or(|current| bound < current) {
                *upper = Some(bound);
            }
        }
        _ => {}
    }
}

fn parse_datetime_value(value: &Value) -> Option<DateTime<Utc>> {
    match value {
        Value::String(value) => DateTime::parse_from_rfc3339(value)
            .ok()
            .map(|value| value.with_timezone(&Utc)),
        Value::Number(value) => value
            .as_i64()
            .and_then(DateTime::<Utc>::from_timestamp_millis),
        _ => None,
    }
}

fn aggregate_virtual_rows(
    block_id: &str,
    rows: &[serde_json::Map<String, Value>],
    request: AggregateRequest,
) -> Result<runtara_object_store::AggregateResult, ReportServiceError> {
    if request.aggregates.is_empty() {
        return Err(ReportServiceError::Validation(format!(
            "Block '{}' must define at least one aggregate",
            block_id
        )));
    }

    let row_refs = rows
        .iter()
        .filter_map(|row| match request.condition.as_ref() {
            Some(condition) => match condition_matches_row(condition, row, block_id) {
                Ok(true) => Some(Ok(row)),
                Ok(false) => None,
                Err(error) => Some(Err(error)),
            },
            None => Some(Ok(row)),
        })
        .collect::<Result<Vec<_>, _>>()?;

    let mut groups: Vec<VirtualAggregateGroup<'_>> = Vec::new();
    if request.group_by.is_empty() {
        groups.push(VirtualAggregateGroup {
            values: Vec::new(),
            rows: row_refs,
        });
    } else {
        let mut group_index_by_key = HashMap::new();
        for row in row_refs {
            let values = request
                .group_by
                .iter()
                .map(|field| {
                    virtual_row_value(row, field)
                        .cloned()
                        .unwrap_or(Value::Null)
                })
                .collect::<Vec<_>>();
            let key = value_to_lookup_key(&Value::Array(values.clone()));
            let index = if let Some(index) = group_index_by_key.get(&key) {
                *index
            } else {
                let index = groups.len();
                group_index_by_key.insert(key, index);
                groups.push(VirtualAggregateGroup {
                    values,
                    rows: Vec::new(),
                });
                index
            };
            groups[index].rows.push(row);
        }
    }

    let mut columns = request.group_by.clone();
    columns.extend(
        request
            .aggregates
            .iter()
            .map(|aggregate| aggregate.alias.clone()),
    );

    let mut output_rows = Vec::with_capacity(groups.len());
    for group in groups {
        let mut row = group.values.clone();
        let mut aliases = HashMap::new();
        for aggregate in &request.aggregates {
            let value = virtual_aggregate_value(block_id, aggregate, &group.rows, &aliases)?;
            aliases.insert(aggregate.alias.clone(), value.clone());
            row.push(value);
        }
        output_rows.push(row);
    }

    let group_count = output_rows.len() as i64;
    sort_virtual_aggregate_rows(&mut output_rows, &columns, &request)?;
    let offset = request.offset.unwrap_or(0).max(0) as usize;
    let rows = output_rows
        .into_iter()
        .skip(offset)
        .take(
            request
                .limit
                .map(|limit| limit.clamp(1, MAX_AGGREGATE_ROWS) as usize)
                .unwrap_or(usize::MAX),
        )
        .collect();

    Ok(runtara_object_store::AggregateResult {
        columns,
        rows,
        group_count,
    })
}

struct VirtualAggregateGroup<'a> {
    values: Vec<Value>,
    rows: Vec<&'a serde_json::Map<String, Value>>,
}

fn virtual_aggregate_value(
    block_id: &str,
    aggregate: &AggregateSpec,
    rows: &[&serde_json::Map<String, Value>],
    aliases: &HashMap<String, Value>,
) -> Result<Value, ReportServiceError> {
    match aggregate.fn_ {
        AggregateFn::Count => virtual_count_value(block_id, aggregate, rows),
        AggregateFn::Sum => {
            let values = virtual_numeric_values(block_id, aggregate, rows)?;
            if values.is_empty() {
                Ok(Value::Null)
            } else {
                Ok(f64_value(values.iter().sum()))
            }
        }
        AggregateFn::Avg => {
            let values = virtual_numeric_values(block_id, aggregate, rows)?;
            if values.is_empty() {
                Ok(Value::Null)
            } else {
                Ok(f64_value(values.iter().sum::<f64>() / values.len() as f64))
            }
        }
        AggregateFn::Min | AggregateFn::Max => virtual_min_max_value(block_id, aggregate, rows),
        AggregateFn::FirstValue | AggregateFn::LastValue => {
            virtual_first_last_value(block_id, aggregate, rows)
        }
        AggregateFn::PercentileCont | AggregateFn::PercentileDisc => {
            virtual_percentile_value(block_id, aggregate, rows)
        }
        AggregateFn::StddevSamp | AggregateFn::VarSamp => {
            virtual_sample_stat_value(block_id, aggregate, rows)
        }
        AggregateFn::Expr => {
            let expression = aggregate.expression.as_ref().ok_or_else(|| {
                ReportServiceError::Validation(format!(
                    "Block '{}' aggregate '{}' expression is required for expr",
                    block_id, aggregate.alias
                ))
            })?;
            evaluate_virtual_expression(block_id, expression, aliases)
        }
    }
}

fn virtual_count_value(
    block_id: &str,
    aggregate: &AggregateSpec,
    rows: &[&serde_json::Map<String, Value>],
) -> Result<Value, ReportServiceError> {
    if aggregate.distinct {
        let column = aggregate.column.as_deref().ok_or_else(|| {
            ReportServiceError::Validation(format!(
                "Block '{}' aggregate '{}' distinct count requires field",
                block_id, aggregate.alias
            ))
        })?;
        let mut values = HashSet::new();
        for row in rows {
            let value = virtual_row_value(row, column).unwrap_or(&Value::Null);
            if !value.is_null() {
                values.insert(value_to_lookup_key(value));
            }
        }
        return Ok(json!(values.len()));
    }

    if let Some(column) = aggregate.column.as_deref() {
        Ok(json!(
            rows.iter()
                .filter(|row| {
                    virtual_row_value(row, column).is_some_and(|value| !value.is_null())
                })
                .count()
        ))
    } else {
        Ok(json!(rows.len()))
    }
}

fn virtual_numeric_values(
    block_id: &str,
    aggregate: &AggregateSpec,
    rows: &[&serde_json::Map<String, Value>],
) -> Result<Vec<f64>, ReportServiceError> {
    let column = required_aggregate_column(block_id, aggregate)?;
    Ok(rows
        .iter()
        .filter_map(|row| virtual_row_value(row, column).and_then(Value::as_f64))
        .collect())
}

fn virtual_min_max_value(
    block_id: &str,
    aggregate: &AggregateSpec,
    rows: &[&serde_json::Map<String, Value>],
) -> Result<Value, ReportServiceError> {
    let column = required_aggregate_column(block_id, aggregate)?;
    let mut values = rows
        .iter()
        .filter_map(|row| virtual_row_value(row, column))
        .filter(|value| !value.is_null());
    let Some(first) = values.next() else {
        return Ok(Value::Null);
    };
    let mut selected = first.clone();
    for value in values {
        let ordering = virtual_compare_values(value, &selected).unwrap_or(Ordering::Equal);
        let should_replace = match aggregate.fn_ {
            AggregateFn::Min => ordering == Ordering::Less,
            AggregateFn::Max => ordering == Ordering::Greater,
            _ => false,
        };
        if should_replace {
            selected = value.clone();
        }
    }
    Ok(selected)
}

fn virtual_first_last_value(
    block_id: &str,
    aggregate: &AggregateSpec,
    rows: &[&serde_json::Map<String, Value>],
) -> Result<Value, ReportServiceError> {
    let column = required_aggregate_column(block_id, aggregate)?;
    if aggregate.order_by.is_empty() {
        return Err(ReportServiceError::Validation(format!(
            "Block '{}' aggregate '{}' first_value/last_value requires orderBy",
            block_id, aggregate.alias
        )));
    }
    let mut sorted = rows.to_vec();
    let flip = matches!(aggregate.fn_, AggregateFn::LastValue);
    sorted.sort_by(|left, right| {
        compare_virtual_rows_by_order(left, right, &aggregate.order_by, flip)
    });
    Ok(sorted
        .first()
        .and_then(|row| virtual_row_value(row, column))
        .cloned()
        .unwrap_or(Value::Null))
}

fn virtual_percentile_value(
    block_id: &str,
    aggregate: &AggregateSpec,
    rows: &[&serde_json::Map<String, Value>],
) -> Result<Value, ReportServiceError> {
    required_aggregate_column(block_id, aggregate)?;
    let percentile = aggregate.percentile.ok_or_else(|| {
        ReportServiceError::Validation(format!(
            "Block '{}' aggregate '{}' percentile is required",
            block_id, aggregate.alias
        ))
    })?;
    if !(0.0..=1.0).contains(&percentile) {
        return Err(ReportServiceError::Validation(format!(
            "Block '{}' aggregate '{}' percentile must be between 0 and 1",
            block_id, aggregate.alias
        )));
    }
    let order_by = aggregate.order_by.first().ok_or_else(|| {
        ReportServiceError::Validation(format!(
            "Block '{}' aggregate '{}' percentile requires orderBy",
            block_id, aggregate.alias
        ))
    })?;
    let mut values = rows
        .iter()
        .filter_map(|row| virtual_row_value(row, &order_by.column).and_then(Value::as_f64))
        .collect::<Vec<_>>();
    values.sort_by(|left, right| left.partial_cmp(right).unwrap_or(Ordering::Equal));
    if order_by.direction == SortDirection::Desc {
        values.reverse();
    }
    if values.is_empty() {
        return Ok(Value::Null);
    }
    if matches!(aggregate.fn_, AggregateFn::PercentileDisc) {
        let index = ((percentile * values.len() as f64).ceil() as usize)
            .saturating_sub(1)
            .min(values.len() - 1);
        return Ok(f64_value(values[index]));
    }
    if values.len() == 1 {
        return Ok(f64_value(values[0]));
    }
    let rank = percentile * (values.len() - 1) as f64;
    let lower_index = rank.floor() as usize;
    let upper_index = rank.ceil() as usize;
    if lower_index == upper_index {
        return Ok(f64_value(values[lower_index]));
    }
    let fraction = rank - lower_index as f64;
    Ok(f64_value(
        values[lower_index] + ((values[upper_index] - values[lower_index]) * fraction),
    ))
}

fn virtual_sample_stat_value(
    block_id: &str,
    aggregate: &AggregateSpec,
    rows: &[&serde_json::Map<String, Value>],
) -> Result<Value, ReportServiceError> {
    let values = virtual_numeric_values(block_id, aggregate, rows)?;
    if values.len() < 2 {
        return Ok(Value::Null);
    }
    let mean = values.iter().sum::<f64>() / values.len() as f64;
    let variance = values
        .iter()
        .map(|value| (value - mean).powi(2))
        .sum::<f64>()
        / (values.len() - 1) as f64;
    if matches!(aggregate.fn_, AggregateFn::StddevSamp) {
        Ok(f64_value(variance.sqrt()))
    } else {
        Ok(f64_value(variance))
    }
}

fn required_aggregate_column<'a>(
    block_id: &str,
    aggregate: &'a AggregateSpec,
) -> Result<&'a str, ReportServiceError> {
    aggregate.column.as_deref().ok_or_else(|| {
        ReportServiceError::Validation(format!(
            "Block '{}' aggregate '{}' requires field",
            block_id, aggregate.alias
        ))
    })
}

fn sort_virtual_aggregate_rows(
    rows: &mut [Vec<Value>],
    columns: &[String],
    request: &AggregateRequest,
) -> Result<(), ReportServiceError> {
    let column_index = columns
        .iter()
        .enumerate()
        .map(|(index, column)| (column.as_str(), index))
        .collect::<HashMap<_, _>>();

    if request.order_by.is_empty() {
        rows.sort_by(|left, right| {
            for index in 0..request.group_by.len() {
                let ordering = virtual_compare_values(
                    left.get(index).unwrap_or(&Value::Null),
                    right.get(index).unwrap_or(&Value::Null),
                )
                .unwrap_or(Ordering::Equal);
                if ordering != Ordering::Equal {
                    return ordering;
                }
            }
            Ordering::Equal
        });
        return Ok(());
    }

    for order_by in &request.order_by {
        if !column_index.contains_key(order_by.column.as_str()) {
            return Err(ReportServiceError::Validation(format!(
                "Aggregate orderBy references unavailable field '{}'",
                order_by.column
            )));
        }
    }

    rows.sort_by(|left, right| {
        for order_by in &request.order_by {
            let index = column_index[order_by.column.as_str()];
            let ordering = virtual_compare_values(
                left.get(index).unwrap_or(&Value::Null),
                right.get(index).unwrap_or(&Value::Null),
            )
            .unwrap_or(Ordering::Equal);
            let ordering = if order_by.direction == SortDirection::Desc {
                ordering.reverse()
            } else {
                ordering
            };
            if ordering != Ordering::Equal {
                return ordering;
            }
        }
        Ordering::Equal
    });
    Ok(())
}

fn compare_virtual_rows_by_order(
    left: &serde_json::Map<String, Value>,
    right: &serde_json::Map<String, Value>,
    order_by: &[AggregateOrderBy],
    flip_direction: bool,
) -> Ordering {
    for order in order_by {
        let left_value = virtual_row_value(left, &order.column).unwrap_or(&Value::Null);
        let right_value = virtual_row_value(right, &order.column).unwrap_or(&Value::Null);
        let ordering = virtual_compare_values(left_value, right_value).unwrap_or(Ordering::Equal);
        let descending = if flip_direction {
            order.direction == SortDirection::Asc
        } else {
            order.direction == SortDirection::Desc
        };
        let ordering = if descending {
            ordering.reverse()
        } else {
            ordering
        };
        if ordering != Ordering::Equal {
            return ordering;
        }
    }
    Ordering::Equal
}

fn virtual_row_value<'a>(
    row: &'a serde_json::Map<String, Value>,
    field: &str,
) -> Option<&'a Value> {
    if let Some(value) = row.get(field) {
        return Some(value);
    }

    let mut parts = field.split('.');
    let first = parts.next()?;
    let mut current = row.get(first)?;
    for part in parts {
        current = match current {
            Value::Object(object) => object.get(part)?,
            Value::Array(values) => {
                let index = part.parse::<usize>().ok()?;
                values.get(index)?
            }
            _ => return None,
        };
    }
    Some(current)
}

fn virtual_compare_values(left: &Value, right: &Value) -> Option<Ordering> {
    match (left, right) {
        (Value::Null, Value::Null) => Some(Ordering::Equal),
        (Value::Null, _) => Some(Ordering::Greater),
        (_, Value::Null) => Some(Ordering::Less),
        (Value::Number(left), Value::Number(right)) => left.as_f64()?.partial_cmp(&right.as_f64()?),
        (Value::String(left), Value::String(right)) => Some(left.cmp(right)),
        (Value::Bool(left), Value::Bool(right)) => Some(left.cmp(right)),
        _ => Some(virtual_value_sort_key(left).cmp(&virtual_value_sort_key(right))),
    }
}

fn virtual_value_sort_key(value: &Value) -> String {
    match value {
        Value::String(value) => value.clone(),
        Value::Bool(value) => value.to_string(),
        Value::Number(value) => value.to_string(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

fn evaluate_virtual_expression(
    block_id: &str,
    expression: &Value,
    aliases: &HashMap<String, Value>,
) -> Result<Value, ReportServiceError> {
    let normalized = normalize_report_aggregate_expression(expression);
    evaluate_virtual_expression_inner(block_id, &normalized, aliases, 0)
}

fn evaluate_virtual_expression_inner(
    block_id: &str,
    expression: &Value,
    aliases: &HashMap<String, Value>,
    depth: u8,
) -> Result<Value, ReportServiceError> {
    if depth > 8 {
        return Err(ReportServiceError::Validation(format!(
            "Block '{}' aggregate expression is too deeply nested",
            block_id
        )));
    }
    let Some(object) = expression.as_object() else {
        return Ok(expression.clone());
    };

    if let Some(value_type) = object.get("valueType").and_then(Value::as_str) {
        let value = object.get("value").cloned().unwrap_or(Value::Null);
        return match value_type.to_ascii_lowercase().as_str() {
            "alias" => {
                let alias = value.as_str().ok_or_else(|| {
                    ReportServiceError::Validation(format!(
                        "Block '{}' aggregate expression alias value must be a string",
                        block_id
                    ))
                })?;
                Ok(aliases.get(alias).cloned().unwrap_or(Value::Null))
            }
            "immediate" => Ok(value),
            "reference" => Err(ReportServiceError::Validation(format!(
                "Block '{}' aggregate expressions cannot reference row fields",
                block_id
            ))),
            other => Err(ReportServiceError::Validation(format!(
                "Block '{}' aggregate expression uses unsupported valueType '{}'",
                block_id, other
            ))),
        };
    }

    let op = object
        .get("op")
        .and_then(Value::as_str)
        .map(normalize_expression_token)
        .ok_or_else(|| {
            ReportServiceError::Validation(format!(
                "Block '{}' aggregate expression operation must include op",
                block_id
            ))
        })?;
    let arguments = object
        .get("arguments")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|argument| evaluate_virtual_expression_inner(block_id, &argument, aliases, depth + 1))
        .collect::<Result<Vec<_>, _>>()?;

    evaluate_virtual_expression_op(&op, &arguments)
}

fn evaluate_virtual_expression_op(
    op: &str,
    arguments: &[Value],
) -> Result<Value, ReportServiceError> {
    match op {
        "ADD" => Ok(option_f64_value(Some(
            arguments.iter().filter_map(Value::as_f64).sum(),
        ))),
        "SUB" => {
            let Some(first) = arguments.first().and_then(Value::as_f64) else {
                return Ok(Value::Null);
            };
            Ok(f64_value(
                arguments
                    .iter()
                    .skip(1)
                    .filter_map(Value::as_f64)
                    .fold(first, |acc, value| acc - value),
            ))
        }
        "MUL" => Ok(f64_value(
            arguments
                .iter()
                .filter_map(Value::as_f64)
                .fold(1.0, |acc, value| acc * value),
        )),
        "DIV" => {
            let (Some(left), Some(right)) = (
                arguments.first().and_then(Value::as_f64),
                arguments.get(1).and_then(Value::as_f64),
            ) else {
                return Ok(Value::Null);
            };
            if right == 0.0 {
                Ok(Value::Null)
            } else {
                Ok(f64_value(left / right))
            }
        }
        "NEG" => Ok(arguments
            .first()
            .and_then(Value::as_f64)
            .map(|value| f64_value(-value))
            .unwrap_or(Value::Null)),
        "ABS" => Ok(arguments
            .first()
            .and_then(Value::as_f64)
            .map(|value| f64_value(value.abs()))
            .unwrap_or(Value::Null)),
        "COALESCE" => Ok(arguments
            .iter()
            .find(|value| !value_is_empty(value))
            .cloned()
            .unwrap_or(Value::Null)),
        "EQ" | "NE" | "GT" | "GTE" | "LT" | "LTE" => {
            let left = arguments.first().unwrap_or(&Value::Null);
            let right = arguments.get(1).unwrap_or(&Value::Null);
            let ordering = virtual_compare_values(left, right);
            let equal = left == right || ordering == Some(Ordering::Equal);
            let result = match op {
                "EQ" => equal,
                "NE" => !equal,
                "GT" => ordering == Some(Ordering::Greater),
                "GTE" => equal || matches!(ordering, Some(Ordering::Greater | Ordering::Equal)),
                "LT" => ordering == Some(Ordering::Less),
                "LTE" => equal || matches!(ordering, Some(Ordering::Less | Ordering::Equal)),
                _ => false,
            };
            Ok(Value::Bool(result))
        }
        "AND" => Ok(Value::Bool(arguments.iter().all(expression_truthy))),
        "OR" => Ok(Value::Bool(arguments.iter().any(expression_truthy))),
        "NOT" => Ok(Value::Bool(
            !arguments.first().is_some_and(expression_truthy),
        )),
        "IS_DEFINED" => Ok(Value::Bool(
            arguments.first().is_some_and(|value| !value.is_null()),
        )),
        "IS_EMPTY" => Ok(Value::Bool(arguments.first().is_none_or(value_is_empty))),
        "IS_NOT_EMPTY" => Ok(Value::Bool(
            arguments
                .first()
                .is_some_and(|value| !value_is_empty(value)),
        )),
        other => Err(ReportServiceError::Validation(format!(
            "Aggregate expression uses unsupported op '{}'",
            other
        ))),
    }
}

fn expression_truthy(value: &Value) -> bool {
    match value {
        Value::Bool(value) => *value,
        Value::Number(value) => value.as_f64().is_some_and(|value| value != 0.0),
        Value::String(value) => !value.trim().is_empty(),
        Value::Array(values) => !values.is_empty(),
        Value::Object(values) => !values.is_empty(),
        Value::Null => false,
    }
}

fn merge_report_action_payload(
    payload: &Value,
    block: &ReportBlockDefinition,
    auth_context: &AuthContext,
) -> Result<Value, ReportServiceError> {
    let mut payload = match payload {
        Value::Null => serde_json::Map::new(),
        Value::Object(map) => map.clone(),
        _ => {
            return Err(ReportServiceError::Validation(
                "Report action payload must be a JSON object".to_string(),
            ));
        }
    };

    if let Some(submit) = block
        .actions
        .as_ref()
        .and_then(|actions| actions.submit.as_ref())
    {
        for (key, value) in &submit.implicit_payload {
            payload.insert(
                key.clone(),
                resolve_report_action_implicit_value(value, auth_context),
            );
        }
    }

    Ok(Value::Object(payload))
}

fn resolve_report_action_implicit_value(value: &Value, auth_context: &AuthContext) -> Value {
    match value {
        Value::String(template) => Value::String(render_viewer_template(template, auth_context)),
        Value::Array(values) => Value::Array(
            values
                .iter()
                .map(|value| resolve_report_action_implicit_value(value, auth_context))
                .collect(),
        ),
        Value::Object(object) => Value::Object(
            object
                .iter()
                .map(|(key, value)| {
                    (
                        key.clone(),
                        resolve_report_action_implicit_value(value, auth_context),
                    )
                })
                .collect(),
        ),
        _ => value.clone(),
    }
}

fn render_viewer_template(template: &str, auth_context: &AuthContext) -> String {
    template
        .replace("{{viewer.user_id}}", &auth_context.user_id)
        .replace("{{viewer.id}}", &auth_context.user_id)
        .replace("{{viewer.org_id}}", &auth_context.org_id)
        .replace("{{viewer.tenant_id}}", &auth_context.org_id)
        .replace(
            "{{viewer.auth_method}}",
            auth_method_name(&auth_context.auth_method),
        )
}

fn auth_method_name(method: &AuthMethod) -> &'static str {
    match method {
        AuthMethod::Jwt => "jwt",
        AuthMethod::ApiKey => "api_key",
        AuthMethod::Unauthenticated => "unauthenticated",
    }
}

fn apply_workflow_runtime_row_filters(
    rows: Vec<serde_json::Map<String, Value>>,
    definition: &ReportDefinition,
    block: &ReportBlockDefinition,
    resolved_filters: &HashMap<String, Value>,
    block_request: Option<&ReportBlockDataRequest>,
) -> Result<Vec<serde_json::Map<String, Value>>, ReportServiceError> {
    let Some(condition) =
        build_block_condition(definition, block, resolved_filters, block_request)?
    else {
        return Ok(rows);
    };

    rows.into_iter()
        .filter_map(
            |row| match condition_matches_row(&condition, &row, &block.id) {
                Ok(true) => Some(Ok(row)),
                Ok(false) => None,
                Err(error) => Some(Err(error)),
            },
        )
        .collect()
}

fn map_workflow_runtime_error_to_report(error: WorkflowRuntimeError) -> ReportServiceError {
    match error {
        WorkflowRuntimeError::InvalidRequest(message) => ReportServiceError::Validation(message),
        WorkflowRuntimeError::NotFound(message) => ReportServiceError::Validation(message),
        WorkflowRuntimeError::Conflict(message) => ReportServiceError::Conflict(message),
        WorkflowRuntimeError::RuntimeUnavailable => ReportServiceError::Validation(
            "Workflow runtime report sources require a configured runtime client".to_string(),
        ),
        WorkflowRuntimeError::Runtime(message) => ReportServiceError::Database(message),
    }
}

fn map_execution_error_to_report(error: ExecutionError) -> ReportServiceError {
    match error {
        ExecutionError::ValidationError(message) => ReportServiceError::Validation(message),
        ExecutionError::NotFound(message) | ExecutionError::WorkflowNotFound(message) => {
            ReportServiceError::Validation(message)
        }
        ExecutionError::NotConnected(_) => ReportServiceError::Validation(
            "Workflow runtime report sources require a configured runtime client".to_string(),
        ),
        _ => ReportServiceError::Database(error.to_string()),
    }
}

#[derive(Debug)]
struct CompiledDatasetQuery {
    source: ReportSource,
    columns: Vec<ReportDatasetQueryColumn>,
}

fn compiled_dataset_block(
    definition: &ReportDefinition,
    block: &ReportBlockDefinition,
) -> Result<ReportBlockDefinition, ReportServiceError> {
    let dataset_query = block.dataset.as_ref().ok_or_else(|| {
        ReportServiceError::Validation(format!("Block '{}' does not define dataset", block.id))
    })?;
    let dataset = definition
        .datasets
        .iter()
        .find(|dataset| dataset.id == dataset_query.id)
        .ok_or_else(|| {
            ReportServiceError::Validation(format!(
                "Block '{}' references unknown dataset '{}'",
                block.id, dataset_query.id
            ))
        })?;
    let compiled = compile_dataset_query(
        &block.id,
        dataset,
        &dataset_query.dimensions,
        &dataset_query.measures,
        &dataset_query.order_by,
        dataset_query.limit,
    )?;
    let mut compiled_block = block.clone();
    compiled_block.dataset = None;
    compiled_block.source = compiled.source;
    compiled_block.source.condition = build_dataset_condition(
        definition,
        dataset,
        &HashMap::new(),
        &dataset_query.dataset_filters,
        None,
    )?;
    Ok(compiled_block)
}

fn compile_dataset_query(
    context: &str,
    dataset: &ReportDatasetDefinition,
    dimensions: &[String],
    measures: &[String],
    order_by: &[ReportOrderBy],
    limit: Option<i64>,
) -> Result<CompiledDatasetQuery, ReportServiceError> {
    if measures.is_empty() {
        return Err(ReportServiceError::Validation(format!(
            "{} must select at least one dataset measure",
            humanize_dataset_context(context)
        )));
    }

    let dimensions_by_field = dataset
        .dimensions
        .iter()
        .map(|dimension| (dimension.field.as_str(), dimension))
        .collect::<HashMap<_, _>>();
    let measures_by_id = dataset
        .measures
        .iter()
        .map(|measure| (measure.id.as_str(), measure))
        .collect::<HashMap<_, _>>();

    let mut seen_dimensions = HashSet::new();
    let mut group_by = Vec::with_capacity(dimensions.len());
    let mut columns = Vec::with_capacity(dimensions.len() + measures.len());
    for dimension_id in dimensions {
        let dimension_id = dimension_id.trim();
        let Some(dimension) = dimensions_by_field.get(dimension_id) else {
            return Err(ReportServiceError::Validation(format!(
                "{} references unknown dataset dimension '{}'",
                humanize_dataset_context(context),
                dimension_id
            )));
        };
        if !seen_dimensions.insert(dimension_id.to_string()) {
            return Err(ReportServiceError::Validation(format!(
                "{} selects duplicate dataset dimension '{}'",
                humanize_dataset_context(context),
                dimension_id
            )));
        }
        group_by.push(dimension.field.clone());
        columns.push(ReportDatasetQueryColumn {
            key: dimension.field.clone(),
            label: dimension.label.clone(),
            column_type: dataset_dimension_type_name(dimension.dimension_type).to_string(),
            format: dimension.format,
        });
    }

    let mut seen_measures = HashSet::new();
    let mut aggregates = Vec::with_capacity(measures.len());
    for measure_id in measures {
        let measure_id = measure_id.trim();
        let Some(measure) = measures_by_id.get(measure_id) else {
            return Err(ReportServiceError::Validation(format!(
                "{} references unknown dataset measure '{}'",
                humanize_dataset_context(context),
                measure_id
            )));
        };
        if !seen_measures.insert(measure_id.to_string()) {
            return Err(ReportServiceError::Validation(format!(
                "{} selects duplicate dataset measure '{}'",
                humanize_dataset_context(context),
                measure_id
            )));
        }
        aggregates.push(ReportAggregateSpec {
            alias: measure.id.clone(),
            op: measure.op,
            field: measure.field.clone(),
            distinct: measure.distinct,
            order_by: measure.order_by.clone(),
            expression: measure.expression.clone(),
            percentile: measure.percentile,
        });
        columns.push(ReportDatasetQueryColumn {
            key: measure.id.clone(),
            label: measure.label.clone(),
            column_type: "measure".to_string(),
            format: Some(measure.format),
        });
    }

    let output_fields = group_by
        .iter()
        .chain(aggregates.iter().map(|aggregate| &aggregate.alias))
        .cloned()
        .collect::<HashSet<_>>();
    for order in order_by {
        if !output_fields.contains(&order.field) {
            return Err(ReportServiceError::Validation(format!(
                "{} orderBy references unselected dataset field '{}'",
                humanize_dataset_context(context),
                order.field
            )));
        }
    }

    Ok(CompiledDatasetQuery {
        source: ReportSource {
            kind: ReportSourceKind::ObjectModel,
            schema: dataset.source.schema.clone(),
            connection_id: dataset.source.connection_id.clone(),
            entity: None,
            workflow_id: None,
            instance_id: None,
            mode: ReportSourceMode::Aggregate,
            condition: None,
            filter_mappings: vec![],
            group_by,
            aggregates,
            order_by: order_by.to_vec(),
            limit: limit.map(|value| value.clamp(1, MAX_AGGREGATE_ROWS)),
            granularity: None,
            interval: None,
            join: vec![],
        },
        columns,
    })
}

fn build_dataset_condition(
    definition: &ReportDefinition,
    dataset: &ReportDatasetDefinition,
    resolved_filters: &HashMap<String, Value>,
    dataset_filters: &[ReportDatasetFilter],
    search: Option<&ReportTableSearchRequest>,
) -> Result<Option<Condition>, ReportServiceError> {
    let mut conditions = Vec::new();
    for filter in &definition.filters {
        let Some(value) = resolved_filters.get(&filter.id) else {
            continue;
        };
        for target in &filter.applies_to {
            if target.block_id.is_some() {
                continue;
            }
            if let Some(condition) = condition_from_filter_target(target, value) {
                conditions.push(condition);
            }
        }
    }

    let dataset_filterable_fields = dataset_filterable_fields(dataset);
    for filter in dataset_filters {
        if !dataset_filterable_fields.contains(&filter.field) {
            return Err(ReportServiceError::Validation(format!(
                "Dataset query filter references unknown dataset field '{}'",
                filter.field
            )));
        }
        let target = ReportFilterTarget {
            filter_id: None,
            block_id: None,
            field: filter.field.clone(),
            op: filter.op.clone(),
        };
        if let Some(condition) = condition_from_filter_target(&target, &filter.value) {
            conditions.push(condition);
        }
    }

    if let Some(search) = search
        && !search.query.trim().is_empty()
    {
        let requested_fields = search
            .fields
            .iter()
            .map(String::as_str)
            .collect::<HashSet<_>>();
        let mut search_conditions = dataset
            .dimensions
            .iter()
            .map(|dimension| dimension.field.as_str())
            .filter(|field| requested_fields.is_empty() || requested_fields.contains(field))
            .filter(|field| dataset_filterable_fields.contains(*field))
            .map(|field| {
                binary_condition(
                    "CONTAINS",
                    Value::String(field.to_string()),
                    Value::String(search.query.trim().to_string()),
                )
            })
            .collect::<Vec<_>>();

        match search_conditions.len() {
            0 => {}
            1 => conditions.push(search_conditions.remove(0)),
            _ => conditions.push(Condition {
                op: "OR".to_string(),
                arguments: Some(
                    search_conditions
                        .into_iter()
                        .filter_map(|condition| serde_json::to_value(condition).ok())
                        .collect(),
                ),
            }),
        }
    }

    Ok(combine_conditions(conditions))
}

fn dataset_filterable_fields(dataset: &ReportDatasetDefinition) -> HashSet<String> {
    let mut fields = dataset
        .dimensions
        .iter()
        .map(|dimension| dimension.field.clone())
        .collect::<HashSet<_>>();
    if let Some(time_dimension) = &dataset.time_dimension {
        fields.insert(time_dimension.clone());
    }
    for measure in &dataset.measures {
        if let Some(field) = &measure.field {
            fields.insert(field.clone());
        }
    }
    fields
}

fn validate_dataset_block_output(
    block: &ReportBlockDefinition,
    source: &ReportSource,
) -> Result<(), ReportServiceError> {
    let output_fields = aggregate_source_output_fields(&source.group_by, &source.aggregates);

    if let Some(table) = &block.table {
        for column in &table.columns {
            if column.is_chart() {
                return Err(ReportServiceError::Validation(format!(
                    "Block '{}' dataset-backed table chart columns are not supported yet",
                    block.id
                )));
            }
            if column.is_interaction_buttons() {
                continue;
            }
            if !output_fields.contains(&column.field) {
                return Err(ReportServiceError::Validation(format!(
                    "Block '{}' references unknown dataset table field '{}'",
                    block.id, column.field
                )));
            }
        }
        for sort in &table.default_sort {
            if !output_fields.contains(&sort.field) {
                return Err(ReportServiceError::Validation(format!(
                    "Block '{}' references unknown dataset table sort field '{}'",
                    block.id, sort.field
                )));
            }
        }
    }

    if let Some(chart) = &block.chart {
        if !output_fields.contains(&chart.x) {
            return Err(ReportServiceError::Validation(format!(
                "Block '{}' references unknown dataset chart x field '{}'",
                block.id, chart.x
            )));
        }
        for series in &chart.series {
            if !output_fields.contains(&series.field) {
                return Err(ReportServiceError::Validation(format!(
                    "Block '{}' references unknown dataset chart series field '{}'",
                    block.id, series.field
                )));
            }
        }
    }

    if let Some(metric) = &block.metric
        && !output_fields.contains(&metric.value_field)
    {
        return Err(ReportServiceError::Validation(format!(
            "Block '{}' references unknown dataset metric valueField '{}'",
            block.id, metric.value_field
        )));
    }

    Ok(())
}

fn humanize_dataset_context(context: &str) -> String {
    if context == "dataset query" {
        "Dataset query".to_string()
    } else {
        format!("Block '{}'", context)
    }
}

fn dataset_dimension_type_name(value: ReportDatasetFieldType) -> &'static str {
    match value {
        ReportDatasetFieldType::String => "string",
        ReportDatasetFieldType::Number => "number",
        ReportDatasetFieldType::Decimal => "decimal",
        ReportDatasetFieldType::Boolean => "boolean",
        ReportDatasetFieldType::Date => "date",
        ReportDatasetFieldType::Datetime => "datetime",
        ReportDatasetFieldType::Json => "json",
    }
}

fn build_aggregate_request(
    definition: &ReportDefinition,
    block: &ReportBlockDefinition,
    resolved_filters: &HashMap<String, Value>,
) -> Result<AggregateRequest, ReportServiceError> {
    build_aggregate_request_from_parts(
        &block.id,
        &block.source.group_by,
        &block.source.aggregates,
        &block.source.order_by,
        Some(
            block
                .source
                .limit
                .unwrap_or(MAX_AGGREGATE_ROWS)
                .min(MAX_AGGREGATE_ROWS),
        ),
        Some(0),
        build_block_condition(definition, block, resolved_filters, None)?,
    )
}

fn build_table_aggregate_request(
    definition: &ReportDefinition,
    block: &ReportBlockDefinition,
    resolved_filters: &HashMap<String, Value>,
    block_request: Option<&ReportBlockDataRequest>,
    sort: &[ReportOrderBy],
    page_size: i64,
    offset: i64,
) -> Result<AggregateRequest, ReportServiceError> {
    build_aggregate_request_from_parts(
        &block.id,
        &block.source.group_by,
        &block.source.aggregates,
        sort,
        Some(page_size),
        Some(offset),
        build_block_condition(definition, block, resolved_filters, block_request)?,
    )
}

fn build_column_aggregate_request(
    source: &ReportTableColumnSource,
    condition: Option<Condition>,
) -> Result<AggregateRequest, ReportServiceError> {
    build_aggregate_request_from_parts(
        "table column",
        &source.group_by,
        &source.aggregates,
        &source.order_by,
        Some(source.limit.unwrap_or(30).min(MAX_AGGREGATE_ROWS)),
        Some(0),
        condition,
    )
}

fn build_aggregate_request_from_parts(
    block_id: &str,
    group_by: &[String],
    aggregates: &[ReportAggregateSpec],
    order_by: &[ReportOrderBy],
    limit: Option<i64>,
    offset: Option<i64>,
    condition: Option<Condition>,
) -> Result<AggregateRequest, ReportServiceError> {
    if aggregates.is_empty() {
        return Err(ReportServiceError::Validation(format!(
            "Block '{}' must define at least one aggregate",
            block_id
        )));
    }

    Ok(AggregateRequest {
        condition,
        group_by: group_by.to_vec(),
        aggregates: aggregates
            .iter()
            .map(|aggregate| AggregateSpec {
                alias: aggregate.alias.clone(),
                fn_: map_aggregate_fn(aggregate.op),
                column: aggregate.field.clone(),
                distinct: aggregate.distinct,
                order_by: aggregate.order_by.iter().map(map_order_by).collect(),
                expression: aggregate
                    .expression
                    .as_ref()
                    .map(normalize_report_aggregate_expression),
                percentile: aggregate.percentile,
            })
            .collect(),
        order_by: order_by.iter().map(map_order_by).collect(),
        limit,
        offset,
    })
}

fn map_aggregate_fn(value: ReportAggregateFn) -> AggregateFn {
    match value {
        ReportAggregateFn::Count => AggregateFn::Count,
        ReportAggregateFn::Sum => AggregateFn::Sum,
        ReportAggregateFn::Avg => AggregateFn::Avg,
        ReportAggregateFn::Min => AggregateFn::Min,
        ReportAggregateFn::Max => AggregateFn::Max,
        ReportAggregateFn::FirstValue => AggregateFn::FirstValue,
        ReportAggregateFn::LastValue => AggregateFn::LastValue,
        ReportAggregateFn::PercentileCont => AggregateFn::PercentileCont,
        ReportAggregateFn::PercentileDisc => AggregateFn::PercentileDisc,
        ReportAggregateFn::StddevSamp => AggregateFn::StddevSamp,
        ReportAggregateFn::VarSamp => AggregateFn::VarSamp,
        ReportAggregateFn::Expr => AggregateFn::Expr,
    }
}

fn map_order_by(value: &ReportOrderBy) -> AggregateOrderBy {
    AggregateOrderBy {
        column: value.field.clone(),
        direction: if value.direction.eq_ignore_ascii_case("desc") {
            SortDirection::Desc
        } else {
            SortDirection::Asc
        },
    }
}

fn table_output_columns(
    table: Option<&ReportTableConfig>,
    source_columns: &[String],
) -> Vec<String> {
    table
        .filter(|table| !table.columns.is_empty())
        .map(|table| {
            table
                .columns
                .iter()
                .map(|column| column.field.clone())
                .collect()
        })
        .unwrap_or_else(|| source_columns.to_vec())
}

fn table_response_columns(table: Option<&ReportTableConfig>) -> Vec<Value> {
    table
        .map(|table| {
            table
                .columns
                .iter()
                .map(table_response_column)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

fn table_response_column(column: &ReportTableColumn) -> Value {
    let mut output = serde_json::Map::new();
    output.insert("key".to_string(), Value::String(column.field.clone()));
    output.insert(
        "label".to_string(),
        Value::String(
            column
                .label
                .clone()
                .unwrap_or_else(|| humanize_label(&column.field)),
        ),
    );
    output.insert(
        "format".to_string(),
        column
            .format
            .as_ref()
            .map(|format| Value::String(format.clone()))
            .unwrap_or(Value::Null),
    );
    if let Some(display_field) = column
        .display_field
        .clone()
        .or_else(|| auto_lookup_display_field(column))
    {
        output.insert("displayField".to_string(), Value::String(display_field));
    }

    Value::Object(output)
}

fn auto_lookup_display_field(column: &ReportTableColumn) -> Option<String> {
    if column.display_field.is_some()
        || column.is_chart()
        || column.is_value_lookup()
        || column.is_workflow_button()
        || column.is_interaction_buttons()
    {
        return None;
    }
    lookup_editor_for_table_column(column)?;
    Some(format!("__lookupDisplay.{}", column.field))
}

fn project_aggregate_table_rows(
    table: Option<&ReportTableConfig>,
    source_columns: &[String],
    rows: Vec<Vec<Value>>,
) -> Result<Vec<Vec<Value>>, ReportServiceError> {
    let Some(table) = table.filter(|table| !table.columns.is_empty()) else {
        return Ok(rows);
    };
    let source_indexes: HashMap<_, _> = source_columns
        .iter()
        .enumerate()
        .map(|(index, column)| (column.as_str(), index))
        .collect();

    rows.into_iter()
        .map(|row| {
            table
                .columns
                .iter()
                .map(|column| {
                    if column.is_chart() || column.is_interaction_buttons() {
                        return Ok(Value::Null);
                    }
                    let Some(source_index) = source_indexes.get(column.field.as_str()) else {
                        return Err(ReportServiceError::Validation(format!(
                            "Aggregate table references unavailable field '{}'",
                            column.field
                        )));
                    };
                    Ok(row.get(*source_index).cloned().unwrap_or(Value::Null))
                })
                .collect()
        })
        .collect()
}

fn aggregate_rows_to_maps(
    source_columns: &[String],
    rows: &[Vec<Value>],
) -> Vec<serde_json::Map<String, Value>> {
    rows.iter()
        .map(|row| {
            source_columns
                .iter()
                .enumerate()
                .map(|(index, column)| {
                    (
                        column.clone(),
                        row.get(index).cloned().unwrap_or(Value::Null),
                    )
                })
                .collect()
        })
        .collect()
}

fn enrich_filter_join_rows(
    alias_to_join: &HashMap<String, &ReportSourceJoin>,
    join_data: &HashMap<String, JoinResolution>,
    rows: Vec<serde_json::Map<String, Value>>,
) -> Vec<serde_json::Map<String, Value>> {
    rows.into_iter()
        .filter_map(|mut row| {
            for (alias, join) in alias_to_join {
                let dim_row = row
                    .get(&join.parent_field)
                    .filter(|value| !value_is_empty(value))
                    .and_then(|value| {
                        join_data
                            .get(alias)
                            .and_then(|data| data.by_key.get(&value_to_lookup_key(value)))
                    });

                let Some(dim_row) = dim_row else {
                    if matches!(join.kind, ReportJoinKind::Inner) {
                        return None;
                    }
                    continue;
                };

                for (field, value) in dim_row {
                    row.insert(format!("{}.{}", alias, field), value.clone());
                }
            }
            Some(row)
        })
        .collect()
}

fn build_table_column_condition(
    condition_context: ReportConditionRuntimeContext<'_>,
    source: &ReportTableColumnSource,
    row: &serde_json::Map<String, Value>,
) -> Result<Option<Condition>, ReportServiceError> {
    let mut conditions = Vec::new();
    let condition_filter_defs =
        block_condition_filter_definitions(condition_context.definition, condition_context.block);
    let condition_filter_values = block_condition_filter_values(
        condition_context.block,
        condition_context.resolved_filters,
        condition_context.block_request,
    );
    let context = format!("block '{}' table column source", condition_context.block.id);
    if let Some(condition) = resolve_optional_report_condition(
        source.condition.as_ref(),
        &condition_filter_defs,
        &condition_filter_values,
        &context,
    )? {
        conditions.push(condition);
    }
    for join in &source.join {
        let Some(value) = row.get(&join.parent_field) else {
            continue;
        };
        if let Some(condition) = condition_from_table_column_join(join, value) {
            conditions.push(condition);
        }
    }
    Ok(combine_conditions(conditions))
}

fn condition_from_table_column_join(
    join: &ReportTableColumnJoin,
    value: &Value,
) -> Option<Condition> {
    if value_is_empty(value) {
        return None;
    }

    let field = Value::String(join.field.clone());
    match join.op.to_ascii_lowercase().as_str() {
        "in" => Some(Condition {
            op: "IN".to_string(),
            arguments: Some(vec![field, ensure_array(value)]),
        }),
        "ne" => Some(binary_condition("NE", field, value.clone())),
        "gt" => Some(binary_condition("GT", field, value.clone())),
        "gte" => Some(binary_condition("GTE", field, value.clone())),
        "lt" => Some(binary_condition("LT", field, value.clone())),
        "lte" => Some(binary_condition("LTE", field, value.clone())),
        "contains" | "search" => Some(binary_condition("CONTAINS", field, value.clone())),
        _ => Some(binary_condition("EQ", field, value.clone())),
    }
}

fn aggregate_output_fields(block: &ReportBlockDefinition) -> HashSet<String> {
    aggregate_source_output_fields(&block.source.group_by, &block.source.aggregates)
}

fn validate_report_aggregate_specs(
    context: &str,
    aggregates: &[ReportAggregateSpec],
) -> Result<(), ReportServiceError> {
    let mut aliases = HashSet::new();
    for aggregate in aggregates {
        if aggregate.alias.trim().is_empty() {
            return Err(ReportServiceError::Validation(format!(
                "{} aggregate aliases cannot be empty",
                context
            )));
        }
        if !aliases.insert(aggregate.alias.clone()) {
            return Err(ReportServiceError::Validation(format!(
                "{} has duplicate aggregate alias '{}'",
                context, aggregate.alias
            )));
        }
        validate_report_aggregate_spec_shape(
            &format!("{} aggregate '{}'", context, aggregate.alias),
            aggregate.op,
            aggregate.field.as_deref(),
            aggregate.distinct,
            &aggregate.order_by,
            aggregate.expression.as_ref(),
            aggregate.percentile,
        )?;
    }
    Ok(())
}

fn validate_report_aggregate_spec_shape(
    context: &str,
    op: ReportAggregateFn,
    field: Option<&str>,
    distinct: bool,
    order_by: &[ReportOrderBy],
    expression: Option<&Value>,
    percentile: Option<f64>,
) -> Result<(), ReportServiceError> {
    let field_empty = field.is_none_or(|field| field.trim().is_empty());
    match op {
        ReportAggregateFn::Count => {
            if distinct && field_empty {
                return Err(ReportServiceError::Validation(format!(
                    "{context} count with distinct=true requires field"
                )));
            }
            if !order_by.is_empty() {
                return Err(ReportServiceError::Validation(format!(
                    "{context} orderBy is only valid for first_value, last_value, percentile_cont, and percentile_disc"
                )));
            }
            if expression.is_some() {
                return Err(ReportServiceError::Validation(format!(
                    "{context} expression is only valid for op expr"
                )));
            }
            if percentile.is_some() {
                return Err(ReportServiceError::Validation(format!(
                    "{context} percentile is only valid for percentile_cont and percentile_disc"
                )));
            }
        }
        ReportAggregateFn::Sum
        | ReportAggregateFn::Avg
        | ReportAggregateFn::Min
        | ReportAggregateFn::Max
        | ReportAggregateFn::StddevSamp
        | ReportAggregateFn::VarSamp => {
            if field_empty {
                return Err(ReportServiceError::Validation(format!(
                    "{context} requires field for op {:?}",
                    op
                )));
            }
            if distinct {
                return Err(ReportServiceError::Validation(format!(
                    "{context} distinct is only valid for count"
                )));
            }
            if !order_by.is_empty() {
                return Err(ReportServiceError::Validation(format!(
                    "{context} orderBy is only valid for first_value, last_value, percentile_cont, and percentile_disc"
                )));
            }
            if expression.is_some() {
                return Err(ReportServiceError::Validation(format!(
                    "{context} expression is only valid for op expr"
                )));
            }
            if percentile.is_some() {
                return Err(ReportServiceError::Validation(format!(
                    "{context} percentile is only valid for percentile_cont and percentile_disc"
                )));
            }
        }
        ReportAggregateFn::FirstValue | ReportAggregateFn::LastValue => {
            if field_empty {
                return Err(ReportServiceError::Validation(format!(
                    "{context} requires field for op {:?}",
                    op
                )));
            }
            if distinct {
                return Err(ReportServiceError::Validation(format!(
                    "{context} distinct is only valid for count"
                )));
            }
            if order_by.is_empty() {
                return Err(ReportServiceError::Validation(format!(
                    "{context} requires non-empty orderBy"
                )));
            }
            if expression.is_some() {
                return Err(ReportServiceError::Validation(format!(
                    "{context} expression is only valid for op expr"
                )));
            }
            if percentile.is_some() {
                return Err(ReportServiceError::Validation(format!(
                    "{context} percentile is only valid for percentile_cont and percentile_disc"
                )));
            }
        }
        ReportAggregateFn::PercentileCont | ReportAggregateFn::PercentileDisc => {
            if !field_empty {
                return Err(ReportServiceError::Validation(format!(
                    "{context} takes its value field from orderBy, not field"
                )));
            }
            if distinct {
                return Err(ReportServiceError::Validation(format!(
                    "{context} distinct is only valid for count"
                )));
            }
            let percentile = percentile.ok_or_else(|| {
                ReportServiceError::Validation(format!(
                    "{context} requires percentile in [0.0, 1.0]"
                ))
            })?;
            if !percentile.is_finite() || !(0.0..=1.0).contains(&percentile) {
                return Err(ReportServiceError::Validation(format!(
                    "{context} percentile must be a finite number in [0.0, 1.0]"
                )));
            }
            if order_by.len() != 1 {
                return Err(ReportServiceError::Validation(format!(
                    "{context} requires exactly one orderBy entry"
                )));
            }
            if expression.is_some() {
                return Err(ReportServiceError::Validation(format!(
                    "{context} expression is only valid for op expr"
                )));
            }
        }
        ReportAggregateFn::Expr => {
            if !field_empty {
                return Err(ReportServiceError::Validation(format!(
                    "{context} must not specify field for op expr"
                )));
            }
            if distinct {
                return Err(ReportServiceError::Validation(format!(
                    "{context} must not specify distinct for op expr"
                )));
            }
            if !order_by.is_empty() {
                return Err(ReportServiceError::Validation(format!(
                    "{context} must not specify orderBy for op expr"
                )));
            }
            if percentile.is_some() {
                return Err(ReportServiceError::Validation(format!(
                    "{context} percentile is only valid for percentile_cont and percentile_disc"
                )));
            }
            let expression = expression.ok_or_else(|| {
                ReportServiceError::Validation(format!("{context} requires expression for op expr"))
            })?;
            let normalized = normalize_report_aggregate_expression(expression);
            serde_json::from_value::<runtara_object_store::ExprNode>(normalized).map_err(
                |err| {
                    ReportServiceError::Validation(format!(
                        "{context} expression is invalid: {}",
                        err
                    ))
                },
            )?;
        }
    }
    Ok(())
}

fn aggregate_source_output_fields(
    group_by: &[String],
    aggregates: &[ReportAggregateSpec],
) -> HashSet<String> {
    group_by
        .iter()
        .chain(aggregates.iter().map(|aggregate| &aggregate.alias))
        .cloned()
        .collect()
}

fn is_schema_field(schema_fields: &HashSet<String>, field: &str) -> bool {
    schema_fields.contains(field)
        || matches!(
            field,
            "id" | "created_at" | "updated_at" | "createdAt" | "updatedAt"
        )
}

fn lookup_editor_for_field<'a>(
    block: &'a ReportBlockDefinition,
    field: &str,
) -> Option<&'a ReportLookupConfig> {
    if let Some(table) = block.table.as_ref() {
        for column in &table.columns {
            if column.field == field
                && let Some(lookup) = lookup_editor_for_table_column(column)
            {
                return Some(lookup);
            }
        }
    }

    if let Some(card) = block.card.as_ref() {
        for group in &card.groups {
            for card_field in &group.fields {
                if card_field.field == field
                    && card_field.kind == ReportCardFieldKind::Value
                    && card_field
                        .editor
                        .as_ref()
                        .is_some_and(|editor| editor.kind == ReportEditorKind::Lookup)
                {
                    return card_field
                        .editor
                        .as_ref()
                        .and_then(|editor| editor.lookup.as_ref());
                }
            }
        }
    }

    None
}

fn lookup_editor_for_table_column(column: &ReportTableColumn) -> Option<&ReportLookupConfig> {
    column
        .editor
        .as_ref()
        .filter(|editor| editor.kind == ReportEditorKind::Lookup)
        .and_then(|editor| editor.lookup.as_ref())
}

fn normalize_report_aggregate_expression(expression: &Value) -> Value {
    let Value::Object(object) = expression else {
        return expression.clone();
    };

    let mut normalized = object.clone();

    if let Some(value_type) = normalized.remove("value_type")
        && !normalized.contains_key("valueType")
    {
        normalized.insert("valueType".to_string(), value_type);
    }

    if !normalized.contains_key("valueType")
        && let Some(type_value) = normalized.get("type").and_then(Value::as_str)
        && matches!(
            normalize_expression_token(type_value).as_str(),
            "ALIAS" | "IMMEDIATE" | "REFERENCE"
        )
    {
        normalized.insert(
            "valueType".to_string(),
            Value::String(type_value.to_ascii_lowercase()),
        );
    }

    if let Some(value_type) = normalized.get_mut("valueType")
        && let Some(raw) = value_type.as_str()
    {
        *value_type = Value::String(raw.to_ascii_lowercase());
    }

    if let Some(op) = normalized.get_mut("op")
        && let Some(raw) = op.as_str()
    {
        *op = Value::String(normalize_expression_token(raw));
    }

    if let Some(fn_name) = normalized.get_mut("fn")
        && let Some(raw) = fn_name.as_str()
    {
        *fn_name = Value::String(normalize_expression_token(raw));
    }

    if let Some(arguments) = normalized.get_mut("arguments")
        && let Some(arguments) = arguments.as_array_mut()
    {
        for argument in arguments {
            *argument = normalize_report_aggregate_expression(argument);
        }
    }

    Value::Object(normalized)
}

fn normalize_expression_token(value: &str) -> String {
    let mut normalized = String::new();
    let mut previous_was_separator = true;
    let mut previous_was_lower_or_digit = false;

    for character in value.trim().chars() {
        if character == '_' || character == '-' || character == ' ' {
            if !previous_was_separator && !normalized.is_empty() {
                normalized.push('_');
            }
            previous_was_separator = true;
            previous_was_lower_or_digit = false;
            continue;
        }

        if character.is_uppercase() && previous_was_lower_or_digit && !previous_was_separator {
            normalized.push('_');
        }

        for upper in character.to_uppercase() {
            normalized.push(upper);
        }
        previous_was_separator = false;
        previous_was_lower_or_digit = character.is_lowercase() || character.is_ascii_digit();
    }

    match normalized.as_str() {
        "EQUALS" => "EQ".to_string(),
        "NOT_EQUALS" | "NOT_EQUAL" => "NE".to_string(),
        "GREATER_THAN" => "GT".to_string(),
        "GREATER_THAN_OR_EQUAL" | "GREATER_THAN_OR_EQUALS" => "GTE".to_string(),
        "LESS_THAN" => "LT".to_string(),
        "LESS_THAN_OR_EQUAL" | "LESS_THAN_OR_EQUALS" => "LTE".to_string(),
        "DEFINED" => "IS_DEFINED".to_string(),
        "EMPTY" => "IS_EMPTY".to_string(),
        "NOT_EMPTY" => "IS_NOT_EMPTY".to_string(),
        _ => normalized,
    }
}

fn normalize_sort_direction(value: &str) -> String {
    if value.eq_ignore_ascii_case("desc") {
        "desc".to_string()
    } else {
        "asc".to_string()
    }
}

fn value_is_empty(value: &Value) -> bool {
    match value {
        Value::Null => true,
        Value::String(value) => value.trim().is_empty(),
        Value::Array(values) => values.is_empty(),
        Value::Object(values) => values.is_empty(),
        _ => false,
    }
}

fn ensure_array(value: &Value) -> Value {
    match value {
        Value::Array(_) => value.clone(),
        _ => Value::Array(vec![value.clone()]),
    }
}

fn flatten_instance(instance: crate::api::dto::object_model::Instance) -> Value {
    let mut row = serde_json::Map::new();
    row.insert("id".to_string(), Value::String(instance.id));
    row.insert("tenantId".to_string(), Value::String(instance.tenant_id));
    row.insert("createdAt".to_string(), Value::String(instance.created_at));
    row.insert("updatedAt".to_string(), Value::String(instance.updated_at));

    if let Some(schema_id) = instance.schema_id {
        row.insert("schemaId".to_string(), Value::String(schema_id));
    }
    if let Some(schema_name) = instance.schema_name {
        row.insert("schemaName".to_string(), Value::String(schema_name));
    }
    if let Value::Object(properties) = instance.properties {
        row.extend(properties);
    }
    if let Some(computed) = instance.computed {
        row.extend(computed);
    }

    Value::Object(row)
}

fn lookup_display_labels_from_instances(
    instances: Vec<crate::api::dto::object_model::Instance>,
    value_field: &str,
    label_field: &str,
) -> HashMap<String, Value> {
    instances
        .into_iter()
        .filter_map(|instance| {
            let Value::Object(row) = flatten_instance(instance) else {
                return None;
            };
            let key = row.get(value_field)?;
            if value_is_empty(key) {
                return None;
            }
            let label = row.get(label_field).unwrap_or(key);
            Some((
                value_to_lookup_key(key),
                Value::String(filter_option_label(label)),
            ))
        })
        .fold(HashMap::new(), |mut labels_by_key, (key, label)| {
            labels_by_key.entry(key).or_insert(label);
            labels_by_key
        })
}

fn is_empty_data(data: &Value) -> bool {
    data.get("rows")
        .and_then(Value::as_array)
        .is_some_and(|rows| rows.is_empty())
        || data
            .get("actions")
            .and_then(Value::as_array)
            .is_some_and(|actions| actions.is_empty())
        || data
            .get("missing")
            .and_then(Value::as_bool)
            .unwrap_or(false)
}

fn clamp_page_size(size: i64) -> i64 {
    size.clamp(1, MAX_TABLE_PAGE_SIZE)
}

fn slugify(value: &str) -> String {
    let mut slug = String::new();
    let mut last_dash = false;

    for ch in value.chars().flat_map(char::to_lowercase) {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch);
            last_dash = false;
        } else if !last_dash {
            slug.push('-');
            last_dash = true;
        }
    }

    slug.trim_matches('-').to_string()
}

fn validate_slug(slug: &str) -> Result<(), ReportServiceError> {
    if slug.is_empty() {
        return Err(ReportServiceError::Validation(
            "Report slug cannot be empty".to_string(),
        ));
    }
    if !slug
        .chars()
        .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-')
    {
        return Err(ReportServiceError::Validation(
            "Report slug can only contain lowercase letters, numbers, and dashes".to_string(),
        ));
    }
    Ok(())
}

fn humanize_label(value: &str) -> String {
    value
        .split(['_', '-'])
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn find_block_index(
    blocks: &[ReportBlockDefinition],
    block_id: &str,
) -> Result<usize, ReportServiceError> {
    blocks
        .iter()
        .position(|block| block.id == block_id)
        .ok_or_else(|| ReportServiceError::Validation(format!("Unknown block '{}'", block_id)))
}

fn resolve_position_index(
    blocks: &[ReportBlockDefinition],
    position: &ReportBlockPosition,
) -> Result<usize, ReportServiceError> {
    let selector_count = usize::from(position.index.is_some())
        + usize::from(position.before_block_id.is_some())
        + usize::from(position.after_block_id.is_some());

    if selector_count > 1 {
        return Err(ReportServiceError::Validation(
            "Report block position must use only one of index, beforeBlockId, or afterBlockId"
                .to_string(),
        ));
    }

    if let Some(index) = position.index {
        return Ok(index.min(blocks.len()));
    }

    if let Some(before_block_id) = &position.before_block_id {
        if before_block_id.trim().is_empty() {
            return Err(ReportServiceError::Validation(
                "beforeBlockId cannot be empty".to_string(),
            ));
        }
        return blocks
            .iter()
            .position(|block| block.id == *before_block_id)
            .ok_or_else(|| {
                ReportServiceError::Validation(format!(
                    "Unknown beforeBlockId '{}'",
                    before_block_id
                ))
            });
    }

    if let Some(after_block_id) = &position.after_block_id {
        if after_block_id.trim().is_empty() {
            return Err(ReportServiceError::Validation(
                "afterBlockId cannot be empty".to_string(),
            ));
        }
        return blocks
            .iter()
            .position(|block| block.id == *after_block_id)
            .map(|index| index + 1)
            .ok_or_else(|| {
                ReportServiceError::Validation(format!("Unknown afterBlockId '{}'", after_block_id))
            });
    }

    Ok(blocks.len())
}

fn apply_json_merge_patch(target: &mut Value, patch: &Value) {
    match (target, patch) {
        (Value::Object(target), Value::Object(patch)) => {
            for (key, patch_value) in patch {
                if patch_value.is_null() {
                    target.remove(key);
                } else {
                    apply_json_merge_patch(
                        target.entry(key.clone()).or_insert(Value::Null),
                        patch_value,
                    );
                }
            }
        }
        (target, patch) => {
            *target = patch.clone();
        }
    }
}

fn map_sqlx_error(error: sqlx::Error) -> ReportServiceError {
    if let sqlx::Error::Database(db_error) = &error
        && db_error
            .constraint()
            .is_some_and(|constraint| constraint == "idx_report_definitions_tenant_slug_active")
    {
        return ReportServiceError::Conflict("A report with this slug already exists".to_string());
    }

    ReportServiceError::Database(error.to_string())
}

fn map_object_model_error(error: ObjectModelServiceError) -> ReportServiceError {
    match error {
        ObjectModelServiceError::ValidationError(message) => {
            ReportServiceError::Validation(message)
        }
        ObjectModelServiceError::NotFound(message) => ReportServiceError::Validation(message),
        ObjectModelServiceError::Conflict(message) => ReportServiceError::Conflict(message),
        ObjectModelServiceError::DatabaseError(message) => ReportServiceError::Database(message),
    }
}

fn seal_json_schema_objects(schema: &mut Value) {
    match schema {
        Value::Object(object) => {
            if object.contains_key("properties") && !object.contains_key("additionalProperties") {
                object.insert("additionalProperties".to_string(), Value::Bool(false));
            }
            for value in object.values_mut() {
                seal_json_schema_objects(value);
            }
        }
        Value::Array(values) => {
            for value in values {
                seal_json_schema_objects(value);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn object_column(
        name: &str,
        column_type: crate::api::dto::object_model::ColumnType,
    ) -> crate::api::dto::object_model::ColumnDefinition {
        crate::api::dto::object_model::ColumnDefinition {
            name: name.to_string(),
            column_type,
            nullable: true,
            unique: false,
            default_value: None,
            text_index: crate::api::dto::object_model::TextIndexKind::None,
        }
    }

    fn object_schema(
        columns: Vec<crate::api::dto::object_model::ColumnDefinition>,
    ) -> crate::api::dto::object_model::Schema {
        crate::api::dto::object_model::Schema {
            id: "schema-1".to_string(),
            tenant_id: "tenant-1".to_string(),
            created_at: "2026-05-10T00:00:00Z".to_string(),
            updated_at: "2026-05-10T00:00:00Z".to_string(),
            name: "CategoryTreeNode".to_string(),
            description: None,
            table_name: "category_tree_node".to_string(),
            columns,
            indexes: None,
        }
    }

    fn test_block(id: &str) -> ReportBlockDefinition {
        ReportBlockDefinition {
            id: id.to_string(),
            block_type: ReportBlockType::Table,
            title: None,
            lazy: false,
            dataset: None,
            source: ReportSource {
                kind: ReportSourceKind::ObjectModel,
                schema: "Order".to_string(),
                connection_id: None,
                entity: None,
                workflow_id: None,
                instance_id: None,
                mode: ReportSourceMode::Filter,
                condition: None,
                filter_mappings: vec![],
                group_by: vec![],
                aggregates: vec![],
                order_by: vec![],
                limit: None,
                granularity: None,
                interval: None,
                join: vec![],
            },
            table: None,
            chart: None,
            metric: None,
            actions: None,
            card: None,
            markdown: None,
            filters: vec![],
            interactions: vec![],
            show_when: None,
            hide_when_empty: false,
        }
    }

    fn test_metric_block(id: &str) -> ReportBlockDefinition {
        let mut block = test_block(id);
        block.block_type = ReportBlockType::Metric;
        block.source.mode = ReportSourceMode::Aggregate;
        block.source.aggregates = vec![ReportAggregateSpec {
            alias: "value".to_string(),
            op: ReportAggregateFn::Count,
            field: None,
            distinct: false,
            order_by: vec![],
            expression: None,
            percentile: None,
        }];
        block.metric = Some(ReportMetricConfig {
            value_field: "value".to_string(),
            label: None,
            format: None,
        });
        block
    }

    fn table_column(field: &str) -> ReportTableColumn {
        ReportTableColumn {
            field: field.to_string(),
            label: None,
            display_field: None,
            format: None,
            column_type: None,
            chart: None,
            source: None,
            secondary_field: None,
            link_field: None,
            tooltip_field: None,
            pill_variants: None,
            levels: None,
            align: None,
            descriptive: false,
            editable: false,
            editor: None,
            workflow_action: None,
            interaction_buttons: vec![],
        }
    }

    #[test]
    fn option_search_condition_prefers_generated_tsvector_columns() {
        let schema = object_schema(vec![
            object_column("name", crate::api::dto::object_model::ColumnType::String),
            object_column(
                "search_blob",
                crate::api::dto::object_model::ColumnType::String,
            ),
            object_column(
                "search_tsv",
                crate::api::dto::object_model::ColumnType::Tsvector {
                    source_column: "search_blob".to_string(),
                    language: "english".to_string(),
                },
            ),
        ]);

        let condition =
            option_search_condition(&schema, &["name".to_string()], "animal health").unwrap();

        assert_eq!(condition.op, "MATCH");
        assert_eq!(
            condition.arguments,
            Some(vec![json!("search_tsv"), json!("animal health")])
        );
    }

    #[test]
    fn option_search_condition_falls_back_to_contains_without_tsvector() {
        let schema = object_schema(vec![
            object_column("name", crate::api::dto::object_model::ColumnType::String),
            object_column("code", crate::api::dto::object_model::ColumnType::String),
        ]);

        let condition =
            option_search_condition(&schema, &["name".to_string(), "code".to_string()], "bolt")
                .unwrap();

        assert_eq!(condition.op, "OR");
        let nested = condition
            .arguments
            .unwrap()
            .into_iter()
            .map(serde_json::from_value::<Condition>)
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(nested[0].op, "CONTAINS");
        assert_eq!(
            nested[0].arguments,
            Some(vec![json!("name"), json!("bolt")])
        );
        assert_eq!(nested[1].op, "CONTAINS");
        assert_eq!(
            nested[1].arguments,
            Some(vec![json!("code"), json!("bolt")])
        );
    }

    #[test]
    fn table_response_columns_auto_display_lookup_editor_labels() {
        let mut column = table_column("suggested_category_id");
        column.label = Some("Suggested Category".to_string());
        column.editable = true;
        column.editor = Some(ReportEditorConfig {
            kind: ReportEditorKind::Lookup,
            lookup: Some(ReportLookupConfig {
                schema: "Category".to_string(),
                connection_id: None,
                value_field: "id".to_string(),
                label_field: "name".to_string(),
                search_fields: vec!["name".to_string()],
                condition: None,
                filter_mappings: vec![],
            }),
            options: vec![],
            min: None,
            max: None,
            step: None,
            regex: None,
            placeholder: None,
        });
        let table = ReportTableConfig {
            columns: vec![column],
            selectable: false,
            actions: vec![],
            default_sort: vec![],
            pagination: None,
        };

        let columns = table_response_columns(Some(&table));

        assert_eq!(
            columns[0].get("displayField"),
            Some(&json!("__lookupDisplay.suggested_category_id"))
        );
    }

    #[test]
    fn lookup_display_labels_from_instances_reads_platform_id() {
        let labels = lookup_display_labels_from_instances(
            vec![crate::api::dto::object_model::Instance {
                id: "b628bfd4-a6a4-40c7-b33b-674611417334".to_string(),
                tenant_id: "org_test".to_string(),
                created_at: "2026-05-10T00:00:00Z".to_string(),
                updated_at: "2026-05-10T00:00:00Z".to_string(),
                schema_id: Some("category_schema".to_string()),
                schema_name: Some("CategoryTreeNode".to_string()),
                properties: json!({
                    "node_id": "mro-bearings-bolts",
                    "label_text": "MRO > Bearings > Bolts"
                }),
                computed: None,
            }],
            "id",
            "label_text",
        );

        assert_eq!(
            labels.get("b628bfd4-a6a4-40c7-b33b-674611417334"),
            Some(&json!("MRO > Bearings > Bolts"))
        );
    }

    #[test]
    fn table_response_columns_preserve_explicit_display_field() {
        let mut column = table_column("category_id");
        column.display_field = Some("category.name".to_string());
        column.editor = Some(ReportEditorConfig {
            kind: ReportEditorKind::Lookup,
            lookup: Some(ReportLookupConfig {
                schema: "Category".to_string(),
                connection_id: None,
                value_field: "id".to_string(),
                label_field: "name".to_string(),
                search_fields: vec![],
                condition: None,
                filter_mappings: vec![],
            }),
            options: vec![],
            min: None,
            max: None,
            step: None,
            regex: None,
            placeholder: None,
        });
        let table = ReportTableConfig {
            columns: vec![column],
            selectable: false,
            actions: vec![],
            default_sort: vec![],
            pagination: None,
        };

        let columns = table_response_columns(Some(&table));

        assert_eq!(
            columns[0].get("displayField"),
            Some(&json!("category.name"))
        );
    }

    fn test_view(id: &str, parent_view_id: Option<&str>) -> ReportViewDefinition {
        ReportViewDefinition {
            id: id.to_string(),
            title: None,
            title_from: None,
            title_from_block: None,
            parent_view_id: parent_view_id.map(str::to_string),
            clear_filters_on_back: vec![],
            breadcrumb: vec![],
            layout: vec![],
        }
    }

    fn test_dataset() -> ReportDatasetDefinition {
        ReportDatasetDefinition {
            id: "stock_snapshots".to_string(),
            label: "Stock snapshots".to_string(),
            source: ReportDatasetSource {
                schema: "StockSnapshot".to_string(),
                connection_id: None,
            },
            time_dimension: Some("snapshot_date".to_string()),
            dimensions: vec![
                ReportDatasetDimension {
                    field: "vendor".to_string(),
                    label: "Vendor".to_string(),
                    dimension_type: ReportDatasetFieldType::String,
                    format: None,
                },
                ReportDatasetDimension {
                    field: "category".to_string(),
                    label: "Category".to_string(),
                    dimension_type: ReportDatasetFieldType::String,
                    format: None,
                },
            ],
            measures: vec![
                ReportDatasetMeasure {
                    id: "snapshot_count".to_string(),
                    label: "Snapshots".to_string(),
                    op: ReportAggregateFn::Count,
                    field: None,
                    distinct: false,
                    order_by: vec![],
                    expression: None,
                    percentile: None,
                    format: ReportDatasetValueFormat::Number,
                },
                ReportDatasetMeasure {
                    id: "qty_total".to_string(),
                    label: "Total quantity".to_string(),
                    op: ReportAggregateFn::Sum,
                    field: Some("qty".to_string()),
                    distinct: false,
                    order_by: vec![],
                    expression: None,
                    percentile: None,
                    format: ReportDatasetValueFormat::Number,
                },
            ],
        }
    }

    #[test]
    fn report_condition_field_validation_rejects_unknown_fields() {
        let fields = HashSet::from(["status".to_string()]);
        let condition = Condition {
            op: "EQ".to_string(),
            arguments: Some(vec![json!("missing_status"), json!("active")]),
        };

        let err = validate_report_condition_field_refs(
            Some(&condition),
            &|field| is_schema_field(&fields, field),
            "block 'orders'",
        )
        .unwrap_err();

        assert!(
            err.to_string().contains("unknown field 'missing_status'"),
            "{}",
            err
        );
    }

    #[test]
    fn report_condition_field_validation_allows_filter_refs_and_subquery_operands() {
        let fields = HashSet::from(["customer_id".to_string(), "created_at".to_string()]);
        let condition = Condition {
            op: "AND".to_string(),
            arguments: Some(vec![
                json!({"op": "GTE", "arguments": ["created_at", {"filter": "date_range", "path": "from"}]}),
                json!({"op": "IN", "arguments": ["customer_id", {"subquery": {"schema": "Customer", "select": "id"}}]}),
            ]),
        };

        validate_report_condition_field_refs(
            Some(&condition),
            &|field| is_schema_field(&fields, field),
            "block 'orders'",
        )
        .unwrap();
    }

    #[test]
    fn report_aggregate_shape_validation_enforces_percentile_contract() {
        let valid = vec![ReportAggregateSpec {
            alias: "p95_amount".to_string(),
            op: ReportAggregateFn::PercentileCont,
            field: None,
            distinct: false,
            order_by: vec![ReportOrderBy {
                field: "amount".to_string(),
                direction: "asc".to_string(),
            }],
            expression: None,
            percentile: Some(0.95),
        }];
        validate_report_aggregate_specs("block 'orders'", &valid).unwrap();

        let mut invalid = valid[0].clone();
        invalid.percentile = Some(1.5);
        let err = validate_report_aggregate_specs("block 'orders'", &[invalid]).unwrap_err();
        assert!(
            err.to_string()
                .contains("percentile must be a finite number"),
            "{}",
            err
        );
    }

    #[test]
    fn report_markdown_placeholders_support_source_paths() {
        let mut block = test_block("intro");
        block.block_type = ReportBlockType::Markdown;
        block.source.mode = ReportSourceMode::Aggregate;
        block.source.group_by = vec!["customer".to_string()];
        block.source.aggregates = vec![ReportAggregateSpec {
            alias: "revenue".to_string(),
            op: ReportAggregateFn::Sum,
            field: Some("total_amount".to_string()),
            distinct: false,
            order_by: vec![],
            expression: None,
            percentile: None,
        }];
        block.markdown = Some(ReportMarkdownConfig {
            content: "Revenue: {{ source.revenue }}\nCustomer: {{source[0].customer.name}}"
                .to_string(),
        });

        let placeholders = validate_report_markdown_block_shape(&block).unwrap();
        let known_fields = HashSet::from(["customer".to_string(), "revenue".to_string()]);

        validate_report_markdown_placeholders(&block, &placeholders, &|field| {
            markdown_output_field_known(field, &|candidate| known_fields.contains(candidate))
        })
        .unwrap();
    }

    #[test]
    fn report_markdown_placeholders_reject_block_placeholders() {
        let err =
            report_markdown_source_placeholders("{{ block.revenue_by_day }}", "block 'intro'")
                .unwrap_err();
        let issue = report_validation_issue_from_error(err);

        assert_eq!(issue.code, "UNSUPPORTED_MARKDOWN_PLACEHOLDER");
    }

    #[test]
    fn report_definition_json_schema_seals_known_object_shapes() {
        let schema = ReportService::report_definition_json_schema();
        assert_eq!(schema.get("additionalProperties"), Some(&json!(false)));
        assert_eq!(
            schema.pointer("/$defs/ReportBlockDefinition/additionalProperties"),
            Some(&json!(false))
        );
    }

    #[test]
    fn report_definition_json_schema_validation_rejects_unknown_fields() {
        let definition = json!({
            "definitionVersion": 1,
            "filters": [],
            "blocks": [],
            "unexpected": true
        });

        let err = ReportService::validate_report_definition_json_syntax(&definition).unwrap_err();
        assert!(err.to_string().contains("unexpected"), "{}", err);
    }

    #[test]
    fn report_definition_json_schema_validation_rejects_legacy_markdown_field() {
        let definition = json!({
            "definitionVersion": 1,
            "markdown": "# Report",
            "filters": [],
            "blocks": []
        });

        let err = ReportService::validate_report_definition_json_syntax(&definition).unwrap_err();
        assert!(err.to_string().contains("markdown"), "{}", err);
    }

    #[test]
    fn report_definition_json_schema_validation_rejects_legacy_layout_markdown_node() {
        let definition = json!({
            "definitionVersion": 1,
            "layout": [{"id": "intro", "type": "markdown", "content": "# Report"}],
            "blocks": []
        });

        let err = ReportService::validate_report_definition_json_syntax(&definition).unwrap_err();
        assert!(err.to_string().contains("markdown"), "{}", err);
    }

    #[test]
    fn report_view_navigation_accepts_parent_chain_and_back_filters() {
        let mut views = vec![
            test_view("a", None),
            test_view("b", Some("a")),
            test_view("c", Some("b")),
        ];
        views[1].clear_filters_on_back = vec!["b_id".to_string()];
        views[2].clear_filters_on_back = vec!["c_id".to_string()];

        let view_ids = HashSet::from(["a".to_string(), "b".to_string(), "c".to_string()]);
        let filter_ids = HashSet::from(["b_id".to_string(), "c_id".to_string()]);

        validate_report_view_navigation(&views, &view_ids, &filter_ids).unwrap();
    }

    #[test]
    fn report_view_navigation_rejects_cycles() {
        let views = vec![test_view("a", Some("b")), test_view("b", Some("a"))];
        let view_ids = HashSet::from(["a".to_string(), "b".to_string()]);
        let filter_ids = HashSet::new();

        let err = validate_report_view_navigation(&views, &view_ids, &filter_ids).unwrap_err();

        assert!(err.to_string().contains("cycle"));
    }

    #[test]
    fn resolves_report_block_positions() {
        let blocks = vec![test_block("a"), test_block("b"), test_block("c")];

        assert_eq!(
            resolve_position_index(
                &blocks,
                &ReportBlockPosition {
                    index: Some(1),
                    ..Default::default()
                }
            )
            .unwrap(),
            1
        );
        assert_eq!(
            resolve_position_index(
                &blocks,
                &ReportBlockPosition {
                    before_block_id: Some("b".to_string()),
                    ..Default::default()
                }
            )
            .unwrap(),
            1
        );
        assert_eq!(
            resolve_position_index(
                &blocks,
                &ReportBlockPosition {
                    after_block_id: Some("b".to_string()),
                    ..Default::default()
                }
            )
            .unwrap(),
            2
        );
    }

    #[test]
    fn table_search_condition_uses_configured_columns() {
        let mut block = test_block("orders");
        block.table = Some(ReportTableConfig {
            columns: vec![table_column("customer_name"), table_column("status")],
            selectable: false,
            actions: vec![],
            default_sort: vec![],
            pagination: None,
        });
        let request = ReportBlockDataRequest {
            id: "orders".to_string(),
            page: None,
            sort: vec![],
            search: Some(ReportTableSearchRequest {
                query: " acme ".to_string(),
                fields: vec![],
            }),
            block_filters: HashMap::new(),
        };

        let mut conditions = Vec::new();
        append_table_search_condition(&mut conditions, &block, &request);

        assert_eq!(conditions.len(), 1);
        assert_eq!(conditions[0].op, "OR");
        let arguments = conditions[0].arguments.as_ref().unwrap();
        assert_eq!(arguments.len(), 2);
        assert_eq!(arguments[0]["arguments"][0], json!("customer_name"));
        assert_eq!(arguments[0]["arguments"][1], json!("acme"));
        assert_eq!(arguments[1]["arguments"][0], json!("status"));
    }

    #[test]
    fn table_search_condition_rejects_unconfigured_requested_fields() {
        let mut block = test_block("orders");
        block.table = Some(ReportTableConfig {
            columns: vec![table_column("customer_name")],
            selectable: false,
            actions: vec![],
            default_sort: vec![],
            pagination: None,
        });
        let request = ReportBlockDataRequest {
            id: "orders".to_string(),
            page: None,
            sort: vec![],
            search: Some(ReportTableSearchRequest {
                query: "acme".to_string(),
                fields: vec!["customer_name".to_string(), "internal_note".to_string()],
            }),
            block_filters: HashMap::new(),
        };

        let mut conditions = Vec::new();
        append_table_search_condition(&mut conditions, &block, &request);

        assert_eq!(conditions.len(), 1);
        assert_eq!(conditions[0].op, "CONTAINS");
        assert_eq!(
            conditions[0].arguments.as_ref().unwrap(),
            &vec![json!("customer_name"), json!("acme")]
        );
    }

    #[test]
    fn aggregate_table_search_condition_uses_group_by_fields() {
        let mut block = test_block("stock");
        block.source.mode = ReportSourceMode::Aggregate;
        block.source.group_by = vec!["sku".to_string(), "vendor".to_string()];
        block.source.aggregates = vec![ReportAggregateSpec {
            alias: "qty_total".to_string(),
            op: ReportAggregateFn::Sum,
            field: Some("qty".to_string()),
            distinct: false,
            order_by: vec![],
            expression: None,
            percentile: None,
        }];
        block.table = Some(ReportTableConfig {
            columns: vec![table_column("sku"), table_column("qty_total")],
            selectable: false,
            actions: vec![],
            default_sort: vec![],
            pagination: None,
        });
        let request = ReportBlockDataRequest {
            id: "stock".to_string(),
            page: None,
            sort: vec![],
            search: Some(ReportTableSearchRequest {
                query: "103".to_string(),
                fields: vec!["qty_total".to_string(), "sku".to_string()],
            }),
            block_filters: HashMap::new(),
        };

        let mut conditions = Vec::new();
        append_table_search_condition(&mut conditions, &block, &request);

        assert_eq!(conditions.len(), 1);
        assert_eq!(conditions[0].op, "CONTAINS");
        assert_eq!(
            conditions[0].arguments.as_ref().unwrap(),
            &vec![json!("sku"), json!("103")]
        );
    }

    #[test]
    fn aggregate_table_rows_project_to_configured_column_order() {
        let table = ReportTableConfig {
            columns: vec![table_column("sku"), table_column("delta")],
            selectable: false,
            actions: vec![],
            default_sort: vec![],
            pagination: None,
        };
        let rows = project_aggregate_table_rows(
            Some(&table),
            &[
                "sku".to_string(),
                "first_qty".to_string(),
                "delta".to_string(),
            ],
            vec![vec![json!("10397904"), json!(15473), json!(-15473)]],
        )
        .unwrap();

        assert_eq!(rows, vec![vec![json!("10397904"), json!(-15473)]]);
    }

    #[test]
    fn aggregate_expr_normalizes_workflow_style_expression() {
        let expression = normalize_report_aggregate_expression(&json!({
            "type": "operation",
            "op": "Sub",
            "arguments": [
                {"valueType": "Alias", "value": "last_qty"},
                {"value_type": "alias", "value": "first_qty"}
            ]
        }));

        assert_eq!(expression["op"], json!("SUB"));
        assert_eq!(expression["arguments"][0]["valueType"], json!("alias"));
        assert_eq!(expression["arguments"][1]["valueType"], json!("alias"));
        serde_json::from_value::<runtara_object_store::ExprNode>(expression).unwrap();
    }

    #[test]
    fn dataset_block_deserializes_without_raw_source() {
        let block: ReportBlockDefinition = serde_json::from_value(json!({
            "id": "vendor_summary",
            "type": "table",
            "dataset": {
                "id": "stock_snapshots",
                "dimensions": ["vendor"],
                "measures": ["snapshot_count", "qty_total"],
                "orderBy": [{"field": "qty_total", "direction": "desc"}]
            },
            "table": {
                "columns": [
                    {"field": "vendor", "label": "Vendor"},
                    {"field": "qty_total", "label": "Total quantity"}
                ]
            }
        }))
        .unwrap();

        assert!(block.source.is_empty());
        assert_eq!(block.dataset.unwrap().id, "stock_snapshots");
    }

    #[test]
    fn value_table_column_source_deserializes_select_and_default_join_kind() {
        let column: ReportTableColumn = serde_json::from_value(json!({
            "field": "part_number",
            "type": "value",
            "source": {
                "schema": "TDProduct",
                "mode": "filter",
                "select": "part_number",
                "join": [{"parentField": "sku", "field": "sku"}]
            }
        }))
        .unwrap();

        let source = column.source.unwrap();
        assert_eq!(source.select.as_deref(), Some("part_number"));
        assert_eq!(source.join[0].kind, ReportJoinKind::Left);
    }

    #[test]
    fn dataset_query_compiles_to_aggregate_source() {
        let dataset = test_dataset();
        let compiled = compile_dataset_query(
            "vendor_summary",
            &dataset,
            &["vendor".to_string()],
            &["snapshot_count".to_string(), "qty_total".to_string()],
            &[ReportOrderBy {
                field: "qty_total".to_string(),
                direction: "desc".to_string(),
            }],
            Some(25),
        )
        .unwrap();

        assert_eq!(compiled.source.schema, "StockSnapshot");
        assert_eq!(compiled.source.mode, ReportSourceMode::Aggregate);
        assert_eq!(compiled.source.group_by, vec!["vendor".to_string()]);
        assert_eq!(compiled.source.aggregates.len(), 2);
        assert_eq!(compiled.source.aggregates[0].alias, "snapshot_count");
        assert_eq!(compiled.source.aggregates[0].op, ReportAggregateFn::Count);
        assert_eq!(compiled.source.aggregates[1].field, Some("qty".to_string()));
        assert_eq!(compiled.source.limit, Some(25));
        assert_eq!(
            compiled
                .columns
                .iter()
                .map(|c| c.key.as_str())
                .collect::<Vec<_>>(),
            vec!["vendor", "snapshot_count", "qty_total"]
        );

        let request = build_aggregate_request_from_parts(
            "vendor_summary",
            &compiled.source.group_by,
            &compiled.source.aggregates,
            &compiled.source.order_by,
            Some(50),
            Some(0),
            None,
        )
        .unwrap();

        assert_eq!(request.group_by, vec!["vendor".to_string()]);
        assert_eq!(request.aggregates[0].fn_, AggregateFn::Count);
        assert_eq!(request.aggregates[1].fn_, AggregateFn::Sum);
        assert_eq!(request.order_by[0].column, "qty_total");
        assert_eq!(request.order_by[0].direction, SortDirection::Desc);
    }

    #[test]
    fn dataset_query_condition_compiles_report_filters_explore_filters_and_search() {
        let dataset = test_dataset();
        let definition = ReportDefinition {
            definition_version: 1,
            layout: vec![],
            views: vec![],
            filters: vec![ReportFilterDefinition {
                id: "vendor".to_string(),
                label: "Vendor".to_string(),
                filter_type: ReportFilterType::Select,
                default: None,
                required: false,
                strict_when_referenced: false,
                options: None,
                applies_to: vec![ReportFilterTarget {
                    filter_id: None,
                    block_id: None,
                    field: "vendor".to_string(),
                    op: "eq".to_string(),
                }],
            }],
            datasets: vec![dataset.clone()],
            blocks: vec![],
        };
        let mut resolved_filters = HashMap::new();
        resolved_filters.insert("vendor".to_string(), json!("Fabrikam"));

        let condition = build_dataset_condition(
            &definition,
            &dataset,
            &resolved_filters,
            &[ReportDatasetFilter {
                field: "category".to_string(),
                op: "contains".to_string(),
                value: json!("Storage"),
            }],
            Some(&ReportTableSearchRequest {
                query: "fab".to_string(),
                fields: vec!["vendor".to_string()],
            }),
        )
        .unwrap()
        .unwrap();
        let condition = serde_json::to_value(condition).unwrap();

        assert_eq!(condition["op"], json!("AND"));
        assert!(condition.to_string().contains("\"EQ\""));
        assert!(condition.to_string().contains("\"CONTAINS\""));
        assert!(condition.to_string().contains("Fabrikam"));
        assert!(condition.to_string().contains("Storage"));
    }

    #[test]
    fn dataset_query_condition_rejects_unknown_filter_fields() {
        let dataset = test_dataset();
        let err = build_dataset_condition(
            &ReportDefinition {
                definition_version: 1,
                layout: vec![],
                views: vec![],
                filters: vec![],
                datasets: vec![dataset.clone()],
                blocks: vec![],
            },
            &dataset,
            &HashMap::new(),
            &[ReportDatasetFilter {
                field: "sku".to_string(),
                op: "eq".to_string(),
                value: json!("SKU-1"),
            }],
            None,
        )
        .unwrap_err();

        assert!(err.to_string().contains("unknown dataset field 'sku'"));
    }

    #[test]
    fn dataset_block_compilation_preserves_dataset_filters() {
        let dataset = test_dataset();
        let mut block = test_block("vendor_storage");
        block.source = default_report_source();
        block.dataset = Some(ReportBlockDatasetQuery {
            id: dataset.id.clone(),
            dimensions: vec!["vendor".to_string()],
            measures: vec!["qty_total".to_string()],
            order_by: vec![],
            dataset_filters: vec![ReportDatasetFilter {
                field: "category".to_string(),
                op: "eq".to_string(),
                value: json!("Storage"),
            }],
            limit: Some(25),
        });
        block.table = Some(ReportTableConfig {
            columns: vec![table_column("vendor"), table_column("qty_total")],
            selectable: false,
            actions: vec![],
            default_sort: vec![],
            pagination: None,
        });
        let definition = ReportDefinition {
            definition_version: 1,
            layout: vec![],
            views: vec![],
            filters: vec![],
            datasets: vec![dataset],
            blocks: vec![block.clone()],
        };

        let compiled = compiled_dataset_block(&definition, &block).unwrap();
        let condition = compiled.source.condition.unwrap();

        assert_eq!(condition.op, "EQ");
        assert_eq!(
            condition.arguments,
            Some(vec![json!("category"), json!("Storage")])
        );
    }

    #[test]
    fn dataset_query_rejects_unknown_fields_and_unselected_sort() {
        let dataset = test_dataset();

        let err = compile_dataset_query(
            "vendor_summary",
            &dataset,
            &["sku".to_string()],
            &["qty_total".to_string()],
            &[],
            None,
        )
        .unwrap_err();
        assert!(err.to_string().contains("unknown dataset dimension 'sku'"));

        let err = compile_dataset_query(
            "vendor_summary",
            &dataset,
            &["vendor".to_string()],
            &["missing".to_string()],
            &[],
            None,
        )
        .unwrap_err();
        assert!(
            err.to_string()
                .contains("unknown dataset measure 'missing'")
        );

        let err = compile_dataset_query(
            "vendor_summary",
            &dataset,
            &["vendor".to_string()],
            &["qty_total".to_string()],
            &[ReportOrderBy {
                field: "snapshot_count".to_string(),
                direction: "desc".to_string(),
            }],
            None,
        )
        .unwrap_err();
        assert!(err.to_string().contains("unselected dataset field"));
    }

    #[test]
    fn dataset_block_validation_checks_visual_output_fields() {
        let dataset = test_dataset();
        let mut block = test_block("vendor_summary");
        block.dataset = Some(ReportBlockDatasetQuery {
            id: dataset.id.clone(),
            dimensions: vec!["vendor".to_string()],
            measures: vec!["qty_total".to_string()],
            order_by: vec![],
            dataset_filters: vec![],
            limit: None,
        });
        block.source = default_report_source();
        block.table = Some(ReportTableConfig {
            columns: vec![table_column("vendor"), table_column("qty_total")],
            selectable: false,
            actions: vec![],
            default_sort: vec![],
            pagination: None,
        });

        let compiled = compile_dataset_query(
            "vendor_summary",
            &dataset,
            &["vendor".to_string()],
            &["qty_total".to_string()],
            &[],
            None,
        )
        .unwrap();
        validate_dataset_block_output(&block, &compiled.source).unwrap();

        block.table = Some(ReportTableConfig {
            columns: vec![table_column("category")],
            selectable: false,
            actions: vec![],
            default_sort: vec![],
            pagination: None,
        });
        let err = validate_dataset_block_output(&block, &compiled.source).unwrap_err();
        assert!(
            err.to_string()
                .contains("unknown dataset table field 'category'")
        );
    }

    #[test]
    fn layout_validation_accepts_nested_layout_nodes() {
        let blocks = [test_metric_block("snapshots"), test_block("records")];
        let block_ids = blocks
            .iter()
            .map(|block| block.id.clone())
            .collect::<HashSet<_>>();
        let block_types = blocks
            .iter()
            .map(|block| (block.id.clone(), block.block_type))
            .collect::<HashMap<_, _>>();
        let filter_ids = HashSet::new();
        let mut layout_node_ids = HashSet::new();

        validate_layout_node(
            &json!({
                "id": "summary",
                "type": "section",
                "title": "Summary",
                "children": [
                    {"id": "summary_metrics", "type": "metric_row", "blocks": ["snapshots"]},
                    {"id": "records_block", "type": "block", "blockId": "records"}
                ]
            }),
            "$.layout[0]",
            &block_ids,
            &block_types,
            &filter_ids,
            &mut layout_node_ids,
        )
        .unwrap();
    }

    #[test]
    fn layout_validation_rejects_unknown_block_refs() {
        let block = test_metric_block("snapshots");
        let block_ids = HashSet::from([block.id.clone()]);
        let block_types = HashMap::from([(block.id.clone(), block.block_type)]);
        let filter_ids = HashSet::new();
        let mut layout_node_ids = HashSet::new();

        let error = validate_layout_node(
            &json!({
                "id": "missing",
                "type": "block",
                "blockId": "not_here"
            }),
            "$.layout[0]",
            &block_ids,
            &block_types,
            &filter_ids,
            &mut layout_node_ids,
        )
        .unwrap_err();

        assert!(error.to_string().contains("unknown block 'not_here'"));
    }

    #[test]
    fn layout_validation_accepts_show_when_filter_refs() {
        let block = test_block("case_summary");
        let block_ids = HashSet::from([block.id.clone()]);
        let block_types = HashMap::from([(block.id.clone(), block.block_type)]);
        let filter_ids = HashSet::from(["case_id".to_string()]);
        let mut layout_node_ids = HashSet::new();

        validate_layout_node(
            &json!({
                "id": "case_summary_node",
                "type": "block",
                "blockId": "case_summary",
                "showWhen": {"filter": "case_id", "exists": true}
            }),
            "$.layout[0]",
            &block_ids,
            &block_types,
            &filter_ids,
            &mut layout_node_ids,
        )
        .unwrap();
    }

    #[test]
    fn block_interactions_accept_filter_and_view_navigation_actions() {
        let mut block = test_block("cases");
        block.interactions = vec![ReportInteractionDefinition {
            id: "open_case".to_string(),
            trigger: ReportInteractionTrigger {
                event: "row_click".to_string(),
                field: None,
            },
            actions: vec![
                ReportInteractionAction {
                    action_type: "set_filter".to_string(),
                    filter_id: Some("case_id".to_string()),
                    filter_ids: vec![],
                    view_id: None,
                    value_from: Some("datum.case_id".to_string()),
                    value: None,
                },
                ReportInteractionAction {
                    action_type: "navigate_view".to_string(),
                    filter_id: None,
                    filter_ids: vec![],
                    view_id: Some("detail".to_string()),
                    value_from: None,
                    value: None,
                },
            ],
        }];

        validate_block_interactions(
            &block,
            &HashSet::from(["case_id".to_string()]),
            &HashSet::from(["detail".to_string()]),
        )
        .unwrap();
    }

    #[test]
    fn table_interaction_buttons_accept_filter_and_view_navigation_actions() {
        let column: ReportTableColumn = serde_json::from_value(json!({
            "field": "views",
            "label": "Views",
            "type": "interaction_buttons",
            "interactionButtons": [
                {
                    "id": "summary",
                    "label": "Summary",
                    "icon": "eye",
                    "actions": [
                        {"type": "set_filter", "filterId": "sku", "valueFrom": "datum.sku"},
                        {"type": "navigate_view", "viewId": "summary"}
                    ]
                },
                {
                    "id": "audit",
                    "label": "Audit",
                    "icon": "file_text",
                    "actions": [
                        {"type": "set_filter", "filterId": "sku", "valueFrom": "datum.sku"},
                        {"type": "navigate_view", "viewId": "audit"}
                    ]
                }
            ]
        }))
        .unwrap();

        assert!(column.is_interaction_buttons());
        validate_report_interaction_buttons(
            &column.interaction_buttons,
            &HashSet::from(["sku".to_string()]),
            &HashSet::from(["summary".to_string(), "audit".to_string()]),
            &|field| field == "sku",
            "block 'stock' interaction button column 'views'",
        )
        .unwrap();
    }

    #[test]
    fn aggregate_table_projection_allows_interaction_button_columns() {
        let mut actions_column = table_column("views");
        actions_column.column_type = Some(ReportTableColumnType::InteractionButtons);
        actions_column.interaction_buttons = vec![ReportTableInteractionButtonConfig {
            id: "summary".to_string(),
            label: Some("Summary".to_string()),
            icon: Some("eye".to_string()),
            visible_when: None,
            hidden_when: None,
            disabled_when: None,
            actions: vec![ReportInteractionAction {
                action_type: "navigate_view".to_string(),
                filter_id: None,
                filter_ids: vec![],
                view_id: Some("summary".to_string()),
                value_from: None,
                value: None,
            }],
        }];
        let table = ReportTableConfig {
            columns: vec![table_column("vendor"), actions_column],
            selectable: false,
            actions: vec![],
            default_sort: vec![],
            pagination: None,
        };

        let rows = project_aggregate_table_rows(
            Some(&table),
            &["vendor".to_string()],
            vec![vec![json!("ACME")]],
        )
        .unwrap();

        assert_eq!(rows, vec![vec![json!("ACME"), Value::Null]]);
    }

    #[test]
    fn json_merge_patch_updates_and_removes_fields() {
        let mut value = json!({
            "id": "orders",
            "title": "Orders",
            "table": { "columns": [{ "field": "id" }], "defaultSort": [] }
        });

        apply_json_merge_patch(
            &mut value,
            &json!({
                "title": "Recent orders",
                "table": { "defaultSort": null }
            }),
        );

        assert_eq!(value["title"], "Recent orders");
        assert!(value["table"].get("columns").is_some());
        assert!(value["table"].get("defaultSort").is_none());
    }

    fn cond(op: &str, args: Vec<Value>) -> Condition {
        Condition {
            op: op.to_string(),
            arguments: Some(args),
        }
    }

    fn report_filter(
        id: &str,
        filter_type: ReportFilterType,
        required: bool,
    ) -> ReportFilterDefinition {
        ReportFilterDefinition {
            id: id.to_string(),
            label: humanize_label(id),
            filter_type,
            default: None,
            required,
            strict_when_referenced: false,
            options: None,
            applies_to: vec![],
        }
    }

    fn filter_ref(filter_id: &str, path: &str) -> Value {
        json!({
            "filter": filter_id,
            "path": path,
        })
    }

    #[test]
    fn report_condition_resolves_scalar_filter_ref() {
        let filters = vec![report_filter("status", ReportFilterType::Select, true)];
        let filter_defs = report_filter_definitions_by_id(&filters);
        let values = HashMap::from([("status".to_string(), json!("open"))]);

        let resolved = resolve_report_condition_values(
            &cond("EQ", vec![json!("status"), filter_ref("status", "value")]),
            &filter_defs,
            &values,
            "test",
        )
        .unwrap()
        .unwrap();

        assert_eq!(
            resolved.arguments.unwrap(),
            vec![json!("status"), json!("open")]
        );
    }

    #[test]
    fn report_condition_resolves_multi_select_filter_ref() {
        let filters = vec![report_filter(
            "vendors",
            ReportFilterType::MultiSelect,
            false,
        )];
        let filter_defs = report_filter_definitions_by_id(&filters);
        let values = HashMap::from([("vendors".to_string(), json!(["HPE", "Cisco"]))]);

        let resolved = resolve_report_condition_values(
            &cond("IN", vec![json!("vendor"), filter_ref("vendors", "values")]),
            &filter_defs,
            &values,
            "test",
        )
        .unwrap()
        .unwrap();

        assert_eq!(
            resolved.arguments.unwrap(),
            vec![json!("vendor"), json!(["HPE", "Cisco"])]
        );
    }

    #[test]
    fn report_condition_resolves_filter_ref_inside_subquery() {
        let filters = vec![report_filter(
            "category",
            ReportFilterType::MultiSelect,
            true,
        )];
        let filter_defs = report_filter_definitions_by_id(&filters);
        let values = HashMap::from([("category".to_string(), json!(["leaf-a", "leaf-b"]))]);

        let resolved = resolve_report_condition_values(
            &cond(
                "IN",
                vec![
                    json!("sku"),
                    json!({
                        "subquery": {
                            "schema": "TDProduct",
                            "select": "sku",
                            "condition": {
                                "op": "IN",
                                "arguments": [
                                    "category_leaf_id",
                                    filter_ref("category", "values")
                                ]
                            }
                        }
                    }),
                ],
            ),
            &filter_defs,
            &values,
            "test",
        )
        .unwrap()
        .unwrap();

        let arguments = resolved.arguments.unwrap();
        assert_eq!(arguments[0], json!("sku"));
        assert_eq!(
            arguments[1]["subquery"]["condition"]["arguments"][1],
            json!(["leaf-a", "leaf-b"])
        );
    }

    #[test]
    fn report_condition_resolves_time_and_number_range_filter_refs() {
        let filters = vec![
            report_filter("period", ReportFilterType::TimeRange, true),
            report_filter("quantity", ReportFilterType::NumberRange, true),
        ];
        let filter_defs = report_filter_definitions_by_id(&filters);
        let values = HashMap::from([
            (
                "period".to_string(),
                json!({"from": "2026-01-01T00:00:00Z", "to": "2026-02-01T00:00:00Z"}),
            ),
            ("quantity".to_string(), json!({"min": 10, "max": 50})),
        ]);

        let from = resolve_report_condition_values(
            &cond(
                "GTE",
                vec![json!("created_at"), filter_ref("period", "from")],
            ),
            &filter_defs,
            &values,
            "test",
        )
        .unwrap()
        .unwrap();
        let max = resolve_report_condition_values(
            &cond("LTE", vec![json!("qty"), filter_ref("quantity", "max")]),
            &filter_defs,
            &values,
            "test",
        )
        .unwrap()
        .unwrap();

        assert_eq!(
            from.arguments.unwrap(),
            vec![json!("created_at"), json!("2026-01-01T00:00:00Z")]
        );
        assert_eq!(max.arguments.unwrap(), vec![json!("qty"), json!(50)]);
    }

    #[test]
    fn report_condition_rejects_in_operator_with_scalar_filter_ref() {
        let filters = vec![report_filter("status", ReportFilterType::Select, true)];
        let filter_defs = report_filter_definitions_by_id(&filters);
        let values = HashMap::from([("status".to_string(), json!("open"))]);

        let error = resolve_report_condition_values(
            &cond("IN", vec![json!("status"), filter_ref("status", "value")]),
            &filter_defs,
            &values,
            "test",
        )
        .unwrap_err();

        assert!(error.to_string().contains("requires an array value"));
    }

    #[test]
    fn report_condition_rejects_unknown_filter_ref() {
        let filter_defs = HashMap::new();

        let error = resolve_report_condition_values(
            &cond("EQ", vec![json!("status"), filter_ref("missing", "value")]),
            &filter_defs,
            &HashMap::new(),
            "test",
        )
        .unwrap_err();

        assert!(error.to_string().contains("unknown filter 'missing'"));
    }

    #[test]
    fn report_condition_prunes_optional_empty_filter_ref() {
        let filters = vec![report_filter("status", ReportFilterType::Select, false)];
        let filter_defs = report_filter_definitions_by_id(&filters);

        let resolved = resolve_report_condition_values(
            &cond("EQ", vec![json!("status"), filter_ref("status", "value")]),
            &filter_defs,
            &HashMap::new(),
            "test",
        )
        .unwrap();

        assert!(resolved.is_none());
    }

    #[test]
    fn virtual_system_aggregate_groups_sorts_and_evaluates_exprs() {
        let rows = vec![
            serde_json::Map::from_iter([
                ("bucketTime".to_string(), json!("2026-05-10T00:00:00Z")),
                ("invocationCount".to_string(), json!(10)),
                ("failureCount".to_string(), json!(1)),
            ]),
            serde_json::Map::from_iter([
                ("bucketTime".to_string(), json!("2026-05-10T00:00:00Z")),
                ("invocationCount".to_string(), json!(5)),
                ("failureCount".to_string(), json!(2)),
            ]),
            serde_json::Map::from_iter([
                ("bucketTime".to_string(), json!("2026-05-11T00:00:00Z")),
                ("invocationCount".to_string(), json!(20)),
                ("failureCount".to_string(), json!(0)),
            ]),
        ];
        let result = aggregate_virtual_rows(
            "usage",
            &rows,
            AggregateRequest {
                condition: None,
                group_by: vec!["bucketTime".to_string()],
                aggregates: vec![
                    AggregateSpec {
                        alias: "invocations".to_string(),
                        fn_: AggregateFn::Sum,
                        column: Some("invocationCount".to_string()),
                        distinct: false,
                        order_by: vec![],
                        expression: None,
                        percentile: None,
                    },
                    AggregateSpec {
                        alias: "failures".to_string(),
                        fn_: AggregateFn::Sum,
                        column: Some("failureCount".to_string()),
                        distinct: false,
                        order_by: vec![],
                        expression: None,
                        percentile: None,
                    },
                    AggregateSpec {
                        alias: "failureRate".to_string(),
                        fn_: AggregateFn::Expr,
                        column: None,
                        distinct: false,
                        order_by: vec![],
                        expression: Some(json!({
                            "op": "DIV",
                            "arguments": [
                                {"valueType": "alias", "value": "failures"},
                                {"valueType": "alias", "value": "invocations"}
                            ]
                        })),
                        percentile: None,
                    },
                ],
                order_by: vec![AggregateOrderBy {
                    column: "bucketTime".to_string(),
                    direction: SortDirection::Asc,
                }],
                limit: None,
                offset: None,
            },
        )
        .unwrap();

        assert_eq!(
            result.columns,
            vec!["bucketTime", "invocations", "failures", "failureRate"]
        );
        assert_eq!(result.group_count, 2);
        assert_eq!(result.rows[0][0], json!("2026-05-10T00:00:00Z"));
        assert_eq!(result.rows[0][1].as_f64(), Some(15.0));
        assert_eq!(result.rows[0][2].as_f64(), Some(3.0));
        assert_eq!(result.rows[0][3].as_f64(), Some(0.2));
        assert_eq!(result.rows[1][0], json!("2026-05-11T00:00:00Z"));
    }

    #[test]
    fn virtual_system_aggregate_applies_conditions_and_pagination() {
        let rows = vec![
            serde_json::Map::from_iter([
                ("connectionId".to_string(), json!("c1")),
                ("eventType".to_string(), json!("request")),
            ]),
            serde_json::Map::from_iter([
                ("connectionId".to_string(), json!("c1")),
                ("eventType".to_string(), json!("rate_limited")),
            ]),
            serde_json::Map::from_iter([
                ("connectionId".to_string(), json!("c2")),
                ("eventType".to_string(), json!("request")),
            ]),
        ];
        let result = aggregate_virtual_rows(
            "events",
            &rows,
            AggregateRequest {
                condition: Some(cond("EQ", vec![json!("connectionId"), json!("c1")])),
                group_by: vec!["eventType".to_string()],
                aggregates: vec![AggregateSpec {
                    alias: "count".to_string(),
                    fn_: AggregateFn::Count,
                    column: None,
                    distinct: false,
                    order_by: vec![],
                    expression: None,
                    percentile: None,
                }],
                order_by: vec![AggregateOrderBy {
                    column: "eventType".to_string(),
                    direction: SortDirection::Asc,
                }],
                limit: Some(1),
                offset: Some(1),
            },
        )
        .unwrap();

        assert_eq!(result.group_count, 2);
        assert_eq!(result.rows.len(), 1);
        assert_eq!(result.rows[0][0], json!("request"));
        assert_eq!(result.rows[0][1], json!(1));
    }

    #[test]
    fn filter_join_enrichment_adds_qualified_dimension_fields() {
        let join = ReportSourceJoin {
            schema: "TDProduct".to_string(),
            alias: Some("p".to_string()),
            connection_id: None,
            field: "sku".to_string(),
            parent_field: "sku".to_string(),
            op: "eq".to_string(),
            kind: ReportJoinKind::Left,
        };
        let alias_to_join = HashMap::from([("p".to_string(), &join)]);
        let mut dim_row = serde_json::Map::new();
        dim_row.insert("sku".to_string(), json!("ABC-1"));
        dim_row.insert("part_number".to_string(), json!("PN-1"));
        let join_data = HashMap::from([(
            "p".to_string(),
            JoinResolution {
                parent_keys: vec![json!("ABC-1")],
                by_key: HashMap::from([("ABC-1".to_string(), dim_row)]),
            },
        )]);
        let rows = vec![serde_json::Map::from_iter([(
            "sku".to_string(),
            json!("ABC-1"),
        )])];

        let enriched = enrich_filter_join_rows(&alias_to_join, &join_data, rows);

        assert_eq!(enriched[0].get("p.part_number"), Some(&json!("PN-1")));
    }

    #[test]
    fn filter_join_enrichment_drops_missing_inner_rows() {
        let join = ReportSourceJoin {
            schema: "TDProduct".to_string(),
            alias: Some("p".to_string()),
            connection_id: None,
            field: "sku".to_string(),
            parent_field: "sku".to_string(),
            op: "eq".to_string(),
            kind: ReportJoinKind::Inner,
        };
        let alias_to_join = HashMap::from([("p".to_string(), &join)]);
        let join_data = HashMap::from([(
            "p".to_string(),
            JoinResolution {
                parent_keys: vec![json!("ABC-1")],
                by_key: HashMap::new(),
            },
        )]);
        let rows = vec![serde_json::Map::from_iter([(
            "sku".to_string(),
            json!("ABC-1"),
        )])];

        let enriched = enrich_filter_join_rows(&alias_to_join, &join_data, rows);

        assert!(enriched.is_empty());
    }

    #[test]
    fn split_qualified_condition_separates_aliased_from_primary_terms() {
        let aliases: HashSet<&str> = ["p"].into_iter().collect();
        let condition = cond(
            "AND",
            vec![
                serde_json::to_value(cond("EQ", vec![json!("status"), json!("active")])).unwrap(),
                serde_json::to_value(cond(
                    "IN",
                    vec![json!("p.category_leaf_id"), json!([1, 2, 3])],
                ))
                .unwrap(),
            ],
        );

        let (primary, by_alias) =
            split_qualified_condition(Some(condition), &aliases, "block").unwrap();

        let primary = primary.expect("primary should retain the unqualified term");
        assert_eq!(primary.op, "EQ");
        assert_eq!(primary.arguments.unwrap()[0].as_str().unwrap(), "status");

        let alias_terms = by_alias.get("p").expect("alias bucket present");
        assert_eq!(alias_terms.len(), 1);
        assert_eq!(alias_terms[0].op, "IN");
    }

    #[test]
    fn split_qualified_condition_rejects_unknown_alias() {
        let aliases: HashSet<&str> = ["p"].into_iter().collect();
        let condition = cond("EQ", vec![json!("q.foo"), json!("bar")]);
        let err = split_qualified_condition(Some(condition), &aliases, "block").unwrap_err();
        assert!(err.to_string().contains("unknown join alias 'q'"));
    }

    #[test]
    fn strip_alias_from_condition_removes_qualifier() {
        let stripped = strip_alias_from_condition(
            cond("IN", vec![json!("p.category_leaf_id"), json!([1, 2, 3])]),
            "p",
        );
        assert_eq!(
            stripped.arguments.unwrap()[0].as_str().unwrap(),
            "category_leaf_id"
        );
    }

    #[test]
    fn enrich_aggregate_result_appends_dim_columns_in_groupby_order() {
        let primary = runtara_object_store::AggregateResult {
            columns: vec!["sku".to_string(), "delta".to_string()],
            rows: vec![
                vec![json!("ABC-1"), json!(5)],
                vec![json!("ABC-2"), json!(-3)],
            ],
            group_count: 2,
        };

        let join = ReportSourceJoin {
            schema: "TDProduct".to_string(),
            alias: Some("p".to_string()),
            connection_id: None,
            field: "sku".to_string(),
            parent_field: "sku".to_string(),
            op: "eq".to_string(),
            kind: ReportJoinKind::Inner,
        };
        let joins = [join];
        let alias_to_join: HashMap<String, &ReportSourceJoin> =
            joins.iter().map(|j| ("p".to_string(), j)).collect();

        let mut by_key: HashMap<String, serde_json::Map<String, Value>> = HashMap::new();
        let mut row1 = serde_json::Map::new();
        row1.insert("sku".to_string(), json!("ABC-1"));
        row1.insert("vendor".to_string(), json!("HPE"));
        row1.insert("part_number".to_string(), json!("X-1"));
        by_key.insert("ABC-1".to_string(), row1);
        let mut row2 = serde_json::Map::new();
        row2.insert("sku".to_string(), json!("ABC-2"));
        row2.insert("vendor".to_string(), json!("Cisco"));
        row2.insert("part_number".to_string(), json!("Y-2"));
        by_key.insert("ABC-2".to_string(), row2);

        let mut join_data: HashMap<String, JoinResolution> = HashMap::new();
        join_data.insert(
            "p".to_string(),
            JoinResolution {
                parent_keys: vec![json!("ABC-1"), json!("ABC-2")],
                by_key,
            },
        );

        let requested_group_by = vec![
            "sku".to_string(),
            "p.part_number".to_string(),
            "p.vendor".to_string(),
        ];

        let enriched =
            enrich_aggregate_result(primary, &requested_group_by, &alias_to_join, &join_data);

        assert_eq!(
            enriched.columns,
            vec![
                "sku".to_string(),
                "p.part_number".to_string(),
                "p.vendor".to_string(),
                "delta".to_string(),
            ]
        );
        assert_eq!(enriched.rows.len(), 2);
        assert_eq!(enriched.rows[0][0], json!("ABC-1"));
        assert_eq!(enriched.rows[0][1], json!("X-1"));
        assert_eq!(enriched.rows[0][2], json!("HPE"));
        assert_eq!(enriched.rows[0][3], json!(5));
        assert_eq!(enriched.rows[1][2], json!("Cisco"));
    }

    #[test]
    fn validate_join_request_rejects_qualified_aggregate_field() {
        let join = ReportSourceJoin {
            schema: "TDProduct".to_string(),
            alias: Some("p".to_string()),
            connection_id: None,
            field: "sku".to_string(),
            parent_field: "sku".to_string(),
            op: "eq".to_string(),
            kind: ReportJoinKind::Inner,
        };
        let joins = [join];
        let alias_to_join: HashMap<String, &ReportSourceJoin> =
            joins.iter().map(|j| ("p".to_string(), j)).collect();

        let request = AggregateRequest {
            condition: None,
            group_by: vec!["sku".to_string()],
            aggregates: vec![AggregateSpec {
                alias: "vendor_sample".to_string(),
                fn_: AggregateFn::Max,
                column: Some("p.vendor".to_string()),
                distinct: false,
                order_by: vec![],
                expression: None,
                percentile: None,
            }],
            order_by: vec![],
            limit: Some(10),
            offset: Some(0),
        };
        let err = validate_join_request(&request, &alias_to_join, "block").unwrap_err();
        assert!(
            err.to_string()
                .contains("qualified refs in aggregate.field are not supported")
        );
    }

    #[test]
    fn validate_join_request_requires_parent_field_in_groupby_when_qualified_used() {
        let join = ReportSourceJoin {
            schema: "TDProduct".to_string(),
            alias: Some("p".to_string()),
            connection_id: None,
            field: "sku".to_string(),
            parent_field: "sku".to_string(),
            op: "eq".to_string(),
            kind: ReportJoinKind::Inner,
        };
        let joins = [join];
        let alias_to_join: HashMap<String, &ReportSourceJoin> =
            joins.iter().map(|j| ("p".to_string(), j)).collect();

        let request = AggregateRequest {
            condition: None,
            group_by: vec!["p.vendor".to_string()],
            aggregates: vec![AggregateSpec {
                alias: "n".to_string(),
                fn_: AggregateFn::Count,
                column: None,
                distinct: false,
                order_by: vec![],
                expression: None,
                percentile: None,
            }],
            order_by: vec![],
            limit: Some(10),
            offset: Some(0),
        };
        let err = validate_join_request(&request, &alias_to_join, "block").unwrap_err();
        assert!(err.to_string().contains("parent field 'sku'"));
    }
}
