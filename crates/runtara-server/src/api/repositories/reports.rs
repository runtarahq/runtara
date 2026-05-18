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

/// Parse a stored definition JSON into a `ReportDefinition`. Two
/// failure modes return an empty stub plus a `needs_re_authoring`
/// message so the FE can surface a clean state instead of letting the
/// report look superficially fine:
///
/// 1. The stored JSON carries legacy markers that the cutover spec
///    flagged as "stop loading" — `markdown` root field or
///    `markdown` layout nodes from the pre-Phase-1 wrapper. Detected
///    explicitly so the original JSON is preserved on the server (the
///    operator can inspect it via `get_report` and re-author through
///    MCP), rather than silently coerced into something that almost
///    works.
/// 2. Serde fails because the JSON doesn't fit the current
///    `ReportDefinition` shape at all (unknown variants, missing
///    required fields, etc.).
///
/// Phase 9's legacy *container* migration (`section` / `columns` /
/// `metric_row` -> `grid`) is a structural rewrite with no information
/// loss, so it runs first; only after that do we fall through to the
/// stub fallback.
fn parse_stored_definition(
    value: Value,
    definition_version: i32,
) -> (ReportDefinition, Option<String>) {
    if let Some(reason) = detect_unsupported_legacy_shape(&value) {
        tracing::warn!(
            "Stored report definition carries legacy shape markers and is flagged for re-authoring: {reason}"
        );
        return (empty_definition(definition_version), Some(reason));
    }
    let migrated = migrate_legacy_layout_in_definition(value);
    match serde_json::from_value::<ReportDefinition>(migrated) {
        Ok(definition) => (definition, None),
        Err(error) => {
            tracing::warn!(
                error = %error,
                "Stored report definition could not be deserialized into the current shape"
            );
            (
                empty_definition(definition_version),
                Some(error.to_string()),
            )
        }
    }
}

fn empty_definition(definition_version: i32) -> ReportDefinition {
    ReportDefinition {
        definition_version,
        layout: runtara_report_dsl::types::default_root_grid(),
        views: vec![],
        filters: vec![],
        datasets: vec![],
        blocks: vec![],
    }
}

/// Detect legacy-shape markers the cutover spec said should refuse to
/// load. Returns a human-readable reason string when found.
fn detect_unsupported_legacy_shape(value: &Value) -> Option<String> {
    let object = value.as_object()?;
    if object.contains_key("markdown") {
        return Some(
            "Stored report uses the pre-Phase-1 `markdown` root field. \
             Re-author through MCP or the wizard using markdown blocks."
                .to_string(),
        );
    }
    if layout_contains_markdown_node(object.get("layout")) {
        return Some(
            "Stored report layout contains a legacy `markdown` layout node. \
             Re-author through MCP or the wizard."
                .to_string(),
        );
    }
    if let Some(Value::Array(views)) = object.get("views") {
        for view in views {
            if let Value::Object(view_obj) = view
                && layout_contains_markdown_node(view_obj.get("layout"))
            {
                return Some(
                    "Stored report view contains a legacy `markdown` layout node. \
                     Re-author through MCP or the wizard."
                        .to_string(),
                );
            }
        }
    }
    None
}

fn layout_contains_markdown_node(value: Option<&Value>) -> bool {
    let Some(Value::Array(nodes)) = value else {
        return false;
    };
    nodes.iter().any(|node| match node {
        Value::Object(object) => {
            if object.get("type").and_then(Value::as_str) == Some("markdown") {
                return true;
            }
            if layout_contains_markdown_node(object.get("children")) {
                return true;
            }
            if let Some(Value::Array(columns)) = object.get("columns") {
                for column in columns {
                    if let Value::Object(col) = column
                        && layout_contains_markdown_node(col.get("children"))
                    {
                        return true;
                    }
                }
            }
            false
        }
        _ => false,
    })
}

/// Walk `definition.layout` and each `view.layout` in the raw JSON tree
/// and translate every legacy layout container into the new `grid`
/// shape, then wrap the resulting node list in a mandatory root grid
/// (Phase 10). Pure structural rewrite — no information loss.
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
            *layout = migrate_layout_field(std::mem::replace(layout, Value::Null), "root");
        }
        if let Some(Value::Array(views)) = object.get_mut("views") {
            for view in views {
                if let Value::Object(view_object) = view {
                    let view_id = view_object
                        .get("id")
                        .and_then(Value::as_str)
                        .map(|s| format!("view_{}_root", s))
                        .unwrap_or_else(|| "view_root".to_string());
                    if let Some(layout) = view_object.get_mut("layout") {
                        *layout =
                            migrate_layout_field(std::mem::replace(layout, Value::Null), &view_id);
                    }
                }
            }
        }
    }
    value
}

/// Translate the `definition.layout` (or `view.layout`) wire value into
/// the current shape: a single root [`ReportGridLayoutNode`]. Handles
/// three input shapes:
///
/// 1. `Vec<ReportLayoutNode>` (legacy pre-Phase-10): each item migrated
///    individually (legacy `section` / `columns` / `metric_row` → grid),
///    then wrapped in a 1-column root grid whose `items[]` carry the
///    migrated nodes.
/// 2. A bare grid object (legacy post-Phase-9): used directly as the
///    root grid (with any legacy `blockId` shorthand inside items
///    rewritten via [`migrate_layout_node`]).
/// 3. Anything else (missing / non-object / non-array): replaced with
///    an empty root grid.
fn migrate_layout_field(value: Value, root_id_hint: &str) -> Value {
    match value {
        Value::Array(nodes) => {
            let items: Vec<Value> = nodes
                .into_iter()
                .enumerate()
                .map(|(i, node)| {
                    let migrated = migrate_layout_node(node);
                    grid_item(&format!("{}_i{}", root_id_hint, i), migrated)
                })
                .collect();
            let mut grid = serde_json::Map::new();
            grid.insert("id".into(), Value::String(root_id_hint.to_string()));
            grid.insert("columns".into(), Value::from(1i64));
            grid.insert("items".into(), Value::Array(items));
            Value::Object(grid)
        }
        Value::Object(_) => {
            // The wire value is already a single object — treat it as a
            // grid node and run the legacy-grid migration. If it was a
            // legacy non-grid container at the root (section/columns/
            // metric_row) we still get a Vec back via `migrate_layout_node`
            // wrapping logic, so this branch is structurally safe for
            // the (rare) case where authors wrote a bare grid at root in
            // older wire formats.
            let migrated = migrate_layout_node(value);
            // After migration, the result must be a grid object — if a
            // bare `block` node was supplied at root, wrap it under a
            // 1-column grid so the root-must-be-grid invariant holds.
            if migrated
                .get("type")
                .and_then(Value::as_str)
                .map(|t| t == "grid" || t == "block")
                == Some(true)
            {
                if migrated.get("type").and_then(Value::as_str) == Some("grid") {
                    // Strip `type` from the migrated grid, since the
                    // ReportGridLayoutNode wire form doesn't carry one
                    // — the outer enum dispatch is moot now that the
                    // root is typed.
                    let Value::Object(mut grid_obj) = migrated else {
                        unreachable!("migrated had Object type")
                    };
                    grid_obj.remove("type");
                    Value::Object(grid_obj)
                } else {
                    // Single block — wrap in a 1-column root grid.
                    let mut grid = serde_json::Map::new();
                    grid.insert("id".into(), Value::String(root_id_hint.to_string()));
                    grid.insert("columns".into(), Value::from(1i64));
                    grid.insert(
                        "items".into(),
                        Value::Array(vec![grid_item(&format!("{}_i0", root_id_hint), migrated)]),
                    );
                    Value::Object(grid)
                }
            } else {
                // Already a typeless root-grid object (today's wire
                // form). Pass-through.
                migrated
            }
        }
        _ => {
            // Null / missing / scalar — fall back to an empty root grid.
            let mut grid = serde_json::Map::new();
            grid.insert("id".into(), Value::String(root_id_hint.to_string()));
            grid.insert("columns".into(), Value::from(1i64));
            grid.insert("items".into(), Value::Array(vec![]));
            Value::Object(grid)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::dto::reports::ReportLayoutNode;

    #[test]
    fn legacy_markdown_root_field_flags_needs_re_authoring_without_stripping() {
        // Per the Phase 8 cutover spec, legacy stored definitions stop
        // loading — the FE/MCP should surface a needs_re_authoring state
        // instead of the server silently coercing the report into
        // something almost-but-not-quite right.
        let value = json!({
            "definitionVersion": 1,
            "markdown": "# Legacy wrapper",
            "layout": [],
            "filters": [],
            "blocks": []
        });
        let (definition, error) = parse_stored_definition(value, 1);
        let reason = error.expect("legacy markdown root must flag needs_re_authoring");
        assert!(
            reason.contains("markdown"),
            "reason should call out the legacy markdown field: {reason}"
        );
        assert!(definition.layout.items.is_empty());
        assert!(definition.blocks.is_empty());
    }

    #[test]
    fn legacy_markdown_layout_node_flags_needs_re_authoring() {
        let value = json!({
            "definitionVersion": 1,
            "layout": [
                {"id": "intro", "type": "markdown", "content": "# Intro"},
                {"id": "items_node", "type": "block", "blockId": "items"}
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
        let (definition, error) = parse_stored_definition(value, 1);
        let reason = error.expect("legacy markdown layout node must flag needs_re_authoring");
        assert!(reason.contains("markdown"));
        // No silent extraction of the block — the empty stub is returned
        // so the FE/MCP surface routes to a re-authoring banner instead
        // of pretending the report works.
        assert!(definition.layout.items.is_empty());
        assert!(definition.blocks.is_empty());
    }

    #[test]
    fn legacy_markdown_layout_node_in_view_flags_needs_re_authoring() {
        let value = json!({
            "definitionVersion": 1,
            "layout": [],
            "filters": [],
            "blocks": [],
            "views": [{
                "id": "v1",
                "title": "View",
                "layout": [{"id": "intro", "type": "markdown", "content": "x"}]
            }]
        });
        let (_, error) = parse_stored_definition(value, 1);
        assert!(error.is_some());
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
        assert!(definition.layout.items.is_empty());
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
    fn migrate_legacy_section_becomes_grid_inside_root_grid() {
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
        let root = migrated
            .get("layout")
            .and_then(Value::as_object)
            .expect("root layout is now a grid object");
        assert_eq!(root.get("id").and_then(Value::as_str), Some("root"));
        assert!(
            root.get("type").is_none(),
            "root grid wire form is typeless"
        );
        let root_items = root.get("items").and_then(Value::as_array).unwrap();
        assert_eq!(root_items.len(), 1);
        let child = root_items[0].get("child").unwrap();
        assert_eq!(child.get("type").and_then(Value::as_str), Some("grid"));
        assert_eq!(child.get("title").and_then(Value::as_str), Some("Heading"));
        assert_eq!(
            child.get("description").and_then(Value::as_str),
            Some("Sub")
        );
        assert_eq!(child.get("columns").and_then(Value::as_i64), Some(1));
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
        let root_items = migrated
            .get("layout")
            .and_then(|v| v.get("items"))
            .and_then(Value::as_array)
            .unwrap();
        let cols = root_items[0].get("child").unwrap();
        assert_eq!(cols.get("type").and_then(Value::as_str), Some("grid"));
        assert_eq!(cols.get("columns").and_then(Value::as_i64), Some(2));
        let widths = cols
            .get("columnWidths")
            .and_then(Value::as_array)
            .expect("columnWidths emitted when any column has explicit width");
        assert_eq!(widths.len(), 2);
        let items = cols.get("items").and_then(Value::as_array).unwrap();
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
        let root_items = migrated
            .get("layout")
            .and_then(|v| v.get("items"))
            .and_then(Value::as_array)
            .unwrap();
        let mr = root_items[0].get("child").unwrap();
        assert_eq!(mr.get("type").and_then(Value::as_str), Some("grid"));
        assert_eq!(mr.get("columns").and_then(Value::as_i64), Some(3));
        assert_eq!(mr.get("title").and_then(Value::as_str), Some("KPIs"));
        let items = mr.get("items").and_then(Value::as_array).unwrap();
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
        let inner_grid = migrated
            .get("layout")
            .and_then(|v| v.get("items"))
            .and_then(Value::as_array)
            .unwrap()[0]
            .get("child")
            .unwrap();
        let items = inner_grid.get("items").and_then(Value::as_array).unwrap();
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
    fn migrate_bare_block_at_root_wraps_in_root_grid() {
        // Pre-Phase-9 reports could carry a single bare `block` node at
        // root. After migration, it must end up inside the new root
        // grid's items.
        let value = json!({
            "definitionVersion": 1,
            "layout": [
                {"id": "n_intro", "type": "block", "blockId": "intro"}
            ],
            "blocks": [{"id": "intro", "type": "markdown", "markdown": {"content": "x"}}]
        });
        let migrated = migrate_legacy_layout_in_definition(value);
        let root = migrated.get("layout").and_then(Value::as_object).unwrap();
        let items = root.get("items").and_then(Value::as_array).unwrap();
        assert_eq!(items.len(), 1);
        let child = items[0].get("child").unwrap();
        assert_eq!(child.get("type").and_then(Value::as_str), Some("block"));
        assert_eq!(child.get("blockId").and_then(Value::as_str), Some("intro"));
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
        // Root grid wraps the two legacy containers.
        assert_eq!(definition.layout.id, "root");
        assert_eq!(definition.layout.items.len(), 2);
        match definition.layout.items[0].child.as_ref() {
            ReportLayoutNode::Grid(g) => {
                assert_eq!(g.title.as_deref(), Some("Intro"));
                assert_eq!(g.columns, Some(1));
                assert_eq!(g.items.len(), 1);
            }
            other => panic!("expected migrated section as grid, got {other:?}"),
        }
        match definition.layout.items[1].child.as_ref() {
            ReportLayoutNode::Grid(g) => {
                assert_eq!(g.columns, Some(2));
                assert_eq!(g.items.len(), 2);
            }
            other => panic!("expected migrated metric_row as grid, got {other:?}"),
        }
    }
}
