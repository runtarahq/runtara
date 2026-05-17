//! Object-model source provider — wraps `ObjectStoreManager` via
//! `InstanceService`. Aggregates and filters push down to storage.

use async_trait::async_trait;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::api::dto::object_model::AggregateRequest;
use crate::api::dto::reports::*;
use crate::api::repositories::object_model::ObjectStoreManager;
use crate::api::services::object_model::InstanceService;
use crate::api::services::reports::ReportServiceError;

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

    pub(crate) fn instance_service(&self) -> &InstanceService {
        &self.instance_service
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
        crate::api::services::reports::object_model_provider_fetch_rows(
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
        crate::api::services::reports::object_model_provider_fetch_aggregate(
            self,
            params.tenant_id,
            params.block,
            request,
        )
        .await
    }

    fn validate_block(
        &self,
        _block: &ReportBlockDefinition,
        _filter_ids: &HashSet<String>,
        _view_ids: &HashSet<String>,
        _filter_defs: &HashMap<String, &ReportFilterDefinition>,
    ) -> Result<(), ReportServiceError> {
        // Object-model blocks are validated by the generic
        // `validate_report_definition` path against the dynamically-loaded
        // schema. The legacy code keeps that machinery in `reports.rs`; the
        // provider is a no-op until the schema-aware validator moves here.
        Ok(())
    }

    fn field_is_known(&self, _block: &ReportBlockDefinition, _field: &str) -> bool {
        // The schema is loaded async per-tenant — the validator does the real
        // check. Default to `true` so the per-source field guards in
        // `validate_report_definition` don't reject object-model fields.
        true
    }

    fn supports_aggregate_pushdown(&self) -> bool {
        true
    }

    fn table_columns(
        &self,
        block: &ReportBlockDefinition,
    ) -> Result<Vec<serde_json::Value>, ReportServiceError> {
        Ok(crate::api::services::reports::table_response_columns(
            block.table.as_ref(),
        ))
    }
}
