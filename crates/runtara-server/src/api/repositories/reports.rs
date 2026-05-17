use chrono::{DateTime, Utc};
use serde_json::Value;
#[cfg(test)]
use serde_json::json;
use sqlx::{PgPool, Row, postgres::PgRow};

use crate::api::dto::reports::{ReportDefinition, ReportDto, ReportStatus};

pub struct ReportRepository {
    pool: PgPool,
}

impl ReportRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn list(&self, tenant_id: &str) -> Result<Vec<ReportDto>, sqlx::Error> {
        let rows = sqlx::query(
            r#"
            SELECT id, slug, name, description, tags, definition_version, definition,
                   status, created_at, updated_at
            FROM report_definitions
            WHERE tenant_id = $1 AND deleted_at IS NULL
            ORDER BY updated_at DESC
            "#,
        )
        .bind(tenant_id)
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter().map(row_to_report).collect()
    }

    pub async fn get(
        &self,
        tenant_id: &str,
        id_or_slug: &str,
    ) -> Result<Option<ReportDto>, sqlx::Error> {
        let row = sqlx::query(
            r#"
            SELECT id, slug, name, description, tags, definition_version, definition,
                   status, created_at, updated_at
            FROM report_definitions
            WHERE tenant_id = $1
              AND deleted_at IS NULL
              AND (id = $2 OR slug = $2)
            "#,
        )
        .bind(tenant_id)
        .bind(id_or_slug)
        .fetch_optional(&self.pool)
        .await?;

        row.map(row_to_report).transpose()
    }

    pub async fn create(
        &self,
        tenant_id: &str,
        report: &ReportDto,
    ) -> Result<ReportDto, sqlx::Error> {
        let definition = serde_json::to_value(&report.definition).unwrap_or(Value::Null);
        let tags = serde_json::to_value(&report.tags).unwrap_or(Value::Array(vec![]));

        let row = sqlx::query(
            r#"
            INSERT INTO report_definitions
                (id, tenant_id, slug, name, description, tags, definition_version,
                 definition, status)
            VALUES
                ($1, $2, $3, $4, $5, $6, $7, $8, $9)
            RETURNING id, slug, name, description, tags, definition_version, definition,
                      status, created_at, updated_at
            "#,
        )
        .bind(&report.id)
        .bind(tenant_id)
        .bind(&report.slug)
        .bind(&report.name)
        .bind(&report.description)
        .bind(tags)
        .bind(report.definition_version)
        .bind(definition)
        .bind(report.status.as_str())
        .fetch_one(&self.pool)
        .await?;

        row_to_report(row)
    }

    pub async fn update(
        &self,
        tenant_id: &str,
        id_or_slug: &str,
        report: &ReportDto,
    ) -> Result<Option<ReportDto>, sqlx::Error> {
        let definition = serde_json::to_value(&report.definition).unwrap_or(Value::Null);
        let tags = serde_json::to_value(&report.tags).unwrap_or(Value::Array(vec![]));

        let row = sqlx::query(
            r#"
            UPDATE report_definitions
            SET slug = $3,
                name = $4,
                description = $5,
                tags = $6,
                definition_version = $7,
                definition = $8,
                status = $9,
                updated_at = NOW()
            WHERE tenant_id = $1
              AND deleted_at IS NULL
              AND (id = $2 OR slug = $2)
            RETURNING id, slug, name, description, tags, definition_version, definition,
                      status, created_at, updated_at
            "#,
        )
        .bind(tenant_id)
        .bind(id_or_slug)
        .bind(&report.slug)
        .bind(&report.name)
        .bind(&report.description)
        .bind(tags)
        .bind(report.definition_version)
        .bind(definition)
        .bind(report.status.as_str())
        .fetch_optional(&self.pool)
        .await?;

        row.map(row_to_report).transpose()
    }

    pub async fn delete(&self, tenant_id: &str, id_or_slug: &str) -> Result<u64, sqlx::Error> {
        let result = sqlx::query(
            r#"
            UPDATE report_definitions
            SET deleted_at = NOW(), updated_at = NOW()
            WHERE tenant_id = $1
              AND deleted_at IS NULL
              AND (id = $2 OR slug = $2)
            "#,
        )
        .bind(tenant_id)
        .bind(id_or_slug)
        .execute(&self.pool)
        .await?;

        Ok(result.rows_affected())
    }
}

fn row_to_report(row: PgRow) -> Result<ReportDto, sqlx::Error> {
    let tags_value: Value = row.try_get("tags")?;
    let tags = serde_json::from_value(tags_value).unwrap_or_default();

    let definition_value: Value = row.try_get("definition")?;
    let definition_version = row.try_get("definition_version").unwrap_or(1);
    let (definition, needs_re_authoring) =
        parse_stored_definition(definition_value, definition_version);

    let status: String = row.try_get("status")?;

    Ok(ReportDto {
        id: row.try_get("id")?,
        slug: row.try_get("slug")?,
        name: row.try_get("name")?,
        description: row.try_get("description")?,
        tags,
        status: ReportStatus::from_db(&status),
        definition_version: row.try_get("definition_version")?,
        definition,
        needs_re_authoring,
        created_at: row.try_get::<DateTime<Utc>, _>("created_at")?,
        updated_at: row.try_get::<DateTime<Utc>, _>("updated_at")?,
    })
}

/// Parse a stored definition JSON into a `ReportDefinition`. When the
/// stored shape no longer matches the current `ReportDefinition` (e.g.
/// after a schema-breaking cutover landed in Phases 1-7), returns an
/// empty stub plus the parser error in `Option<String>` so callers can
/// surface a "needs re-authoring" state instead of crashing.
///
/// Phase 9 collapse: the four legacy layout container types (`section`,
/// `columns`, `metric_row`, plus the pre-Phase-9 flat-items `grid`) are
/// translated into the new recursive `Grid` shape *before* serde
/// deserialize runs, so existing stored reports keep loading without
/// needing the `needs_re_authoring` fallback.
fn parse_stored_definition(
    value: Value,
    definition_version: i32,
) -> (ReportDefinition, Option<String>) {
    let sanitized = sanitize_unsupported_report_definition(value);
    let migrated = migrate_legacy_layout_in_definition(sanitized);
    match serde_json::from_value::<ReportDefinition>(migrated) {
        Ok(definition) => (definition, None),
        Err(error) => {
            tracing::warn!(
                error = %error,
                "Stored report definition could not be read after compatibility sanitization"
            );
            (
                ReportDefinition {
                    definition_version,
                    layout: vec![],
                    views: vec![],
                    filters: vec![],
                    datasets: vec![],
                    blocks: vec![],
                },
                Some(error.to_string()),
            )
        }
    }
}

/// Walk `definition.layout` and each `view.layout` in the raw JSON tree
/// and translate every legacy layout container into the new `grid`
/// shape. Pure structural rewrite — no information loss.
///
/// The four legacy container types and their grid equivalents:
///
/// - `section { title?, description?, children }`
///   → `grid { title?, description?, columns: 1, items: children.map(wrap) }`
/// - `columns { columns: [{ id, width?, children }] }`
///   → `grid { columns: N, columnWidths: widths?, items: columns.map(col =>
///      { id, child: grid { columns: 1, items: col.children.map(wrap) } }) }`
///   so each column's children stay co-located.
/// - `metric_row { title?, blocks }`
///   → `grid { title?, columns: blocks.len, items: blocks.map(blockId =>
///      { id, child: block { blockId } }) }`
/// - `grid { columns?, items: [{ blockId, colSpan?, rowSpan? }] }`
///   → `grid { columns?, items: items.map(item =>
///      { id, colSpan?, rowSpan?, child: block { blockId } }) }`
///   (existing grid items get their `blockId` wrapped in a `block` child)
fn migrate_legacy_layout_in_definition(mut value: Value) -> Value {
    if let Value::Object(object) = &mut value {
        if let Some(layout) = object.get_mut("layout") {
            migrate_layout_array(layout);
        }
        if let Some(Value::Array(views)) = object.get_mut("views") {
            for view in views {
                if let Value::Object(view_object) = view
                    && let Some(layout) = view_object.get_mut("layout")
                {
                    migrate_layout_array(layout);
                }
            }
        }
    }
    value
}

fn migrate_layout_array(value: &mut Value) {
    let Value::Array(nodes) = value else {
        return;
    };
    for node in nodes.iter_mut() {
        if let Value::Object(_) = node {
            let original = std::mem::replace(node, Value::Null);
            *node = migrate_layout_node(original);
        }
    }
}

fn migrate_layout_node(node: Value) -> Value {
    let Value::Object(mut object) = node else {
        return node;
    };
    let node_type = object
        .get("type")
        .and_then(Value::as_str)
        .map(str::to_string);
    let id = object
        .get("id")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_default();

    match node_type.as_deref() {
        Some("block") => Value::Object(object),
        Some("section") => {
            let title = object.remove("title");
            let description = object.remove("description");
            let show_when = object.remove("showWhen");
            let children = object.remove("children").unwrap_or(Value::Array(vec![]));
            let items = wrap_children_as_items(children, &id);
            let mut grid = serde_json::Map::new();
            grid.insert("type".into(), Value::String("grid".into()));
            grid.insert("id".into(), Value::String(id.clone()));
            if let Some(v) = title {
                grid.insert("title".into(), v);
            }
            if let Some(v) = description {
                grid.insert("description".into(), v);
            }
            grid.insert("columns".into(), Value::from(1i64));
            grid.insert("items".into(), Value::Array(items));
            if let Some(v) = show_when {
                grid.insert("showWhen".into(), v);
            }
            Value::Object(grid)
        }
        Some("metric_row") => {
            let title = object.remove("title");
            let show_when = object.remove("showWhen");
            let blocks_array = object
                .remove("blocks")
                .and_then(|v| match v {
                    Value::Array(arr) => Some(arr),
                    _ => None,
                })
                .unwrap_or_default();
            let columns = blocks_array.len().max(1) as i64;
            let items = blocks_array
                .into_iter()
                .enumerate()
                .filter_map(|(i, entry)| {
                    let block_id = entry.as_str()?.to_string();
                    let child_id = format!("{}_b{}", id, i);
                    let mut block_node = serde_json::Map::new();
                    block_node.insert("type".into(), Value::String("block".into()));
                    block_node.insert("id".into(), Value::String(child_id.clone()));
                    block_node.insert("blockId".into(), Value::String(block_id));
                    Some(grid_item(
                        &format!("{}_i{}", id, i),
                        Value::Object(block_node),
                    ))
                })
                .collect();
            let mut grid = serde_json::Map::new();
            grid.insert("type".into(), Value::String("grid".into()));
            grid.insert("id".into(), Value::String(id.clone()));
            if let Some(v) = title {
                grid.insert("title".into(), v);
            }
            grid.insert("columns".into(), Value::from(columns));
            grid.insert("items".into(), Value::Array(items));
            if let Some(v) = show_when {
                grid.insert("showWhen".into(), v);
            }
            Value::Object(grid)
        }
        Some("columns") => {
            let show_when = object.remove("showWhen");
            let columns_array = object
                .remove("columns")
                .and_then(|v| match v {
                    Value::Array(arr) => Some(arr),
                    _ => None,
                })
                .unwrap_or_default();
            let column_count = columns_array.len().max(1) as i64;
            let mut widths: Vec<f64> = Vec::with_capacity(columns_array.len());
            let mut any_width = false;
            let mut items: Vec<Value> = Vec::with_capacity(columns_array.len());
            for (i, column) in columns_array.into_iter().enumerate() {
                let Value::Object(mut column_obj) = column else {
                    continue;
                };
                let column_id = column_obj
                    .get("id")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .unwrap_or_else(|| format!("{}_c{}", id, i));
                let width = column_obj.get("width").and_then(Value::as_f64);
                widths.push(width.unwrap_or(1.0));
                if width.is_some() {
                    any_width = true;
                }
                let children = column_obj
                    .remove("children")
                    .unwrap_or(Value::Array(vec![]));
                let nested_items = wrap_children_as_items(children, &column_id);
                let mut nested_grid = serde_json::Map::new();
                nested_grid.insert("type".into(), Value::String("grid".into()));
                nested_grid.insert("id".into(), Value::String(column_id.clone()));
                nested_grid.insert("columns".into(), Value::from(1i64));
                nested_grid.insert("items".into(), Value::Array(nested_items));
                items.push(grid_item(
                    &format!("{}_item_{}", id, column_id),
                    Value::Object(nested_grid),
                ));
            }
            let mut grid = serde_json::Map::new();
            grid.insert("type".into(), Value::String("grid".into()));
            grid.insert("id".into(), Value::String(id.clone()));
            grid.insert("columns".into(), Value::from(column_count));
            if any_width {
                grid.insert(
                    "columnWidths".into(),
                    Value::Array(widths.into_iter().map(Value::from).collect()),
                );
            }
            grid.insert("items".into(), Value::Array(items));
            if let Some(v) = show_when {
                grid.insert("showWhen".into(), v);
            }
            Value::Object(grid)
        }
        Some("grid") => {
            // Existing grid — wrap each legacy item's bare `blockId` in
            // a child block-layout-node so the new shape's `child` field
            // is populated. Already-migrated items (with a `child`
            // object) pass through untouched.
            let mut items = match object.remove("items") {
                Some(Value::Array(items)) => items,
                _ => vec![],
            };
            for (i, item) in items.iter_mut().enumerate() {
                if let Value::Object(item_object) = item {
                    if item_object.contains_key("child") {
                        continue;
                    }
                    let block_id = item_object
                        .remove("blockId")
                        .and_then(|v| v.as_str().map(str::to_string));
                    let item_id = item_object
                        .get("id")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                        .unwrap_or_else(|| format!("{}_i{}", id, i));
                    item_object
                        .entry("id".to_string())
                        .or_insert_with(|| Value::String(item_id.clone()));
                    if let Some(block_id) = block_id {
                        let child_id = format!("{}_b{}", id, i);
                        let mut child = serde_json::Map::new();
                        child.insert("type".into(), Value::String("block".into()));
                        child.insert("id".into(), Value::String(child_id));
                        child.insert("blockId".into(), Value::String(block_id));
                        item_object.insert("child".into(), Value::Object(child));
                    }
                }
            }
            object.insert("items".into(), Value::Array(items));
            Value::Object(object)
        }
        _ => Value::Object(object),
    }
}

/// Wraps a flat `Vec<ReportLayoutNode>` (children of a legacy section
/// or columns column) into a `Vec<ReportGridLayoutItem>`. Each child
/// is recursively migrated before being wrapped.
fn wrap_children_as_items(children: Value, parent_id: &str) -> Vec<Value> {
    let Value::Array(nodes) = children else {
        return vec![];
    };
    nodes
        .into_iter()
        .enumerate()
        .map(|(i, child)| {
            let migrated = migrate_layout_node(child);
            grid_item(&format!("{}_i{}", parent_id, i), migrated)
        })
        .collect()
}

fn grid_item(id: &str, child: Value) -> Value {
    let mut item = serde_json::Map::new();
    item.insert("id".into(), Value::String(id.into()));
    item.insert("child".into(), child);
    Value::Object(item)
}

fn sanitize_unsupported_report_definition(mut value: Value) -> Value {
    let Value::Object(object) = &mut value else {
        return value;
    };

    object.remove("markdown");

    if let Some(layout) = object.get_mut("layout") {
        sanitize_unsupported_layout_nodes(layout);
    }

    if let Some(Value::Array(views)) = object.get_mut("views") {
        for view in views {
            if let Some(layout) = view.get_mut("layout") {
                sanitize_unsupported_layout_nodes(layout);
            }
        }
    }

    value
}

fn sanitize_unsupported_layout_nodes(node: &mut Value) {
    match node {
        Value::Array(nodes) => {
            nodes.retain(is_supported_layout_node);
            for child in nodes {
                sanitize_unsupported_layout_nodes(child);
            }
        }
        Value::Object(object) => {
            if let Some(children) = object.get_mut("children") {
                sanitize_unsupported_layout_nodes(children);
            }

            if let Some(Value::Array(columns)) = object.get_mut("columns") {
                for column in columns {
                    if let Some(children) = column.get_mut("children") {
                        sanitize_unsupported_layout_nodes(children);
                    }
                }
            }
        }
        _ => {}
    }
}

fn is_supported_layout_node(node: &Value) -> bool {
    matches!(
        node.get("type").and_then(Value::as_str),
        Some("block" | "metric_row" | "section" | "columns" | "grid")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::dto::reports::ReportLayoutNode;

    #[test]
    fn sanitizes_legacy_layout_markdown_nodes_without_converting_them() {
        let value = json!({
            "definitionVersion": 1,
            "markdown": "# Legacy wrapper",
            "layout": [
                {
                    "id": "intro",
                    "type": "markdown",
                    "content": "# Intro"
                },
                {
                    "id": "items_node",
                    "type": "block",
                    "blockId": "items"
                }
            ],
            "filters": [],
            "blocks": [
                {
                    "id": "items",
                    "type": "table",
                    "source": {"schema": "Item", "mode": "filter"},
                    "table": {"columns": [{"field": "name"}]}
                }
            ]
        });

        let sanitized = sanitize_unsupported_report_definition(value);
        assert!(sanitized.get("markdown").is_none());

        let definition: ReportDefinition = serde_json::from_value(sanitized).unwrap();
        assert_eq!(definition.blocks.len(), 1);
        assert_eq!(definition.blocks[0].id, "items");
        assert_eq!(definition.layout.len(), 1);

        match &definition.layout[0] {
            ReportLayoutNode::Block(node) => {
                assert_eq!(node.block_id, "items");
            }
            other => panic!("expected block layout node, got {other:?}"),
        }
    }

    #[test]
    fn parse_stored_definition_marks_unparseable_legacy_shape_as_needs_re_authoring() {
        // A shape with an unknown root-level "weirdField" plus an
        // unknown block type that the strict schema rejects.
        let value = json!({
            "definitionVersion": 1,
            "weirdField": { "removed": "in-cutover" },
            "blocks": [
                {
                    "id": "first",
                    "type": "totallyMadeUpBlockType",
                    "weirdField": true
                }
            ]
        });

        let (definition, error) = parse_stored_definition(value, 1);
        assert!(error.is_some(), "expected re-authoring error to be set");
        assert!(definition.blocks.is_empty());
        assert!(definition.layout.is_empty());
    }

    #[test]
    fn parse_stored_definition_accepts_current_shape_without_flag() {
        let value = json!({
            "definitionVersion": 1,
            "blocks": [
                {
                    "id": "intro",
                    "type": "markdown",
                    "markdown": { "content": "# Hi" }
                }
            ]
        });
        let (definition, error) = parse_stored_definition(value, 1);
        assert!(error.is_none());
        assert_eq!(definition.blocks.len(), 1);
    }

    #[test]
    fn migrate_legacy_section_becomes_grid_with_columns_one() {
        let value = json!({
            "definitionVersion": 1,
            "layout": [{
                "id": "s1",
                "type": "section",
                "title": "Heading",
                "description": "Sub",
                "children": [
                    {"id": "n_b1", "type": "block", "blockId": "b1"}
                ]
            }],
            "blocks": [
                {"id": "b1", "type": "markdown", "markdown": {"content": "x"}}
            ]
        });
        let migrated = migrate_legacy_layout_in_definition(value);
        let layout = migrated
            .get("layout")
            .and_then(Value::as_array)
            .expect("layout array");
        assert_eq!(layout.len(), 1);
        let node = &layout[0];
        assert_eq!(node.get("type").and_then(Value::as_str), Some("grid"));
        assert_eq!(node.get("title").and_then(Value::as_str), Some("Heading"));
        assert_eq!(node.get("description").and_then(Value::as_str), Some("Sub"));
        assert_eq!(node.get("columns").and_then(Value::as_i64), Some(1));
        let items = node.get("items").and_then(Value::as_array).unwrap();
        assert_eq!(items.len(), 1);
        let child = items[0].get("child").unwrap();
        assert_eq!(child.get("type").and_then(Value::as_str), Some("block"));
        assert_eq!(child.get("blockId").and_then(Value::as_str), Some("b1"));
    }

    #[test]
    fn migrate_legacy_columns_wraps_children_in_nested_grids() {
        let value = json!({
            "definitionVersion": 1,
            "layout": [{
                "id": "cols",
                "type": "columns",
                "columns": [
                    {
                        "id": "left",
                        "width": 2.0,
                        "children": [{"id": "n_a", "type": "block", "blockId": "a"}]
                    },
                    {
                        "id": "right",
                        "width": 1.0,
                        "children": [{"id": "n_b", "type": "block", "blockId": "b"}]
                    }
                ]
            }],
            "blocks": []
        });
        let migrated = migrate_legacy_layout_in_definition(value);
        let layout = migrated.get("layout").and_then(Value::as_array).unwrap();
        let node = &layout[0];
        assert_eq!(node.get("type").and_then(Value::as_str), Some("grid"));
        assert_eq!(node.get("columns").and_then(Value::as_i64), Some(2));
        let widths = node
            .get("columnWidths")
            .and_then(Value::as_array)
            .expect("columnWidths emitted when any column has explicit width");
        assert_eq!(widths.len(), 2);
        let items = node.get("items").and_then(Value::as_array).unwrap();
        assert_eq!(items.len(), 2);
        let left_child = items[0].get("child").unwrap();
        assert_eq!(left_child.get("type").and_then(Value::as_str), Some("grid"));
        assert_eq!(left_child.get("id").and_then(Value::as_str), Some("left"));
        let left_items = left_child.get("items").and_then(Value::as_array).unwrap();
        assert_eq!(
            left_items[0]
                .get("child")
                .unwrap()
                .get("blockId")
                .and_then(Value::as_str),
            Some("a")
        );
    }

    #[test]
    fn migrate_legacy_metric_row_becomes_n_column_grid() {
        let value = json!({
            "definitionVersion": 1,
            "layout": [{
                "id": "mr",
                "type": "metric_row",
                "title": "KPIs",
                "blocks": ["m1", "m2", "m3"]
            }],
            "blocks": []
        });
        let migrated = migrate_legacy_layout_in_definition(value);
        let layout = migrated.get("layout").and_then(Value::as_array).unwrap();
        let node = &layout[0];
        assert_eq!(node.get("type").and_then(Value::as_str), Some("grid"));
        assert_eq!(node.get("columns").and_then(Value::as_i64), Some(3));
        assert_eq!(node.get("title").and_then(Value::as_str), Some("KPIs"));
        let items = node.get("items").and_then(Value::as_array).unwrap();
        assert_eq!(items.len(), 3);
        let ids: Vec<&str> = items
            .iter()
            .map(|item| {
                item.get("child")
                    .unwrap()
                    .get("blockId")
                    .and_then(Value::as_str)
                    .unwrap()
            })
            .collect();
        assert_eq!(ids, vec!["m1", "m2", "m3"]);
    }

    #[test]
    fn migrate_existing_grid_wraps_block_id_into_child() {
        let value = json!({
            "definitionVersion": 1,
            "layout": [{
                "id": "g",
                "type": "grid",
                "columns": 2,
                "items": [
                    {"id": "i1", "blockId": "a", "colSpan": 1},
                    {"id": "i2", "blockId": "b"}
                ]
            }],
            "blocks": []
        });
        let migrated = migrate_legacy_layout_in_definition(value);
        let items = migrated.get("layout").and_then(Value::as_array).unwrap()[0]
            .get("items")
            .and_then(Value::as_array)
            .unwrap();
        assert_eq!(
            items[0]
                .get("child")
                .unwrap()
                .get("blockId")
                .and_then(Value::as_str),
            Some("a")
        );
        assert_eq!(items[0].get("colSpan").and_then(Value::as_i64), Some(1));
        assert_eq!(
            items[1]
                .get("child")
                .unwrap()
                .get("blockId")
                .and_then(Value::as_str),
            Some("b")
        );
        // blockId is removed from the item itself (moved into child).
        assert!(items[0].get("blockId").is_none());
    }

    #[test]
    fn parse_stored_definition_migrates_legacy_layout_through_serde() {
        // Round-trip: a legacy stored definition deserializes successfully
        // (no needs_re_authoring error) because the migration step rewrites
        // the layout before serde_json::from_value runs.
        let value = json!({
            "definitionVersion": 1,
            "blocks": [
                {"id": "intro", "type": "markdown", "markdown": {"content": "Hello"}},
                {"id": "kpi1", "type": "metric", "source": {"schema": ""}, "metric": {"valueField": "v"}},
                {"id": "kpi2", "type": "metric", "source": {"schema": ""}, "metric": {"valueField": "v"}}
            ],
            "layout": [
                {
                    "id": "intro_section",
                    "type": "section",
                    "title": "Intro",
                    "children": [{"id": "n_intro", "type": "block", "blockId": "intro"}]
                },
                {
                    "id": "kpi_row",
                    "type": "metric_row",
                    "blocks": ["kpi1", "kpi2"]
                }
            ]
        });
        let (definition, error) = parse_stored_definition(value, 1);
        assert!(error.is_none(), "legacy layout should migrate cleanly");
        assert_eq!(definition.layout.len(), 2);
        match &definition.layout[0] {
            ReportLayoutNode::Grid(g) => {
                assert_eq!(g.title.as_deref(), Some("Intro"));
                assert_eq!(g.columns, Some(1));
                assert_eq!(g.items.len(), 1);
            }
            other => panic!("expected grid at root, got {other:?}"),
        }
        match &definition.layout[1] {
            ReportLayoutNode::Grid(g) => {
                assert_eq!(g.columns, Some(2));
                assert_eq!(g.items.len(), 2);
            }
            other => panic!("expected grid at root, got {other:?}"),
        }
    }
}
