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

const MAX_TABLE_PAGE_SIZE: i64 = 500;
const MAX_AGGREGATE_ROWS: i64 = 1000;

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

        let mut block_ids = HashSet::new();
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
            let is_known_field = |field: &str| -> bool { is_schema_field(&schema_fields, field) };
            let aggregate_output_fields = aggregate_output_fields(block);
            let is_table_value_field = |field: &str| -> bool {
                match block.source.mode {
                    ReportSourceMode::Filter => is_known_field(field),
                    ReportSourceMode::Aggregate => aggregate_output_fields.contains(field),
                }
            };

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

    async fn render_block(
        &self,
        tenant_id: &str,
        definition: &ReportDefinition,
        block: &ReportBlockDefinition,
        resolved_filters: &HashMap<String, Value>,
        block_request: Option<&ReportBlockDataRequest>,
    ) -> Result<ReportBlockRenderResult, ReportServiceError> {
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
            condition: build_block_condition(definition, block, resolved_filters, block_request),
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
            .hydrate_table_chart_columns(tenant_id, table, rows)
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
            .instance_service
            .aggregate_instances_by_schema(
                tenant_id,
                &block.source.schema,
                request,
                block.source.connection_id.as_deref(),
            )
            .await
            .map_err(map_object_model_error)?;

        let source_columns = result.columns.clone();
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
                            .render_table_chart_cell(tenant_id, chart_column, row_map)
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
        if chart_columns.is_empty() {
            return Ok(rows);
        }

        let mut hydrated_rows = Vec::with_capacity(rows.len());
        for row in rows {
            let Value::Object(mut object) = row else {
                hydrated_rows.push(row);
                continue;
            };
            for column in &chart_columns {
                let cell = self
                    .render_table_chart_cell(tenant_id, column, &object)
                    .await?;
                object.insert(column.field.clone(), cell);
            }
            hydrated_rows.push(Value::Object(object));
        }

        Ok(hydrated_rows)
    }

    async fn render_table_chart_cell(
        &self,
        tenant_id: &str,
        column: &ReportTableColumn,
        row: &serde_json::Map<String, Value>,
    ) -> Result<Value, ReportServiceError> {
        let Some(source) = &column.source else {
            return Ok(Value::Null);
        };
        let condition = build_table_column_condition(source, row);
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
            .instance_service
            .aggregate_instances_by_schema(
                tenant_id,
                &block.source.schema,
                request,
                block.source.connection_id.as_deref(),
            )
            .await
            .map_err(map_object_model_error)?;

        Ok(json!({
            "columns": result.columns,
            "rows": result.rows,
            "groupCount": result.group_count,
        }))
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

fn build_block_condition(
    definition: &ReportDefinition,
    block: &ReportBlockDefinition,
    resolved_filters: &HashMap<String, Value>,
    block_request: Option<&ReportBlockDataRequest>,
) -> Option<Condition> {
    let mut conditions = Vec::new();

    if let Some(condition) = &block.source.condition {
        conditions.push(condition.clone());
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

    combine_conditions(conditions)
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
        build_block_condition(definition, block, resolved_filters, None),
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
        build_block_condition(definition, block, resolved_filters, block_request),
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

fn build_table_column_condition(
    source: &ReportTableColumnSource,
    row: &serde_json::Map<String, Value>,
) -> Option<Condition> {
    let mut conditions = Vec::new();
    if let Some(condition) = &source.condition {
        conditions.push(condition.clone());
    }
    for join in &source.join {
        let Some(value) = row.get(&join.parent_field) else {
            continue;
        };
        if let Some(condition) = condition_from_table_column_join(join, value) {
            conditions.push(condition);
        }
    }
    combine_conditions(conditions)
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
            },
            table: None,
            chart: None,
            metric: None,
            filters: vec![],
        }
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
}
