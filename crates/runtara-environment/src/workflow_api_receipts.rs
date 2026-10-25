// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Workflow API idempotency receipts in the runtime database.
//!
//! The in-memory store fits a launch that lives in one process. When the
//! process serving a launch's API can restart while the launch runs on
//! (runtara-cloud's binding gateway), receipts must outlive it, so a retried
//! write still gets its first answer.

use async_trait::async_trait;
use runtara_workflow_api::server::{Receipt, ReceiptStore};
use runtara_workflow_api::{ApiError, ErrorCode};
use sqlx::PgPool;

/// Receipts in `workflow_api_receipts`.
#[derive(Clone)]
pub struct PostgresReceiptStore {
    pool: PgPool,
}

impl PostgresReceiptStore {
    /// Receipts in `pool`'s runtime database (migrated by
    /// [`crate::migrations::run`]).
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Delete receipts older than `max_age`: launches whose end was never
    /// observed. Returns how many rows went.
    pub async fn delete_older_than(&self, max_age: std::time::Duration) -> Result<u64, ApiError> {
        let seconds = i64::try_from(max_age.as_secs()).unwrap_or(i64::MAX);
        sqlx::query(
            "DELETE FROM workflow_api_receipts \
             WHERE created_at < now() - make_interval(secs => $1::double precision)",
        )
        .bind(seconds as f64)
        .execute(&self.pool)
        .await
        .map(|done| done.rows_affected())
        .map_err(unavailable)
    }
}

fn unavailable(error: sqlx::Error) -> ApiError {
    tracing::warn!(%error, "workflow API receipt store");
    ApiError::new(ErrorCode::Unavailable, "receipt store unavailable")
}

#[async_trait]
impl ReceiptStore for PostgresReceiptStore {
    async fn get(&self, launch_id: &str, key: &str) -> Result<Option<Receipt>, ApiError> {
        let row: Option<(Vec<u8>, i16, serde_json::Value, Vec<u8>)> = sqlx::query_as(
            "SELECT request_hash, status, headers, body FROM workflow_api_receipts \
             WHERE launch_id = $1 AND idempotency_key = $2",
        )
        .bind(launch_id)
        .bind(key)
        .fetch_optional(&self.pool)
        .await
        .map_err(unavailable)?;
        let Some((hash, status, headers, body)) = row else {
            return Ok(None);
        };
        let request_hash: [u8; 32] = hash
            .try_into()
            .map_err(|_| ApiError::new(ErrorCode::Unavailable, "malformed stored receipt"))?;
        let headers: Vec<(String, String)> = serde_json::from_value(headers)
            .map_err(|_| ApiError::new(ErrorCode::Unavailable, "malformed stored receipt"))?;
        Ok(Some(Receipt {
            request_hash,
            status: u16::try_from(status).unwrap_or(500),
            headers,
            body,
        }))
    }

    async fn put(&self, launch_id: &str, key: &str, receipt: Receipt) -> Result<(), ApiError> {
        let headers = serde_json::to_value(&receipt.headers)
            .map_err(|e| ApiError::new(ErrorCode::Unavailable, e.to_string()))?;
        sqlx::query(
            "INSERT INTO workflow_api_receipts \
             (launch_id, idempotency_key, request_hash, status, headers, body) \
             VALUES ($1, $2, $3, $4, $5, $6) \
             ON CONFLICT (launch_id, idempotency_key) DO NOTHING",
        )
        .bind(launch_id)
        .bind(key)
        .bind(receipt.request_hash.as_slice())
        .bind(i16::try_from(receipt.status).unwrap_or(500))
        .bind(headers)
        .bind(&receipt.body)
        .execute(&self.pool)
        .await
        .map(|_| ())
        .map_err(unavailable)
    }

    async fn forget_launch(&self, launch_id: &str) -> Result<(), ApiError> {
        sqlx::query("DELETE FROM workflow_api_receipts WHERE launch_id = $1")
            .bind(launch_id)
            .execute(&self.pool)
            .await
            .map(|_| ())
            .map_err(unavailable)
    }
}
