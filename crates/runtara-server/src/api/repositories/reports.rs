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
    let definition: ReportDefinition =
        serde_json::from_value(sanitize_unsupported_report_definition(definition_value))
            .unwrap_or_else(|error| {
                tracing::warn!(
                    error = %error,
                    "Stored report definition could not be read after compatibility sanitization"
                );
                ReportDefinition {
                    definition_version,
                    layout: vec![],
                    views: vec![],
                    filters: vec![],
                    datasets: vec![],
                    blocks: vec![],
                }
            });

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
        created_at: row.try_get::<DateTime<Utc>, _>("created_at")?,
        updated_at: row.try_get::<DateTime<Utc>, _>("updated_at")?,
    })
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
}
