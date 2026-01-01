// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Shared connection management for agents
//!
//! This module provides agent-neutral connection retrieval.
//!
//! Connections are injected via `_connection` field in the capability input
//! by the workflow runtime (fetched from the connection service).
//!
//! Agents should use `resolve_connection()` to extract connection data from input.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Raw connection data returned from host
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawConnection {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub connection_subtype: Option<String>,
    /// Connection type identifier that maps to a connection schema (e.g., bearer, api_key, sftp)
    pub integration_id: String,
    pub parameters: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rate_limit_config: Option<Value>,
}

/// Resolve a connection from the capability input.
///
/// This is the primary function agents should use to get connection data.
/// The connection is expected to be in the `_connection` field of the input,
/// injected by the workflow runtime after fetching from the connection service.
///
/// # Arguments
/// * `input` - The capability input JSON containing the `_connection` field
///
/// # Returns
/// The resolved connection or an error if not found
pub fn resolve_connection(input: &Value) -> Result<RawConnection, String> {
    input
        .get("_connection")
        .ok_or_else(|| {
            "No _connection field in input. Connection not configured for this capability."
                .to_string()
        })
        .and_then(|conn| {
            serde_json::from_value(conn.clone())
                .map_err(|e| format!("Failed to parse _connection: {}", e))
        })
}
