//! Source provider trait + dispatch registry for the reports module.
//!
//! Each [`ReportSourceProvider`] owns the data-acquisition path for a single
//! [`ReportSourceKind`]: it fetches rows (and optionally aggregates), reports
//! its field set for the renderer's column validators, and validates blocks
//! that target it. The renderer in `services/reports.rs` dispatches through
//! the [`ProviderRegistry`] instead of branching on `source.kind` itself.

use async_trait::async_trait;
use serde_json::{Map, Value};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::api::dto::object_model::{AggregateRequest, Condition};
use crate::api::dto::reports::*;
use crate::api::services::reports::ReportServiceError;

pub mod object_model;
pub mod system;
pub mod workflow_runtime;

pub use object_model::ObjectModelProvider;
pub use system::SystemProvider;
pub use workflow_runtime::WorkflowRuntimeProvider;

/// Inputs to a provider fetch.
///
/// `condition` is pre-resolved from the block + active filters + view.
/// `sort` / `offset` / `limit` carry the requested page. Providers with
/// aggregate pushdown apply all of these at storage; providers without
/// (system, workflow_runtime) fetch the full set matching `condition`
/// and let the renderer slice in-memory.
#[derive(Clone, Copy)]
pub struct FetchParams<'a> {
    pub tenant_id: &'a str,
    pub block: &'a ReportBlockDefinition,
    pub condition: Option<&'a Condition>,
    pub sort: &'a [ReportOrderBy],
    pub offset: i64,
    pub limit: i64,
}

/// Filter-mode fetch result.
///
/// `total_count` is `None` when the provider streamed everything matching
/// the condition (then the renderer paginates in-memory); `Some(_)` when
/// the provider pushed pagination down (`page` already reflects the slice).
pub struct FetchRowsOutput {
    pub rows: Vec<Map<String, Value>>,
    pub total_count: Option<i64>,
}

/// Aggregate-mode fetch result. Mirrors `runtara_object_store::AggregateResult`
/// so the object-model provider can return it directly and the virtual-aggregate
/// path (system / workflow_runtime) can construct the same shape.
pub struct FetchAggregateOutput {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Value>>,
    pub group_count: i64,
}

impl From<runtara_object_store::AggregateResult> for FetchAggregateOutput {
    fn from(value: runtara_object_store::AggregateResult) -> Self {
        Self {
            columns: value.columns,
            rows: value.rows,
            group_count: value.group_count,
        }
    }
}

#[async_trait]
pub trait ReportSourceProvider: Send + Sync {
    /// The source kind this provider answers for. The registry uses this to
    /// dispatch and to validate uniqueness at construction time.
    fn kind(&self) -> ReportSourceKind;

    /// Pull rows for a filter-mode block. Providers that push the condition
    /// down to storage should return `total_count = Some(..)`; providers that
    /// post-filter in memory should leave it `None` so the renderer slices.
    async fn fetch_rows(
        &self,
        params: FetchParams<'_>,
    ) -> Result<FetchRowsOutput, ReportServiceError>;

    /// Run an aggregate request. Providers without pushdown rebuild this on
    /// top of `fetch_rows` + the virtual aggregate engine.
    async fn fetch_aggregate(
        &self,
        params: FetchParams<'_>,
        request: AggregateRequest,
    ) -> Result<FetchAggregateOutput, ReportServiceError>;

    /// Block-level validation specific to this source. Called from
    /// `validate_report_definition` after generic structural checks.
    fn validate_block(
        &self,
        block: &ReportBlockDefinition,
        filter_ids: &HashSet<String>,
        view_ids: &HashSet<String>,
        filter_defs: &HashMap<String, &ReportFilterDefinition>,
    ) -> Result<(), ReportServiceError>;

    /// `true` if a single dotted field path matches a known row field for
    /// this block's entity. Object-model defers to schema metadata so this
    /// returns `true` for fields the storage layer will resolve.
    fn field_is_known(&self, block: &ReportBlockDefinition, field: &str) -> bool;

    /// Whether the provider pushes aggregates down to storage. Object-model
    /// returns `true` (SQL); system/workflow_runtime return `false`.
    fn supports_aggregate_pushdown(&self) -> bool {
        false
    }

    /// Build the `columns` array for a table response. Falls back to the
    /// block's `table.columns` config when present; otherwise the provider
    /// supplies a default column set for the entity.
    fn table_columns(
        &self,
        block: &ReportBlockDefinition,
    ) -> Result<Vec<Value>, ReportServiceError>;
}

/// Dispatch table from [`ReportSourceKind`] to its provider.
pub struct ProviderRegistry {
    object_model: Arc<dyn ReportSourceProvider>,
    workflow_runtime: Arc<dyn ReportSourceProvider>,
    system: Arc<dyn ReportSourceProvider>,
}

impl ProviderRegistry {
    pub fn new(
        object_model: Arc<dyn ReportSourceProvider>,
        workflow_runtime: Arc<dyn ReportSourceProvider>,
        system: Arc<dyn ReportSourceProvider>,
    ) -> Self {
        debug_assert_eq!(object_model.kind(), ReportSourceKind::ObjectModel);
        debug_assert_eq!(workflow_runtime.kind(), ReportSourceKind::WorkflowRuntime);
        debug_assert_eq!(system.kind(), ReportSourceKind::System);
        Self {
            object_model,
            workflow_runtime,
            system,
        }
    }

    pub fn get(&self, kind: ReportSourceKind) -> &Arc<dyn ReportSourceProvider> {
        match kind {
            ReportSourceKind::ObjectModel => &self.object_model,
            ReportSourceKind::WorkflowRuntime => &self.workflow_runtime,
            ReportSourceKind::System => &self.system,
        }
    }
}
