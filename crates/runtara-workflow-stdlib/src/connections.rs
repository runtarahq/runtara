// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Connection management for workflows.
//!
//! This module provides functionality to fetch connections from an external
//! connection service at runtime. The connection service URL is configured
//! at compilation time and baked into the generated workflow binary.
//!
//! ## Connection Service Protocol
//!
//! The connection service should implement:
//! ```text
//! GET {base_url}/{tenant_id}/{connection_id}
//!
//! Response 200:
//! {
//!   "parameters": { ... },          // Connection credentials/config
//!   "integration_id": "bearer",     // Connection type identifier
//!   "connection_subtype": "...",    // Optional subtype
//!   "rate_limit": {                 // Optional rate limit state
//!     "is_limited": false,
//!     "remaining": 100,
//!     "reset_at": 1234567890,       // Unix timestamp
//!     "retry_after_ms": 60000
//!   }
//! }
//! ```

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::time::Duration;
use urlencoding::encode;

/// Context for connection request tracking.
///
/// When provided to `fetch_connection`, these fields are sent as query parameters
/// so the connection service can record which agent/step/scenario is using the connection.
pub struct ConnectionRequestContext<'a> {
    /// Agent or capability identifier (e.g. `shopify_graphql`, `http_request`)
    pub tag: Option<&'a str>,
    /// Step ID within the scenario
    pub step_id: Option<&'a str>,
    /// Scenario ID
    pub scenario_id: Option<&'a str>,
    /// Execution instance ID
    pub instance_id: Option<&'a str>,
}

/// Response from the connection service.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectionResponse {
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

/// Rate limit state from the connection service.
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

/// Error fetching a connection.
#[derive(Debug)]
pub enum ConnectionError {
    /// Connection not found
    NotFound(String),
    /// Rate limited - should wait and retry
    RateLimited {
        connection_id: String,
        retry_after: Duration,
    },
    /// Network or HTTP error
    FetchError(String),
    /// Invalid response from connection service
    InvalidResponse(String),
}

impl std::fmt::Display for ConnectionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConnectionError::NotFound(id) => write!(f, "Connection '{}' not found", id),
            ConnectionError::RateLimited {
                connection_id,
                retry_after,
            } => write!(
                f,
                "Connection '{}' is rate limited, retry after {:?}",
                connection_id, retry_after
            ),
            ConnectionError::FetchError(msg) => write!(f, "Failed to fetch connection: {}", msg),
            ConnectionError::InvalidResponse(msg) => {
                write!(f, "Invalid connection response: {}", msg)
            }
        }
    }
}

impl std::error::Error for ConnectionError {}

/// Fetch a connection from the connection service.
///
/// # Arguments
/// * `service_url` - Base URL of the connection service
/// * `tenant_id` - Tenant identifier
/// * `connection_id` - Connection identifier
/// * `context` - Optional request context for usage tracking
///
/// # Returns
/// Connection response with credentials and rate limit state
pub fn fetch_connection(
    service_url: &str,
    tenant_id: &str,
    connection_id: &str,
    context: Option<&ConnectionRequestContext>,
) -> Result<ConnectionResponse, ConnectionError> {
    let mut url = format!("{}/{}/{}", service_url, tenant_id, connection_id);

    if let Some(ctx) = context {
        let mut params = Vec::new();
        if let Some(tag) = ctx.tag {
            params.push(format!("tag={}", encode(tag)));
        }
        if let Some(step_id) = ctx.step_id {
            params.push(format!("stepId={}", encode(step_id)));
        }
        if let Some(scenario_id) = ctx.scenario_id {
            params.push(format!("scenarioId={}", encode(scenario_id)));
        }
        if let Some(instance_id) = ctx.instance_id {
            params.push(format!("instanceId={}", encode(instance_id)));
        }
        if !params.is_empty() {
            url.push('?');
            url.push_str(&params.join("&"));
        }
    }

    let response = ureq::get(&url)
        .timeout(Duration::from_secs(30))
        .call()
        .map_err(|e| ConnectionError::FetchError(e.to_string()))?;

    if response.status() == 404 {
        return Err(ConnectionError::NotFound(connection_id.to_string()));
    }

    if response.status() == 429 {
        // Rate limited by connection service itself
        let retry_after = response
            .header("Retry-After")
            .and_then(|h| h.parse::<u64>().ok())
            .unwrap_or(60);
        return Err(ConnectionError::RateLimited {
            connection_id: connection_id.to_string(),
            retry_after: Duration::from_secs(retry_after),
        });
    }

    if response.status() != 200 {
        return Err(ConnectionError::FetchError(format!(
            "HTTP {}",
            response.status()
        )));
    }

    let body = response
        .into_string()
        .map_err(|e| ConnectionError::FetchError(e.to_string()))?;

    serde_json::from_str(&body).map_err(|e| ConnectionError::InvalidResponse(e.to_string()))
}

impl RateLimitState {
    /// Get the duration to wait before retrying.
    pub fn wait_duration(&self) -> Duration {
        if let Some(ms) = self.retry_after_ms {
            return Duration::from_millis(ms);
        }

        if let Some(reset_at) = self.reset_at {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64;

            if reset_at > now {
                return Duration::from_secs((reset_at - now) as u64);
            }
        }

        // Default wait time if no specific value provided
        Duration::from_secs(60)
    }
}
