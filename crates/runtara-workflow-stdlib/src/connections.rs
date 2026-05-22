// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Connection envelope types used by generated workflow code.
//!
//! Historically this module also exposed a `fetch_connection` function that
//! made an HTTP GET to the connection service from inside the workflow
//! .wasm. That model put credentials in-process. The current architecture
//! never puts secrets in the workflow: the codegen builds a stub
//! `ConnectionResponse` carrying only the `connection_id`, and `runtara-http`
//! attaches the `X-Runtara-Connection-Id` header on outbound calls so the
//! host-side proxy injects credentials server-side. `fetch_connection` and
//! its error/context types were dead code in that flow and have been
//! removed.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Response from the connection service.
///
/// In the current proxy-based architecture only `connection_id` is
/// meaningfully populated by the codegen; the remaining fields are kept
/// because some agent stubs pass the whole struct downstream and a few
/// older code paths still consume the JSON shape.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectionResponse {
    /// Connection ID (for proxy-based credential injection)
    #[serde(default)]
    pub connection_id: String,

    /// Connection credentials and configuration
    pub parameters: Value,

    /// Connection type identifier (e.g., "sftp", "bearer", "api_key")
    pub integration_id: String,

    /// Optional connection subtype
    #[serde(skip_serializing_if = "Option::is_none")]
    pub connection_subtype: Option<String>,

    /// Rate limit state (if applicable)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rate_limit: Option<RateLimitState>,
}

/// Rate limit state carried inside `ConnectionResponse`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RateLimitState {
    /// Whether the connection is currently rate limited
    pub is_limited: bool,

    /// Remaining requests in the current window (if known)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remaining: Option<u32>,

    /// Unix timestamp when the rate limit resets
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reset_at: Option<i64>,

    /// Milliseconds to wait before retrying
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_after_ms: Option<u64>,
}
