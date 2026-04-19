//! Rate Limit Analytics Handlers
//!
//! HTTP handlers for rate limit status endpoints

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::Json,
};
use serde_json::{Value, json};
use sqlx::PgPool;
use std::sync::Arc;

use crate::crypto::CredentialCipher;
use crate::repository::connections::ConnectionRepository;
use crate::service::rate_limits::{RateLimitService, ServiceError};
use crate::types::*;

/// List rate limit status for all tenant connections
///
/// Returns real-time rate limit state from Redis combined with
/// configuration from PostgreSQL for all connections.
/// Optionally includes aggregated period stats based on the interval parameter.
#[cfg_attr(feature = "utoipa", utoipa::path(
    get,
    path = "/api/runtime/rate-limits",
    params(
        ("interval" = Option<String>, Query, description = "Time interval for aggregated stats: 1h, 24h, 7d, 30d (default: 24h)")
    ),
    responses(
        (status = 200, description = "Rate limit status for all connections", body = ListRateLimitsResponse),
        (status = 400, description = "Invalid interval parameter", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    ),
    tag = "rate-limits-controller"
))]
pub async fn list_rate_limits_handler(
    crate::tenant::TenantId(tenant_id): crate::tenant::TenantId,
    State(pool): State<PgPool>,
    State(cipher): State<Arc<dyn CredentialCipher>>,
    Query(query): Query<ListRateLimitsQuery>,
) -> Result<Json<ListRateLimitsResponse>, (StatusCode, Json<Value>)> {
    // Create service with db pool for period stats queries
    let repository = Arc::new(ConnectionRepository::new(pool.clone(), cipher.clone()));
    let service = RateLimitService::with_db_pool(repository, pool);

    // Use interval from query, defaulting to 24h
    let interval = if query.interval.is_empty() {
        "24h"
    } else {
        &query.interval
    };

    match service
        .list_all_rate_limits(&tenant_id, Some(interval))
        .await
    {
        Ok(rate_limits) => {
            let count = rate_limits.len();
            Ok(Json(ListRateLimitsResponse {
                success: true,
                data: rate_limits,
                count,
            }))
        }
        Err(ServiceError::DatabaseError(msg)) => {
            // Check if this is an invalid interval error
            if msg.contains("Invalid interval") {
                Err((
                    StatusCode::BAD_REQUEST,
                    Json(json!({
                        "success": false,
                        "error": "INVALID_INTERVAL",
                        "message": msg
                    })),
                ))
            } else {
                Err((
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({
                        "success": false,
                        "error": "DATABASE_ERROR",
                        "message": msg
                    })),
                ))
            }
        }
        Err(ServiceError::RedisError(msg)) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({
                "success": false,
                "error": "REDIS_ERROR",
                "message": msg
            })),
        )),
        Err(ServiceError::NotFound(_)) => {
            // Should not happen for list operation
            Ok(Json(ListRateLimitsResponse {
                success: true,
                data: vec![],
                count: 0,
            }))
        }
    }
}

/// Get rate limit status for a single connection
///
/// Returns real-time rate limit state from Redis combined with
/// configuration from PostgreSQL for the specified connection.
#[cfg_attr(feature = "utoipa", utoipa::path(
    get,
    path = "/api/runtime/connections/{id}/rate-limit-status",
    params(
        ("id" = String, Path, description = "Connection ID")
    ),
    responses(
        (status = 200, description = "Rate limit status for the connection", body = GetRateLimitStatusResponse),
        (status = 404, description = "Connection not found", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    ),
    tag = "rate-limits-controller"
))]
pub async fn get_connection_rate_limit_status_handler(
    crate::tenant::TenantId(tenant_id): crate::tenant::TenantId,
    State(pool): State<PgPool>,
    State(cipher): State<Arc<dyn CredentialCipher>>,
    Path(id): Path<String>,
) -> Result<Json<GetRateLimitStatusResponse>, (StatusCode, Json<Value>)> {
    // Create service
    let repository = Arc::new(ConnectionRepository::new(pool, cipher.clone()));
    let service = RateLimitService::new(repository);

    match service
        .get_connection_rate_limit_status(&id, &tenant_id)
        .await
    {
        Ok(status) => Ok(Json(GetRateLimitStatusResponse {
            success: true,
            data: status,
        })),
        Err(ServiceError::NotFound(msg)) => Err((
            StatusCode::NOT_FOUND,
            Json(json!({
                "success": false,
                "error": "NOT_FOUND",
                "message": msg
            })),
        )),
        Err(ServiceError::DatabaseError(msg)) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({
                "success": false,
                "error": "DATABASE_ERROR",
                "message": msg
            })),
        )),
        Err(ServiceError::RedisError(msg)) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({
                "success": false,
                "error": "REDIS_ERROR",
                "message": msg
            })),
        )),
    }
}

/// Get time-bucketed rate limit timeline for a connection
///
/// Returns aggregated event counts in time buckets (per-minute, hourly, or daily).
/// Supports filtering by tag to see which agent/step generated the requests.
/// Data is retained for 30 days.
#[cfg_attr(feature = "utoipa", utoipa::path(
    get,
    path = "/api/runtime/connections/{id}/rate-limit-timeline",
    params(
        ("id" = String, Path, description = "Connection ID"),
        ("startTime" = Option<String>, Query, description = "Start time (ISO 8601), defaults to 1 hour ago"),
        ("endTime" = Option<String>, Query, description = "End time (ISO 8601), defaults to now"),
        ("granularity" = Option<String>, Query, description = "Time granularity: minute, hourly, daily (default: minute)"),
        ("tag" = Option<String>, Query, description = "Filter by tag (e.g. agent name like 'shopify_graphql')")
    ),
    responses(
        (status = 200, description = "Time-bucketed rate limit timeline", body = RateLimitTimelineResponse),
        (status = 400, description = "Invalid granularity parameter", body = ErrorResponse),
        (status = 404, description = "Connection not found", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    ),
    tag = "rate-limits-controller"
))]
pub async fn get_connection_rate_limit_timeline_handler(
    crate::tenant::TenantId(tenant_id): crate::tenant::TenantId,
    State(pool): State<PgPool>,
    State(cipher): State<Arc<dyn CredentialCipher>>,
    Path(id): Path<String>,
    Query(query): Query<RateLimitTimelineQuery>,
) -> Result<Json<RateLimitTimelineResponse>, (StatusCode, Json<Value>)> {
    let repository = Arc::new(ConnectionRepository::new(pool.clone(), cipher.clone()));
    let service = RateLimitService::with_db_pool(repository, pool);

    match service
        .get_rate_limit_timeline(&id, &tenant_id, &query)
        .await
    {
        Ok(response) => Ok(Json(response)),
        Err(ServiceError::DatabaseError(msg)) => {
            if msg.contains("Invalid granularity") {
                Err((
                    StatusCode::BAD_REQUEST,
                    Json(json!({
                        "success": false,
                        "error": "INVALID_GRANULARITY",
                        "message": msg
                    })),
                ))
            } else {
                Err((
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({
                        "success": false,
                        "error": "DATABASE_ERROR",
                        "message": msg
                    })),
                ))
            }
        }
        Err(ServiceError::NotFound(msg)) => Err((
            StatusCode::NOT_FOUND,
            Json(json!({
                "success": false,
                "error": "NOT_FOUND",
                "message": msg
            })),
        )),
        Err(ServiceError::RedisError(msg)) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({
                "success": false,
                "error": "REDIS_ERROR",
                "message": msg
            })),
        )),
    }
}

/// Get rate limit history (timeline) for a connection
///
/// Returns historical rate limit events including requests,
/// rate limited events, and retries for the specified connection.
/// Data is retained for 30 days.
#[cfg_attr(feature = "utoipa", utoipa::path(
    get,
    path = "/api/runtime/connections/{id}/rate-limit-history",
    params(
        ("id" = String, Path, description = "Connection ID"),
        ("limit" = Option<i64>, Query, description = "Maximum events to return (default: 100, max: 1000)"),
        ("offset" = Option<i64>, Query, description = "Number of events to skip for pagination"),
        ("event_type" = Option<String>, Query, description = "Filter by event type: request, rate_limited, retry"),
        ("from" = Option<String>, Query, description = "Filter events after this ISO 8601 timestamp"),
        ("to" = Option<String>, Query, description = "Filter events before this ISO 8601 timestamp")
    ),
    responses(
        (status = 200, description = "Rate limit history for the connection", body = RateLimitHistoryResponse),
        (status = 404, description = "Connection not found", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    ),
    tag = "rate-limits-controller"
))]
pub async fn get_connection_rate_limit_history_handler(
    crate::tenant::TenantId(tenant_id): crate::tenant::TenantId,
    State(pool): State<PgPool>,
    State(cipher): State<Arc<dyn CredentialCipher>>,
    Path(id): Path<String>,
    Query(query): Query<RateLimitHistoryQuery>,
) -> Result<Json<RateLimitHistoryResponse>, (StatusCode, Json<Value>)> {
    // Create service with db pool for timeline queries
    let repository = Arc::new(ConnectionRepository::new(pool.clone(), cipher.clone()));
    let service = RateLimitService::with_db_pool(repository, pool);

    match service
        .get_rate_limit_history(&id, &tenant_id, &query)
        .await
    {
        Ok(response) => Ok(Json(response)),
        Err(ServiceError::NotFound(msg)) => Err((
            StatusCode::NOT_FOUND,
            Json(json!({
                "success": false,
                "error": "NOT_FOUND",
                "message": msg
            })),
        )),
        Err(ServiceError::DatabaseError(msg)) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({
                "success": false,
                "error": "DATABASE_ERROR",
                "message": msg
            })),
        )),
        Err(ServiceError::RedisError(msg)) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({
                "success": false,
                "error": "REDIS_ERROR",
                "message": msg
            })),
        )),
    }
}
