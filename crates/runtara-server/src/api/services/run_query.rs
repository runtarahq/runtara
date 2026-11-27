// Copyright (C) 2026 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! A tenant's workflow runs, queried (`docs/workflow-records.md`): each
//! workflow is its own store, a run label a record, its state the data.
//!
//! The workflow is named here, by id or slug, from the server database,
//! which the API reads. The query itself is the engine's to answer
//! ([`Engine::query_runs`]), in Postgres where the runs are; it takes no
//! SQL.

use std::sync::Arc;

use runtara_environment::run_query::{RunQuery, RunQueryResult};
use serde::Serialize;
use serde_json::Value;
use sqlx::PgPool;

use crate::engine::Engine;
use crate::runtime_client::RuntimeError;

/// Why a query was not answered.
#[derive(Debug, thiserror::Error)]
pub enum QueryRunsError {
    #[error("no workflow '{0}'")]
    NoWorkflow(String),
    #[error("{0}")]
    Invalid(String),
    /// The engine runs as many queries as it may: try again shortly.
    #[error("{0}")]
    Busy(String),
    /// The query exceeded its database time limit.
    #[error("{0}")]
    Timeout(String),
    /// The runtime engine is not currently available.
    #[error("runtime is temporarily unavailable")]
    Unavailable,
    #[error("{0}")]
    Failed(String),
}

/// The tenant's workflow named by `workflow`, an id or a slug.
async fn workflow_id(
    pool: &PgPool,
    tenant_id: &str,
    workflow: &str,
) -> Result<Option<String>, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT workflow_id FROM workflows \
         WHERE tenant_id = $1 AND deleted_at IS NULL AND (workflow_id = $2 OR slug = $2) \
         ORDER BY (workflow_id = $2) DESC LIMIT 1",
    )
    .bind(tenant_id)
    .bind(workflow)
    .fetch_optional(pool)
    .await
}

/// Answers `query` over the tenant's runs of `workflow`.
pub async fn query(
    pool: &PgPool,
    engine: Arc<dyn Engine>,
    tenant_id: &str,
    workflow: &str,
    query: RunQuery,
) -> Result<RunQueryResult, QueryRunsError> {
    if !engine.available() {
        return Err(QueryRunsError::Unavailable);
    }
    let id = workflow_id(pool, tenant_id, workflow)
        .await
        .map_err(|error| QueryRunsError::Failed(format!("reading the workflows: {error}")))?
        .ok_or_else(|| QueryRunsError::NoWorkflow(workflow.to_owned()))?;
    engine
        .query_runs(tenant_id, &id, query)
        .await
        .map_err(|error| match error {
            RuntimeError::InvalidRequest(reason) => QueryRunsError::Invalid(reason),
            RuntimeError::Busy(reason) => QueryRunsError::Busy(reason),
            RuntimeError::QueryTimeout(reason) => QueryRunsError::Timeout(reason),
            // Compatibility fallback if a peer reports errors as SdkError.
            RuntimeError::SdkError(ref text) if text.contains("Invalid request: ") => {
                QueryRunsError::Invalid(
                    text.split_once("Invalid request: ")
                        .map(|(_, reason)| reason.to_owned())
                        .unwrap_or_else(|| text.clone()),
                )
            }
            RuntimeError::SdkError(ref text)
                if text.contains(runtara_environment::run_query::BUSY) =>
            {
                QueryRunsError::Busy(runtara_environment::run_query::BUSY.to_owned())
            }
            other => QueryRunsError::Failed(other.to_string()),
        })
}

/// A workflow whose runs can be queried, with the state its runs keep.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowState {
    pub workflow_id: String,
    pub slug: Option<String>,
    pub name: Option<String>,
    pub description: Option<String>,
    /// The newest version's state schema: each field's type, label and
    /// description, with nested `properties` and array `items`.
    #[schema(value_type = Object)]
    pub state_schema: Value,
}

/// A workflow's id, slug, newest name, description and state schema.
type WorkflowRow = (
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    Value,
);

/// The tenant's workflows, with their newest version's state schema.
pub async fn describe(pool: &PgPool, tenant_id: &str) -> Result<Vec<WorkflowState>, sqlx::Error> {
    let rows: Vec<WorkflowRow> = sqlx::query_as(
        r#"
            SELECT DISTINCT ON (w.workflow_id) w.workflow_id, w.slug,
                   wd.definition->>'name', wd.definition->>'description',
                   COALESCE(wd.definition->'stateSchema', '{}'::jsonb)
            FROM workflows w
            JOIN workflow_definitions wd
              ON wd.tenant_id = w.tenant_id AND wd.workflow_id = w.workflow_id
            WHERE w.tenant_id = $1 AND w.deleted_at IS NULL AND wd.deleted_at IS NULL
            ORDER BY w.workflow_id, wd.version DESC
            "#,
    )
    .bind(tenant_id)
    .fetch_all(pool)
    .await?;
    let mut workflows: Vec<WorkflowState> = rows
        .into_iter()
        .map(
            |(workflow_id, slug, name, description, state_schema)| WorkflowState {
                workflow_id,
                slug,
                name,
                description,
                state_schema,
            },
        )
        .collect();
    workflows.sort_by(|a, b| a.slug.cmp(&b.slug).then(a.workflow_id.cmp(&b.workflow_id)));
    Ok(workflows)
}
