//! Object-model source provider — wraps `ObjectStoreManager` via
//! `InstanceService`. Aggregates and filters push down to storage.
//!
//! `validate_block` is currently a no-op: object-model definitions are
//! validated by the generic `validate_report_definition` path in
//! `reports.rs` against the dynamically-loaded schema. The schema-aware
//! validator will move here in Phase 5.

use async_trait::async_trait;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::api::dto::object_model::{AggregateRequest, Condition, FilterRequest};
use crate::api::dto::reports::*;
use crate::api::repositories::object_model::ObjectStoreManager;
use crate::api::services::object_model::InstanceService;
use crate::api::services::reports::{
    ReportServiceError, flatten_instance, map_object_model_error, normalize_sort_direction,
    table_response_columns,
};

use super::{FetchAggregateOutput, FetchParams, FetchRowsOutput, ReportSourceProvider};

pub struct ObjectModelProvider {
    instance_service: InstanceService,
}

impl ObjectModelProvider {
    pub fn new(
        manager: Arc<ObjectStoreManager>,
        connections: Arc<runtara_connections::ConnectionsFacade>,
    ) -> Self {
        Self {
            instance_service: InstanceService::new(manager, connections),
        }
    }
}

#[async_trait]
impl ReportSourceProvider for ObjectModelProvider {
    fn kind(&self) -> ReportSourceKind {
        ReportSourceKind::ObjectModel
    }

    async fn fetch_rows(
        &self,
        params: FetchParams<'_>,
    ) -> Result<FetchRowsOutput, ReportServiceError> {
        fetch_rows_inner(
            self,
            params.tenant_id,
            params.block,
            params.condition,
            params.sort,
            params.offset,
            params.limit,
        )
        .await
    }

    async fn fetch_aggregate(
        &self,
        params: FetchParams<'_>,
        request: AggregateRequest,
    ) -> Result<FetchAggregateOutput, ReportServiceError> {
        let result = self
            .instance_service
            .aggregate_instances_by_schema(
                params.tenant_id,
                &params.block.source.schema,
                request,
                params.block.source.connection_id.as_deref(),
            )
            .await
            .map_err(map_object_model_error)?;
        Ok(FetchAggregateOutput::from(result))
    }

    fn validate_block(
        &self,
        _block: &ReportBlockDefinition,
        _filter_ids: &HashSet<String>,
        _view_ids: &HashSet<String>,
        _filter_defs: &HashMap<String, &ReportFilterDefinition>,
    ) -> Result<(), ReportServiceError> {
        Ok(())
    }

    fn field_is_known(&self, _block: &ReportBlockDefinition, _field: &str) -> bool {
        true
    }

    fn supports_aggregate_pushdown(&self) -> bool {
        true
    }

    fn table_columns(
        &self,
        block: &ReportBlockDefinition,
    ) -> Result<Vec<Value>, ReportServiceError> {
        Ok(table_response_columns(block.table.as_ref()))
    }
}

async fn fetch_rows_inner(
    provider: &ObjectModelProvider,
    tenant_id: &str,
    block: &ReportBlockDefinition,
    condition: Option<&Condition>,
    sort: &[ReportOrderBy],
    offset: i64,
    limit: i64,
) -> Result<FetchRowsOutput, ReportServiceError> {
    let filter_request = FilterRequest {
        offset,
        limit,
        condition: condition.cloned(),
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

    let (instances, total_count) = provider
        .instance_service
        .filter_instances_by_schema(
            tenant_id,
            &block.source.schema,
            filter_request,
            block.source.connection_id.as_deref(),
        )
        .await
        .map_err(map_object_model_error)?;

    let rows = instances
        .into_iter()
        .map(flatten_instance)
        .filter_map(|value| match value {
            Value::Object(map) => Some(map),
            _ => None,
        })
        .collect();
    Ok(FetchRowsOutput {
        rows,
        total_count: Some(total_count),
    })
}
