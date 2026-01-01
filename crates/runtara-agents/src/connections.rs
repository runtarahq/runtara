// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Shared connection types for agents.
//!
//! Connections are injected via `_connection` field in the capability input
//! by the workflow runtime (fetched from the connection service).
//!
//! Agents access connection data directly from their typed input struct's
//! `_connection: Option<RawConnection>` field.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Raw connection data injected by workflow runtime.
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
