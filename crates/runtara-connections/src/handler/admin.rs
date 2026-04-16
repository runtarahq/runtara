//! Operator-triggered maintenance endpoints for the connections domain.
//!
//! These endpoints are intentionally unauthenticated — the crate owns the
//! HTTP surface, but the host application is expected to mount this router
//! on a **localhost-only / internal** interface. Do not expose to the public
//! internet.
//!
//! # Endpoints
//!
//! - `POST /reencrypt` — re-encrypt all connection parameters with the
//!   currently configured cipher. Accepts an optional `?tenant_id=…` query
//!   parameter to scope the job. Idempotent.

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::Json;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::config::ConnectionsState;
use crate::facade::ConnectionsFacade;
use crate::repository::connections::ReencryptionStats;

#[derive(Debug, Deserialize)]
pub struct ReencryptQuery {
    /// Optional tenant scope. If omitted, re-encrypts all tenants.
    pub tenant_id: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReencryptResponse {
    pub success: bool,
    pub encryption_enabled: bool,
    pub stats: ReencryptionStats,
}

/// `POST /reencrypt`
///
/// Re-encrypts every connection's `connection_parameters` using the current
/// cipher. Safe to run during live traffic (per-row updates).
pub async fn reencrypt_handler(
    State(state): State<ConnectionsState>,
    Query(params): Query<ReencryptQuery>,
) -> Result<(StatusCode, Json<ReencryptResponse>), (StatusCode, Json<Value>)> {
    let facade = ConnectionsFacade::new(state);

    if !facade.is_encryption_enabled() {
        return Err((
            StatusCode::PRECONDITION_FAILED,
            Json(json!({
                "success": false,
                "error": "Encryption is not enabled. Set RUNTARA_CONNECTIONS_ENCRYPTION_KEY before running this migration.",
            })),
        ));
    }

    match facade.reencrypt_all(params.tenant_id.as_deref()).await {
        Ok(stats) => Ok((
            StatusCode::OK,
            Json(ReencryptResponse {
                success: true,
                encryption_enabled: true,
                stats,
            }),
        )),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({
                "success": false,
                "error": format!("re-encryption failed: {}", e),
            })),
        )),
    }
}
