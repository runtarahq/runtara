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
/// Returns `None` (= all columns) only for shapes that ship the **whole row**
/// to a workflow and so can't be enumerated: a row/selection-mode workflow
/// action, a table-wide action, or row selection. Per-row buttons and
/// interactions (a `field`-mode "run workflow" button, a row-click
/// `set_filter`, a `disabledWhen` guard) read a handful of named fields — those
/// are collected into the set rather than triggering a fall-back, so a table
/// with a drilldown/action button still projects (and still skips the file
/// blob it never displays).
///
/// Only the plain (non-join, non-aggregate) filter path routes here, so WHERE /
/// ORDER BY columns are pushed into SQL and never read back from the row — they
/// need not be projected. The reads that DO come back from the row: each
/// column's `field`, its `displayField`/`secondaryField`/`linkField`/
/// `tooltipField`, the fields its `displayTemplate` interpolates, value-lookup/
/// chart `source.join[].parentField`, plus per-row button/interaction
/// `field`/`valueFrom`/`*When` references. (`id`/`createdAt`/`updatedAt` are
/// always selected by the store regardless of projection.)
fn table_block_projection(block: &ReportBlockDefinition) -> Option<Vec<String>> {
    use crate::api::dto::reports::ReportWorkflowActionContextMode as Mode;

    let table = block.table.as_ref()?;

    // Table-wide actions and row selection ship the whole selected row(s) to a
    // workflow — we can't statically enumerate which fields that needs.
    if !table.actions.is_empty() || table.selectable {
        return None;
    }

    let mut cols: HashSet<String> = HashSet::new();

    // Block interactions (e.g. row_click -> set_filter) read named row fields
    // via `valueFrom` ("datum.<field>"). Collect them; only fall back if a ref
    // can't be resolved to a single field.
    for interaction in &block.interactions {
        for action in &interaction.actions {
            if let Some(value_from) = &action.value_from {
                match datum_field_ref(value_from) {
                    Some(field) => {
                        cols.insert(field);
                    }
                    None => return None,
                }
            }
        }
    }

    for column in &table.columns {
        // Workflow-button column: a field/value-mode action sends one named
        // field; a row/selection-mode one ships the whole row.
        if let Some(action) = &column.workflow_action {
            match action.context.mode {
                Mode::Row | Mode::Selection => return None,
                Mode::Field | Mode::Value => {
                    if let Some(field) = &action.context.field {
                        cols.insert(field.clone());
                    }
                }
            }
            if let Some(when) = &action.disabled_when {
                collect_condition_refs(when, &mut cols);
            }
        }

        // Per-row interaction buttons carry their own actions + `*When` guards.
        for button in &column.interaction_buttons {
            for action in &button.actions {
                if let Some(value_from) = &action.value_from {
                    match datum_field_ref(value_from) {
                        Some(field) => {
                            cols.insert(field);
                        }
                        None => return None,
                    }
                }
            }
            for when in [
                &button.visible_when,
                &button.hidden_when,
                &button.disabled_when,
            ]
            .into_iter()
            .flatten()
            {
                collect_condition_refs(when, &mut cols);
            }
        }

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

/// Report-agnostic safety net, applied to a rendered object-model **table**
/// block: collapse oversized string values in the row fields the block does NOT
/// display or need, reusing the same keep-set as [`table_block_projection`].
///
/// This is what makes blob-heavy reports "just work" with no authoring: SQL
/// projection only covers the plain filter path, but this runs on the final
/// rendered rows, so it also catches the joined-filter path (which fetches
/// every column) and any undisplayed blob that otherwise reaches the response.
/// It never touches a value the report shows — displayed columns are in the
/// keep-set — and small fields (`id`/`createdAt`/...) pass through unchanged
/// because only strings over the threshold are collapsed. Whole-row shapes
/// (selectable / row-mode action), where the keep-set is `None`, are left
/// intact since the client ships every field to a workflow.
pub fn elide_undisplayed_row_blobs(block: &ReportBlockDefinition, data: &mut Value) {
    if block.source.kind != ReportSourceKind::ObjectModel {
        return;
    }
    let Some(keep) = table_block_projection(block) else {
        return;
    };
    let keep: HashSet<&str> = keep.iter().map(String::as_str).collect();
    let Some(rows) = data.get_mut("rows").and_then(Value::as_array_mut) else {
        return;
    };
    for row in rows {
        let Some(obj) = row.as_object_mut() else {
            continue;
        };
        for (field, value) in obj.iter_mut() {
            if keep.contains(field.as_str()) {
                continue;
            }
            *value = crate::workers::runtara_dto::elide_large_strings(std::mem::take(value));
        }
    }
}

/// Resolve an interaction `valueFrom` path (`datum.<field>` / `row.<field>` /
/// `<field>`) to the single schema column it reads, or `None` if it can't be
/// reduced to one — in which case the caller falls back to selecting all
/// columns rather than risk dropping a needed field.
fn datum_field_ref(value_from: &str) -> Option<String> {
    let path = value_from
        .strip_prefix("datum.")
        .or_else(|| value_from.strip_prefix("row."))
        .unwrap_or(value_from);
    let seg = path.split(['.', '[']).next().unwrap_or("").trim();
    (!seg.is_empty()).then(|| seg.to_string())
}

/// Collect the first path segment of every `{valueType: "reference", value:
/// "<field>"}` operand in a serialized condition expression (a button's
/// `disabledWhen`/`visibleWhen`/`hiddenWhen`) — the row fields it evaluates.
fn collect_condition_refs<T: serde::Serialize>(cond: &T, out: &mut HashSet<String>) {
    if let Ok(value) = serde_json::to_value(cond) {
        collect_reference_fields(&value, out);
    }
}

fn collect_reference_fields(value: &Value, out: &mut HashSet<String>) {
    match value {
        Value::Object(map) => {
            let is_ref = map
                .get("valueType")
                .or_else(|| map.get("value_type"))
                .and_then(|v| v.as_str())
                == Some("reference");
            if is_ref
                && let Some(field) = map.get("value").and_then(|v| v.as_str())
                && let Some(seg) = field.split(['.', '[']).next()
                && !seg.is_empty()
            {
                out.insert(seg.to_string());
            }
            for nested in map.values() {
                collect_reference_fields(nested, out);
            }
        }
        Value::Array(items) => {
            for nested in items {
                collect_reference_fields(nested, out);
            }
        }
        _ => {}
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn projection_collects_button_and_interaction_refs_excluding_blob() {
        // The "Spend file inbox" files_table shape: a field-mode workflow button
        // (with a `disabledWhen` on `status`) plus a row-click `set_filter`
        // interaction (valueFrom `datum.id`). None of the columns is the file
        // blob, so the blob column must NOT end up in the projection.
        let block: ReportBlockDefinition = serde_json::from_value(serde_json::json!({
            "id": "files_table",
            "type": "table",
            "source": { "schema": "CustomerTransactionsFile", "mode": "filter" },
            "table": {
                "columns": [
                    {
                        "field": "id",
                        "type": "workflow_button",
                        "workflowAction": {
                            "workflowId": "wf",
                            "context": { "mode": "field", "field": "id", "inputKey": "file_id" },
                            "disabledWhen": {
                                "type": "operation",
                                "op": "IN",
                                "arguments": [
                                    { "valueType": "reference", "value": "status" },
                                    { "valueType": "immediate", "value": ["done"] }
                                ]
                            }
                        }
                    },
                    { "field": "file_name" },
                    { "field": "status" }
                ]
            },
            "interactions": [
                {
                    "id": "open_file",
                    "trigger": { "event": "row_click" },
                    "actions": [
                        { "type": "set_filter", "filterId": "file_id", "valueFrom": "datum.id" }
                    ]
                }
            ]
        }))
        .expect("block should deserialize");

        let cols: HashSet<String> = table_block_projection(&block)
            .expect("a field-mode button + row-click interaction must still project")
            .into_iter()
            .collect();

        for needed in ["id", "file_name", "status"] {
            assert!(
                cols.contains(needed),
                "projection missing {needed}: {cols:?}"
            );
        }
        // Undisplayed file-content columns are never referenced -> excluded.
        assert!(!cols.contains("file_content"));
        assert!(!cols.contains("raw_csv"));
    }

    #[test]
    fn projection_bails_for_selectable_table() {
        let block: ReportBlockDefinition = serde_json::from_value(serde_json::json!({
            "id": "t",
            "type": "table",
            "source": { "schema": "S", "mode": "filter" },
            "table": { "selectable": true, "columns": [{ "field": "a" }] }
        }))
        .expect("block should deserialize");
        assert!(table_block_projection(&block).is_none());
    }

    fn big_string() -> String {
        "X".repeat(crate::workers::runtara_dto::ELIDE_THRESHOLD_BYTES + 1)
    }

    #[test]
    fn response_elision_collapses_undisplayed_blob_keeps_displayed_and_small() {
        let block: ReportBlockDefinition = serde_json::from_value(serde_json::json!({
            "id": "t",
            "type": "table",
            "source": { "schema": "Files", "mode": "filter" },
            "table": { "columns": [{ "field": "code" }, { "field": "name" }] }
        }))
        .unwrap();
        let mut data = serde_json::json!({
            "rows": [{ "id": "1", "code": "A", "name": "n", "file_content": big_string() }],
            "columns": [],
            "page": {}
        });

        elide_undisplayed_row_blobs(&block, &mut data);

        let row = &data["rows"][0];
        assert_eq!(row["code"], serde_json::json!("A")); // displayed -> kept
        assert_eq!(row["name"], serde_json::json!("n"));
        assert_eq!(row["id"], serde_json::json!("1")); // small -> kept verbatim
        // Undisplayed blob -> collapsed to the elision stub.
        assert_eq!(row["file_content"]["_elided"], serde_json::json!(true));
    }

    #[test]
    fn response_elision_keeps_a_displayed_blob_column() {
        let block: ReportBlockDefinition = serde_json::from_value(serde_json::json!({
            "id": "t",
            "type": "table",
            "source": { "schema": "Files", "mode": "filter" },
            "table": { "columns": [{ "field": "file_content" }] }
        }))
        .unwrap();
        let big = big_string();
        let mut data = serde_json::json!({ "rows": [{ "file_content": big }] });

        elide_undisplayed_row_blobs(&block, &mut data);

        // file_content IS a displayed column -> never elided.
        assert!(data["rows"][0]["file_content"].is_string());
        assert!(data["rows"][0]["file_content"].as_str().unwrap().len() > 1000);
    }

    #[test]
    fn response_elision_skips_non_object_model_sources() {
        let block: ReportBlockDefinition = serde_json::from_value(serde_json::json!({
            "id": "t",
            "type": "table",
            "source": { "kind": "workflow_runtime", "schema": "", "mode": "filter" },
            "table": { "columns": [{ "field": "a" }] }
        }))
        .unwrap();
        let mut data = serde_json::json!({ "rows": [{ "blob": big_string() }] });
        elide_undisplayed_row_blobs(&block, &mut data);
        // Non-object-model sources are untouched (no schema-column blobs).
        assert!(data["rows"][0]["blob"].is_string());
    }
}
