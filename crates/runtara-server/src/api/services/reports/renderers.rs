//! Per-block-type render dispatch.
//!
//! Each [`ReportBlockType`] variant has a corresponding [`BlockRenderer`]
//! implementation. The renderers are zero-sized; they delegate into the
//! `ReportService`'s per-type render methods (which already exist as the
//! data-acquisition + formatting bodies). The trait + factory replace the
//! prior inline `match block.block_type` in `render_block`.
//!
//! Block-type-specific *data acquisition* lives in
//! [`crate::api::services::reports::providers`]. The renderers here own the
//! per-type *response shaping* (column/row layout, metric value extraction,
//! card field projection, etc.).

use async_trait::async_trait;
use serde_json::Value;
use std::collections::HashMap;

use crate::api::dto::reports::*;
use crate::api::services::reports::{ReportService, ReportServiceError};

/// Pluggable per-block-type render entry point.
#[async_trait]
pub(super) trait BlockRenderer: Send + Sync {
    async fn render(
        &self,
        service: &ReportService,
        tenant_id: &str,
        definition: &ReportDefinition,
        block: &ReportBlockDefinition,
        resolved_filters: &HashMap<String, Value>,
        block_request: Option<&ReportBlockDataRequest>,
    ) -> Result<Value, ReportServiceError>;
}

pub(super) struct TableRenderer;
pub(super) struct ChartRenderer;
pub(super) struct MetricRenderer;
pub(super) struct ActionsRenderer;
pub(super) struct MarkdownRenderer;
pub(super) struct CardRenderer;

#[async_trait]
impl BlockRenderer for TableRenderer {
    async fn render(
        &self,
        service: &ReportService,
        tenant_id: &str,
        definition: &ReportDefinition,
        block: &ReportBlockDefinition,
        resolved_filters: &HashMap<String, Value>,
        block_request: Option<&ReportBlockDataRequest>,
    ) -> Result<Value, ReportServiceError> {
        service
            .render_table_block(
                tenant_id,
                definition,
                block,
                resolved_filters,
                block_request,
            )
            .await
    }
}

#[async_trait]
impl BlockRenderer for ChartRenderer {
    async fn render(
        &self,
        service: &ReportService,
        tenant_id: &str,
        definition: &ReportDefinition,
        block: &ReportBlockDefinition,
        resolved_filters: &HashMap<String, Value>,
        _block_request: Option<&ReportBlockDataRequest>,
    ) -> Result<Value, ReportServiceError> {
        service
            .render_aggregate_block(tenant_id, definition, block, resolved_filters)
            .await
    }
}

#[async_trait]
impl BlockRenderer for MetricRenderer {
    async fn render(
        &self,
        service: &ReportService,
        tenant_id: &str,
        definition: &ReportDefinition,
        block: &ReportBlockDefinition,
        resolved_filters: &HashMap<String, Value>,
        _block_request: Option<&ReportBlockDataRequest>,
    ) -> Result<Value, ReportServiceError> {
        service
            .render_metric_block(tenant_id, definition, block, resolved_filters)
            .await
    }
}

#[async_trait]
impl BlockRenderer for ActionsRenderer {
    async fn render(
        &self,
        service: &ReportService,
        tenant_id: &str,
        definition: &ReportDefinition,
        block: &ReportBlockDefinition,
        resolved_filters: &HashMap<String, Value>,
        block_request: Option<&ReportBlockDataRequest>,
    ) -> Result<Value, ReportServiceError> {
        service
            .render_actions_block(
                tenant_id,
                definition,
                block,
                resolved_filters,
                block_request,
            )
            .await
    }
}

#[async_trait]
impl BlockRenderer for MarkdownRenderer {
    async fn render(
        &self,
        service: &ReportService,
        tenant_id: &str,
        definition: &ReportDefinition,
        block: &ReportBlockDefinition,
        resolved_filters: &HashMap<String, Value>,
        block_request: Option<&ReportBlockDataRequest>,
    ) -> Result<Value, ReportServiceError> {
        service
            .render_markdown_block(
                tenant_id,
                definition,
                block,
                resolved_filters,
                block_request,
            )
            .await
    }
}

#[async_trait]
impl BlockRenderer for CardRenderer {
    async fn render(
        &self,
        service: &ReportService,
        tenant_id: &str,
        definition: &ReportDefinition,
        block: &ReportBlockDefinition,
        resolved_filters: &HashMap<String, Value>,
        _block_request: Option<&ReportBlockDataRequest>,
    ) -> Result<Value, ReportServiceError> {
        service
            .render_card_block(tenant_id, definition, block, resolved_filters)
            .await
    }
}

/// Look up the renderer for a given block type. Adding a new block type
/// requires a new [`BlockRenderer`] impl + a new branch here.
pub(super) fn renderer_for(block_type: ReportBlockType) -> &'static dyn BlockRenderer {
    match block_type {
        ReportBlockType::Table => &TableRenderer,
        ReportBlockType::Chart => &ChartRenderer,
        ReportBlockType::Metric => &MetricRenderer,
        ReportBlockType::Actions => &ActionsRenderer,
        ReportBlockType::Markdown => &MarkdownRenderer,
        ReportBlockType::Card => &CardRenderer,
    }
}
