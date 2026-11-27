// Copyright (C) 2026 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! A workflow's runs queried as records (`docs/workflow-records.md`): the
//! routes people and AI tools read a workflow's store through.

use std::sync::Arc;

use axum::{Json, extract::State, http::StatusCode};
use runtara_environment::run_query::RunQuery;
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::PgPool;
use utoipa::ToSchema;

use crate::api::services::run_query::{self, QueryRunsError};
use crate::engine::Engine;

/// A query over one workflow's runs.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct RunQueryRequest {
    /// The workflow, by id or slug.
    pub workflow: String,
    /// What to read: `latestPerLabel`, `condition`, `unnest`, `select`,
    /// `groupBy`, `aggregates`, `orderBy`, `limit`, `offset`, `count`
    /// (`runtara_environment::run_query::RunQuery`).
    #[schema(value_type = Object)]
    #[serde(default)]
    pub query: RunQuery,
}

/// Query one workflow's runs: filter, group and aggregate them by run
/// columns and by paths into their state, nested objects and arrays
/// included.
#[utoipa::path(
    post,
    path = "/api/runtime/runs/query",
    request_body = RunQueryRequest,
    responses(
        (status = 200, description = "Columns and rows", body = Value),
        (status = 400, description = "The query is not valid", body = Value),
        (status = 404, description = "No such workflow", body = Value),
        (status = 429, description = "Too many queries at once; try again shortly", body = Value),
        (status = 401, description = "Missing Authorization header", body = Value),
        (status = 500, description = "The query failed; the cause is logged, not returned", body = Value),
        (status = 503, description = "The engine that answers queries is not reachable", body = Value),
        (status = 504, description = "The query ran past its 30-second limit", body = Value)
    ),
    tag = "runs"
)]
pub async fn run_query_handler(
    crate::middleware::tenant_auth::OrgId(tenant_id): crate::middleware::tenant_auth::OrgId,
    State(engine): State<Arc<dyn Engine>>,
    State(pool): State<PgPool>,
    Json(request): Json<RunQueryRequest>,
) -> (StatusCode, Json<Value>) {
    match run_query::query(&pool, engine, &tenant_id, &request.workflow, request.query).await {
        Ok(result) => (
            StatusCode::OK,
            Json(json!({ "success": true, "data": result })),
        ),
        Err(error) => {
            let (status, message) = match &error {
                QueryRunsError::Invalid(_) => (StatusCode::BAD_REQUEST, error.to_string()),
                QueryRunsError::NoWorkflow(_) => (StatusCode::NOT_FOUND, error.to_string()),
                QueryRunsError::Busy(_) => (StatusCode::TOO_MANY_REQUESTS, error.to_string()),
                QueryRunsError::Timeout(_) => (StatusCode::GATEWAY_TIMEOUT, error.to_string()),
                QueryRunsError::Unavailable => (StatusCode::SERVICE_UNAVAILABLE, error.to_string()),
                QueryRunsError::Failed(_) => {
                    tracing::warn!(error = %error, "a query over runs failed");
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "could not query the workflow's runs".to_owned(),
                    )
                }
            };
            (status, Json(json!({ "success": false, "error": message })))
        }
    }
}

/// The tenant's workflows, each with the state schema its runs keep: what
/// to write `POST /api/runtime/runs/query` from.
#[utoipa::path(
    get,
    path = "/api/runtime/runs/state",
    responses(
        (status = 200, description = "The workflows and their state schemas", body = Value),
        (status = 401, description = "Missing Authorization header", body = Value)
    ),
    tag = "runs"
)]
pub async fn run_state_handler(
    crate::middleware::tenant_auth::OrgId(tenant_id): crate::middleware::tenant_auth::OrgId,
    State(pool): State<PgPool>,
) -> (StatusCode, Json<Value>) {
    match run_query::describe(&pool, &tenant_id).await {
        Ok(workflows) => (
            StatusCode::OK,
            Json(json!({ "success": true, "data": workflows })),
        ),
        Err(error) => {
            tracing::error!(error = %error, "describing run state failed");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "success": false, "error": "could not read the tenant's workflows" })),
            )
        }
    }
}
