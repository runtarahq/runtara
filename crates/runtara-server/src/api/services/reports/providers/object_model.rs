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

use crate::api::dto::object_model::{AggregateOrderBy, AggregateRequest, Condition, FilterRequest};
use crate::api::dto::reports::*;
use crate::api::repositories::object_model::ObjectStoreManager;
use crate::api::services::object_model::InstanceService;
use crate::api::services::reports::query_plan::{
    JoinResolution, build_alias_index, empty_join_result, enrich_aggregate_result,
    field_alias_prefix, split_qualified_condition, strip_alias_from_condition,
    validate_join_request,
};
use crate::api::services::reports::{
    MAX_BROADCAST_JOIN_DIM_ROWS, ReportServiceError, combine_conditions, flatten_instance,
    normalize_sort_direction, table_response_columns,
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
        let result = aggregate_with_optional_joins(
            &self.instance_service,
            params.tenant_id,
            params.block,
            request,
        )
        .await?;
        Ok(FetchAggregateOutput::from(result))
    }

    fn validate_block(
        &self,
        block: &ReportBlockDefinition,
        _filter_ids: &HashSet<String>,
        _view_ids: &HashSet<String>,
        _filter_defs: &HashMap<String, &ReportFilterDefinition>,
    ) -> Result<(), ReportServiceError> {
        if block.block_type == ReportBlockType::Card
            && block.source.mode != ReportSourceMode::Filter
        {
            return Err(ReportServiceError::Validation(format!(
                "Block '{}' card blocks only support filter-mode sources",
                block.id
            )));
        }
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

/// Schema columns a Filter-mode table block actually needs in each fetched
/// row, or `None` to fall back to selecting every column. Projecting to this
/// set stops large unused columns (e.g. 31 kB HTML / base64 file uploads) from
/// being pulled out of the DB and serialized on every page — the dominant cost
/// of a report over a blob-heavy schema once round trips are fixed.
///
/// Returns `None` (= all columns) whenever the block can read row fields we
/// can't statically enumerate: row/selection-mode workflow actions ship the
/// whole row, and interaction drilldowns / interaction-button columns reference
/// arbitrary `valueFrom`/`*_when` fields. Correctness beats savings there.
///
/// Only the plain (non-join, non-aggregate) filter path routes here, so WHERE /
/// ORDER BY columns are pushed into SQL and never read back from the row — they
/// need not be projected. The display-side reads do: each column's `field`, its
/// `displayField`/`secondaryField`/`linkField`/`tooltipField`, the fields its
/// `displayTemplate` interpolates, and any value-lookup/chart
/// `source.join[].parentField` used to hydrate the cell. (`id`/`createdAt`/
/// `updatedAt` are always selected by the store regardless of projection.)
fn table_block_projection(block: &ReportBlockDefinition) -> Option<Vec<String>> {
    let table = block.table.as_ref()?;

    // Bail to "all columns" for shapes that read un-enumerable row fields.
    if !block.interactions.is_empty() || !table.actions.is_empty() || table.selectable {
        return None;
    }
    for column in &table.columns {
        if column.is_workflow_button()
            || matches!(
                column.column_type,
                Some(ReportTableColumnType::InteractionButtons)
            )
        {
            return None;
        }
    }

    let mut cols: HashSet<String> = HashSet::new();
    for column in &table.columns {
        cols.insert(column.field.clone());
        for name in [
            &column.display_field,
            &column.secondary_field,
            &column.link_field,
            &column.tooltip_field,
        ]
        .into_iter()
        .flatten()
        {
            cols.insert(name.clone());
        }
        if let Some(template) = &column.display_template {
            collect_display_template_fields(template, &mut cols);
        }
        if let Some(source) = &column.source {
            for join in &source.join {
                cols.insert(join.parent_field.clone());
            }
        }
    }

    Some(cols.into_iter().collect())
}

/// Collect the first path segment of each `{{ path | format }}` token in a
/// display template — the schema column the client resolves it against. Errs
/// toward over-collecting (harmless: the store intersects against real columns)
/// so a referenced column is never dropped.
fn collect_display_template_fields(template: &str, out: &mut HashSet<String>) {
    let mut rest = template;
    while let Some(open) = rest.find("{{") {
        rest = &rest[open + 2..];
        let Some(close) = rest.find("}}") else { break };
        let inner = &rest[..close];
        rest = &rest[close + 2..];
        let path = inner.split('|').next().unwrap_or("").trim();
        let path = path.strip_prefix("row.").unwrap_or(path);
        let seg: String = path
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        if !seg.is_empty() {
            out.insert(seg);
        }
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
        projection: table_block_projection(block),
    };

    let (instances, total_count) = provider
        .instance_service
        .filter_instances_by_schema(
            tenant_id,
            &block.source.schema,
            filter_request,
            block.source.connection_id.as_deref(),
        )
        .await?;

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

/// Run an aggregate request that may reference joined dimension schemas
/// via `<alias>.<field>` qualified field names. Implements broadcast-hash
/// join: each declared dimension is resolved client-side first (with any
/// `<alias>.<field>` condition terms applied), the primary aggregate is
/// filtered by the resolved parent-field keys, and result rows are enriched
/// with the joined dimension columns. When `block.source.join` is empty
/// this is a passthrough to the regular aggregate.
async fn aggregate_with_optional_joins(
    instance_service: &InstanceService,
    tenant_id: &str,
    block: &ReportBlockDefinition,
    request: AggregateRequest,
) -> Result<runtara_object_store::AggregateResult, ReportServiceError> {
    let joins = &block.source.join;
    if joins.is_empty() {
        return instance_service
            .aggregate_instances_by_schema(
                tenant_id,
                &block.source.schema,
                request,
                block.source.connection_id.as_deref(),
            )
            .await
            .map_err(Into::into);
    }

    let alias_to_join = build_alias_index(joins, &block.id)?;
    validate_join_request(&request, &alias_to_join, &block.id)?;

    let alias_set: HashSet<&str> = alias_to_join.keys().map(|s| s.as_str()).collect();
    let (primary_condition, by_alias) =
        split_qualified_condition(request.condition.clone(), &alias_set, &block.id)?;

    let mut join_data: HashMap<String, JoinResolution> = HashMap::new();
    for (alias, join) in &alias_to_join {
        let alias_terms = by_alias.get(alias).cloned().unwrap_or_default();
        let resolution = resolve_join(instance_service, tenant_id, join, &alias_terms).await?;
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

    let primary_result = instance_service
        .aggregate_instances_by_schema(
            tenant_id,
            &block.source.schema,
            primary_request,
            block.source.connection_id.as_deref(),
        )
        .await?;

    Ok(enrich_aggregate_result(
        primary_result,
        &request.group_by,
        &alias_to_join,
        &join_data,
    ))
}

/// Query a dimension schema and build a lookup keyed by the join's `field`.
async fn resolve_join(
    instance_service: &InstanceService,
    tenant_id: &str,
    join: &ReportSourceJoin,
    alias_terms: &[Condition],
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
        projection: None,
    };

    let (dim_instances, total) = instance_service
        .filter_instances_by_schema(
            tenant_id,
            &join.schema,
            filter,
            join.connection_id.as_deref(),
        )
        .await?;

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
        if crate::api::services::reports::value_is_empty(key_value) {
            continue;
        }
        let key_str = crate::api::services::reports::query_plan::value_to_lookup_key(key_value);
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
