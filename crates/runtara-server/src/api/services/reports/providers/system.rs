//! System source provider — runtime metric buckets, system snapshot, and
//! connection rate-limit data. Pulls from [`RuntimeClient`] and the
//! connections facade; never touches Postgres directly. Aggregates are
//! always virtual (no pushdown).

use async_trait::async_trait;
use serde_json::{Map, Value};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::api::dto::object_model::AggregateRequest;
use crate::api::dto::reports::*;
use crate::api::services::reports::ReportServiceError;
use crate::runtime_client::RuntimeClient;

use super::{FetchAggregateOutput, FetchParams, FetchRowsOutput, ReportSourceProvider};

pub struct SystemProvider {
    runtime_client: Option<Arc<RuntimeClient>>,
    connections: Arc<runtara_connections::ConnectionsFacade>,
}

impl SystemProvider {
    pub fn new(
        runtime_client: Option<Arc<RuntimeClient>>,
        connections: Arc<runtara_connections::ConnectionsFacade>,
    ) -> Self {
        Self {
            runtime_client,
            connections,
        }
    }

    pub(crate) fn runtime_client(&self) -> Option<&Arc<RuntimeClient>> {
        self.runtime_client.as_ref()
    }

    pub(crate) fn connections(&self) -> &Arc<runtara_connections::ConnectionsFacade> {
        &self.connections
    }
}

#[async_trait]
impl ReportSourceProvider for SystemProvider {
    fn kind(&self) -> ReportSourceKind {
        ReportSourceKind::System
    }

    async fn fetch_rows(
        &self,
        params: FetchParams<'_>,
    ) -> Result<FetchRowsOutput, ReportServiceError> {
        let rows = crate::api::services::reports::system_provider_fetch_rows(
            self,
            params.tenant_id,
            params.block,
            params.condition,
        )
        .await?;
        Ok(FetchRowsOutput {
            rows,
            total_count: None,
        })
    }

    async fn fetch_aggregate(
        &self,
        params: FetchParams<'_>,
        request: AggregateRequest,
    ) -> Result<FetchAggregateOutput, ReportServiceError> {
        let rows: Vec<Map<String, Value>> =
            crate::api::services::reports::system_provider_fetch_rows(
                self,
                params.tenant_id,
                params.block,
                params.condition,
            )
            .await?;
        let result = crate::api::services::reports::aggregate_virtual_rows(
            &params.block.id,
            &rows,
            request,
        )?;
        Ok(FetchAggregateOutput::from(result))
    }

    fn validate_block(
        &self,
        block: &ReportBlockDefinition,
        filter_ids: &HashSet<String>,
        view_ids: &HashSet<String>,
        filter_defs: &HashMap<String, &ReportFilterDefinition>,
    ) -> Result<(), ReportServiceError> {
        crate::api::services::reports::validate_system_block(
            block,
            filter_ids,
            view_ids,
            filter_defs,
        )
    }

    fn field_is_known(&self, block: &ReportBlockDefinition, field: &str) -> bool {
        let Ok(entity) = crate::api::services::reports::system_entity(block) else {
            return false;
        };
        let fields = crate::api::services::reports::system_fields(entity);
        crate::api::services::reports::system_row_field_known(&fields, field)
    }

    fn table_columns(
        &self,
        block: &ReportBlockDefinition,
    ) -> Result<Vec<Value>, ReportServiceError> {
        let entity = crate::api::services::reports::system_entity(block)?;
        Ok(crate::api::services::reports::system_table_columns(
            block.table.as_ref(),
            entity,
        ))
    }
}
