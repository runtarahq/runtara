use chrono::{DateTime, Datelike, Duration, NaiveTime, Utc};
use regex::Regex;
use serde_json::{Value, json};
use sqlx::PgPool;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use thiserror::Error;
use uuid::Uuid;

use crate::api::dto::object_model::{
    AggregateFn, AggregateOrderBy, AggregateRequest, AggregateSpec, Condition, FilterRequest,
    SortDirection,
};
use crate::api::dto::reports::*;
use crate::api::repositories::object_model::ObjectStoreManager;
use crate::api::repositories::reports::ReportRepository;
use crate::api::services::object_model::{
    InstanceService, SchemaService, ServiceError as ObjectModelServiceError,
};

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
    Conflict(String),
    #[error("{0}")]
    Database(String),
}

pub struct ReportService {
    repo: ReportRepository,
    schema_service: SchemaService,
    instance_service: InstanceService,
}

#[derive(Clone, Copy)]
struct ReportConditionRuntimeContext<'a> {
    definition: &'a ReportDefinition,
    block: &'a ReportBlockDefinition,
    resolved_filters: &'a HashMap<String, Value>,
    block_request: Option<&'a ReportBlockDataRequest>,
}

impl ReportService {
    pub fn new(
        pool: PgPool,
        manager: Arc<ObjectStoreManager>,
        connections: Arc<runtara_connections::ConnectionsFacade>,
    ) -> Self {
        Self {
            repo: ReportRepository::new(pool),
            schema_service: SchemaService::new(manager.clone(), connections.clone()),
            instance_service: InstanceService::new(manager, connections),
        }
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
        if request.insert_markdown_placeholder {
            insert_markdown_placeholder(
                &mut report.definition.markdown,
                &block_id,
                Some(&position),
            );
        }

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

        if request.move_markdown_placeholder {
            move_markdown_placeholder(
                &mut report.definition.markdown,
                block_id,
                Some(&request.position),
            );
        }

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
        request: RemoveReportBlockRequest,
    ) -> Result<ReportBlockMutationResponse, ReportServiceError> {
        let mut report = self.get_report(tenant_id, id_or_slug).await?;
        let index = find_block_index(&report.definition.blocks, block_id)?;
        report.definition.blocks.remove(index);

        if request.remove_markdown_placeholder {
            remove_markdown_placeholder(&mut report.definition.markdown, block_id);
        }

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
            Err(ReportServiceError::Validation(message)) => ValidateReportResponse {
                valid: false,
                errors: vec![ReportValidationIssue {
                    path: "$".to_string(),
                    code: "VALIDATION_ERROR".to_string(),
                    message,
                }],
                warnings: vec![],
            },
            Err(error) => ValidateReportResponse {
                valid: false,
                errors: vec![ReportValidationIssue {
                    path: "$".to_string(),
                    code: "VALIDATION_ERROR".to_string(),
                    message: error.to_string(),
                }],
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
        if option_bool(options_config, "search").unwrap_or(false) && !search_query.is_empty() {
            conditions.push(binary_condition(
                "CONTAINS",
                Value::String(label_field.clone()),
                Value::String(search_query.to_string()),
            ));
        }

        let mut group_by = vec![field.clone()];
        if label_field != field {
            group_by.push(label_field.clone());
        }

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
                column: label_field.clone(),
                direction: SortDirection::Asc,
            }],
            limit: Some(limit),
            offset: Some(offset),
        };

        let result = self
            .instance_service
            .aggregate_instances_by_schema(tenant_id, &schema, aggregate_request, connection_id)
            .await
            .map_err(map_object_model_error)?;
        let value_index = result
            .columns
            .iter()
            .position(|column| column == &field)
            .unwrap_or(0);
        let label_index = result
            .columns
            .iter()
            .position(|column| column == &label_field)
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

        Ok(ReportFilterOptionsResponse {
            success: true,
            filter: ReportFilterOptionsMetadata {
                id: filter.id.clone(),
            },
            options,
            page: ReportFilterOptionsPage {
                offset,
                size: limit,
                total_count: result.group_count,
                has_next_page: offset + limit < result.group_count,
            },
        })
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
        if definition.markdown.len() > 250_000 {
            return Err(ReportServiceError::Validation(
                "Report markdown is too large".to_string(),
            ));
        }

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
            self.validate_filter_options(tenant_id, filter).await?;
        }
        let report_condition_filter_defs = report_filter_definitions_by_id(&definition.filters);
        for filter in &definition.filters {
            validate_filter_option_condition_filter_refs(filter, &report_condition_filter_defs)?;
        }

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
            block_types.insert(block.id.clone(), block.block_type);
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
                validate_block_interactions(block, &filter_ids)?;
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
            let block_condition_filter_defs = block_condition_filter_definitions(definition, block);

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
                    } else if !is_table_value_field(&column.field) {
                        return Err(ReportServiceError::Validation(format!(
                            "Block '{}' references unknown table field '{}'",
                            block.id, column.field
                        )));
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
            if let Some(table) = &block.table {
                for column in &table.columns {
                    if let Some(source) = &column.source {
                        validate_report_condition_filter_refs(
                            source.condition.as_ref(),
                            &block_condition_filter_defs,
                            &format!("block '{}' table column '{}'", block.id, column.field),
                        )?;
                    }
                }
            }

            validate_block_interactions(block, &filter_ids)?;
        }

        let mut layout_node_ids = HashSet::new();
        for (index, node) in definition.layout.iter().enumerate() {
            validate_layout_node(
                node,
                &format!("$.layout[{index}]"),
                &block_ids,
                &block_types,
                &mut layout_node_ids,
            )?;
        }

        let placeholder_ids = extract_markdown_block_placeholders(&definition.markdown);
        for placeholder in placeholder_ids {
            if !block_ids.contains(&placeholder) {
                return Err(ReportServiceError::Validation(format!(
                    "Markdown references unknown report block '{}'",
                    placeholder
                )));
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
            ReportBlockType::Markdown => json!({}),
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
        let columns = table
            .map(|table| {
                table
                    .columns
                    .iter()
                    .map(|column| {
                        json!({
                            "key": column.field,
                            "label": column.label.clone().unwrap_or_else(|| humanize_label(&column.field)),
                            "format": column.format,
                        })
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
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

        let columns = table
            .map(|table| {
                table
                    .columns
                    .iter()
                    .map(|column| {
                        json!({
                            "key": column.field,
                            "label": column.label.clone().unwrap_or_else(|| humanize_label(&column.field)),
                            "format": column.format,
                        })
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

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
        if chart_columns.is_empty() && value_columns.is_empty() {
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
}

fn validate_layout_node(
    node: &Value,
    path: &str,
    block_ids: &HashSet<String>,
    block_types: &HashMap<String, ReportBlockType>,
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

    match node_type {
        "markdown" => {
            if object.get("content").and_then(Value::as_str).is_none() {
                return Err(ReportServiceError::Validation(format!(
                    "Markdown layout node '{}' must include content",
                    node_id
                )));
            }
        }
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

fn validate_block_interactions(
    block: &ReportBlockDefinition,
    filter_ids: &HashSet<String>,
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
            if action.action_type == "set_filter" {
                let Some(filter_id) = action.filter_id.as_deref() else {
                    return Err(ReportServiceError::Validation(format!(
                        "Block '{}' interaction '{}' set_filter action must include filterId",
                        block.id, interaction.id
                    )));
                };
                if !filter_ids.contains(filter_id) {
                    return Err(ReportServiceError::Validation(format!(
                        "Block '{}' interaction '{}' references unknown filter '{}'",
                        block.id, interaction.id, filter_id
                    )));
                }
                if action.value_from.is_none() && action.value.is_none() {
                    return Err(ReportServiceError::Validation(format!(
                        "Block '{}' interaction '{}' set_filter action must include value or valueFrom",
                        block.id, interaction.id
                    )));
                }
            }
        }
    }

    Ok(())
}

fn validate_layout_children(
    children: &Value,
    path: &str,
    block_ids: &HashSet<String>,
    block_types: &HashMap<String, ReportBlockType>,
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
        if let Some(child) = condition_from_value(argument) {
            validate_report_condition_filter_refs(Some(&child), filter_defs, context)?;
        }
    }
    Ok(())
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
        && resolved_arguments
            .get(1)
            .is_some_and(|argument| !argument.is_array())
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
        .filter(|column| {
            requested_fields.is_empty() || requested_fields.contains(column.field.as_str())
        })
        .filter_map(|column| {
            if seen.insert(column.field.clone()) {
                Some(column.field.clone())
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
            schema: dataset.source.schema.clone(),
            connection_id: dataset.source.connection_id.clone(),
            mode: ReportSourceMode::Aggregate,
            condition: None,
            filter_mappings: vec![],
            group_by,
            aggregates,
            order_by: order_by.to_vec(),
            limit: limit.map(|value| value.clamp(1, MAX_AGGREGATE_ROWS)),
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
                    if column.is_chart() {
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

fn is_empty_data(data: &Value) -> bool {
    data.get("rows")
        .and_then(Value::as_array)
        .is_some_and(|rows| rows.is_empty())
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

fn extract_markdown_block_placeholders(markdown: &str) -> Vec<String> {
    let mut placeholders = Vec::new();
    let mut rest = markdown;

    while let Some(start) = rest.find("{{") {
        rest = &rest[start + 2..];
        let Some(end) = rest.find("}}") else {
            break;
        };
        let candidate = rest[..end].trim();
        if let Some(block_id) = candidate.strip_prefix("block.") {
            let block_id = block_id.trim();
            if !block_id.is_empty() {
                placeholders.push(block_id.to_string());
            }
        }
        rest = &rest[end + 2..];
    }

    placeholders
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

fn placeholder_text(block_id: &str) -> String {
    format!("{{{{ block.{} }}}}", block_id)
}

fn find_placeholder_range(markdown: &str, block_id: &str) -> Option<std::ops::Range<usize>> {
    let pattern = format!(r"\{{\{{\s*block\.{}\s*\}}\}}", regex::escape(block_id));
    Regex::new(&pattern).ok()?.find(markdown).map(|m| m.range())
}

fn all_placeholder_ranges(markdown: &str) -> Vec<(String, std::ops::Range<usize>)> {
    let Ok(regex) = Regex::new(r"\{\{\s*block\.([^}]+?)\s*\}\}") else {
        return Vec::new();
    };

    regex
        .captures_iter(markdown)
        .filter_map(|captures| {
            let block_id = captures.get(1)?.as_str().trim().to_string();
            let range = captures.get(0)?.range();
            if block_id.is_empty() {
                None
            } else {
                Some((block_id, range))
            }
        })
        .collect()
}

fn insert_markdown_placeholder(
    markdown: &mut String,
    block_id: &str,
    position: Option<&ReportBlockPosition>,
) {
    if find_placeholder_range(markdown, block_id).is_some() {
        return;
    }

    let placeholder = placeholder_text(block_id);

    if let Some(position) = position {
        if let Some(before_block_id) = &position.before_block_id
            && let Some(range) = find_placeholder_range(markdown, before_block_id)
        {
            insert_placeholder_at(markdown, range.start, &placeholder);
            return;
        }

        if let Some(after_block_id) = &position.after_block_id
            && let Some(range) = find_placeholder_range(markdown, after_block_id)
        {
            insert_placeholder_at(markdown, range.end, &placeholder);
            return;
        }

        if let Some(index) = position.index {
            let placeholders = all_placeholder_ranges(markdown);
            if let Some((_, range)) = placeholders.get(index) {
                insert_placeholder_at(markdown, range.start, &placeholder);
                return;
            }
        }
    }

    append_placeholder(markdown, &placeholder);
}

fn insert_placeholder_at(markdown: &mut String, index: usize, placeholder: &str) {
    let mut insertion = String::new();
    if index > 0 && !markdown[..index].ends_with("\n\n") {
        insertion.push_str(if markdown[..index].ends_with('\n') {
            "\n"
        } else {
            "\n\n"
        });
    }
    insertion.push_str(placeholder);
    if index < markdown.len() && !markdown[index..].starts_with("\n\n") {
        insertion.push_str(if markdown[index..].starts_with('\n') {
            "\n"
        } else {
            "\n\n"
        });
    }

    markdown.insert_str(index, &insertion);
    compact_blank_lines(markdown);
}

fn append_placeholder(markdown: &mut String, placeholder: &str) {
    if markdown.trim().is_empty() {
        *markdown = placeholder.to_string();
        return;
    }

    if !markdown.ends_with('\n') {
        markdown.push('\n');
    }
    if !markdown.ends_with("\n\n") {
        markdown.push('\n');
    }
    markdown.push_str(placeholder);
}

fn move_markdown_placeholder(
    markdown: &mut String,
    block_id: &str,
    position: Option<&ReportBlockPosition>,
) {
    if find_placeholder_range(markdown, block_id).is_none() {
        return;
    }

    remove_markdown_placeholder(markdown, block_id);
    insert_markdown_placeholder(markdown, block_id, position);
}

fn remove_markdown_placeholder(markdown: &mut String, block_id: &str) {
    let Some(range) = find_placeholder_range(markdown, block_id) else {
        return;
    };

    markdown.replace_range(range, "");
    compact_blank_lines(markdown);
    *markdown = markdown.trim().to_string();
}

fn compact_blank_lines(markdown: &mut String) {
    while markdown.contains("\n\n\n") {
        *markdown = markdown.replace("\n\n\n", "\n\n");
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

#[cfg(test)]
mod tests {
    use super::*;

    fn test_block(id: &str) -> ReportBlockDefinition {
        ReportBlockDefinition {
            id: id.to_string(),
            block_type: ReportBlockType::Table,
            title: None,
            lazy: false,
            dataset: None,
            source: ReportSource {
                schema: "Order".to_string(),
                connection_id: None,
                mode: ReportSourceMode::Filter,
                condition: None,
                filter_mappings: vec![],
                group_by: vec![],
                aggregates: vec![],
                order_by: vec![],
                limit: None,
                join: vec![],
            },
            table: None,
            chart: None,
            metric: None,
            filters: vec![],
            interactions: vec![],
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
            format: None,
            column_type: None,
            chart: None,
            source: None,
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
    fn moves_markdown_placeholder_to_requested_position() {
        let mut markdown =
            "# Report\n\n{{ block.a }}\n\n{{ block.b }}\n\n{{ block.c }}".to_string();

        move_markdown_placeholder(
            &mut markdown,
            "c",
            Some(&ReportBlockPosition {
                before_block_id: Some("a".to_string()),
                ..Default::default()
            }),
        );

        assert!(markdown.find("{{ block.c }}") < markdown.find("{{ block.a }}"));
        assert_eq!(
            extract_markdown_block_placeholders(&markdown),
            vec!["c".to_string(), "a".to_string(), "b".to_string()]
        );
    }

    #[test]
    fn table_search_condition_uses_configured_columns() {
        let mut block = test_block("orders");
        block.table = Some(ReportTableConfig {
            columns: vec![table_column("customer_name"), table_column("status")],
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
            markdown: String::new(),
            layout: vec![],
            filters: vec![ReportFilterDefinition {
                id: "vendor".to_string(),
                label: "Vendor".to_string(),
                filter_type: ReportFilterType::Select,
                default: None,
                required: false,
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
                markdown: String::new(),
                layout: vec![],
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
            default_sort: vec![],
            pagination: None,
        });
        let definition = ReportDefinition {
            definition_version: 1,
            markdown: String::new(),
            layout: vec![],
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
            &mut layout_node_ids,
        )
        .unwrap();
    }

    #[test]
    fn layout_validation_rejects_unknown_block_refs() {
        let block = test_metric_block("snapshots");
        let block_ids = HashSet::from([block.id.clone()]);
        let block_types = HashMap::from([(block.id.clone(), block.block_type)]);
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
            &mut layout_node_ids,
        )
        .unwrap_err();

        assert!(error.to_string().contains("unknown block 'not_here'"));
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
