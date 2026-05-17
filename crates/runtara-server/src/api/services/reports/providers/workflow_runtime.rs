//! Workflow runtime source provider — `Instances` + `Actions` entities.
//! Pulls from the execution engine + runtime client. Aggregates are always
//! virtual (no pushdown).

use async_trait::async_trait;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::api::dto::object_model::AggregateRequest;
use crate::api::dto::reports::*;
use crate::api::services::reports::ReportServiceError;
use crate::runtime_client::RuntimeClient;
use crate::workers::execution_engine::ExecutionEngine;

use super::{FetchAggregateOutput, FetchParams, FetchRowsOutput, ReportSourceProvider};

pub struct WorkflowRuntimeProvider {
    engine: Option<Arc<ExecutionEngine>>,
    runtime_client: Option<Arc<RuntimeClient>>,
}

impl WorkflowRuntimeProvider {
    pub fn new(
        engine: Option<Arc<ExecutionEngine>>,
        runtime_client: Option<Arc<RuntimeClient>>,
    ) -> Self {
        Self {
            engine,
            runtime_client,
        }
    }

    pub(crate) fn engine(&self) -> Option<&Arc<ExecutionEngine>> {
        self.engine.as_ref()
    }

    pub(crate) fn runtime_client(&self) -> Option<&Arc<RuntimeClient>> {
        self.runtime_client.as_ref()
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
        crate::api::services::reports::workflow_runtime_provider_fetch_rows(
            self,
            params.tenant_id,
            params.block,
            params.condition,
        )
        .await
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
        crate::api::services::reports::validate_workflow_runtime_block(
            block,
            filter_ids,
            view_ids,
            filter_defs,
        )
    }

    fn field_is_known(&self, block: &ReportBlockDefinition, field: &str) -> bool {
        let Ok(entity) = crate::api::services::reports::workflow_runtime_entity(block) else {
            return false;
        };
        let fields = crate::api::services::reports::workflow_runtime_fields(entity);
        crate::api::services::reports::workflow_runtime_row_field_known(&fields, field)
    }

    fn table_columns(
        &self,
        block: &ReportBlockDefinition,
    ) -> Result<Vec<serde_json::Value>, ReportServiceError> {
        let entity = crate::api::services::reports::workflow_runtime_entity(block)?;
        Ok(
            crate::api::services::reports::workflow_runtime_table_columns(
                block.table.as_ref(),
                entity,
            ),
        )
    }
}
