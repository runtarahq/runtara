// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Shared connection management for agents
//!
//! This module provides agent-neutral connection storage and retrieval.
//!
//! Connections can come from two sources:
//! 1. Injected via `_connection` field in input (primary - from connection service)
//! 2. Thread-local storage (legacy - for testing service)
//!
//! Agents should use `resolve_connection()` which tries both sources.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::cell::RefCell;
use std::collections::HashMap;

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

thread_local! {
    /// Thread-local storage for connection data (legacy - used by testing service)
    static CONNECTIONS: RefCell<HashMap<String, RawConnection>> = RefCell::new(HashMap::new());
}

/// Register a connection for the current thread (used by testing service)
pub fn register_connection(connection_id: &str, connection: RawConnection) {
    CONNECTIONS.with(|c| {
        c.borrow_mut().insert(connection_id.to_string(), connection);
    });
}

/// Clear all registered connections for the current thread
pub fn clear_connections() {
    CONNECTIONS.with(|c| {
        c.borrow_mut().clear();
    });
}

/// Get connection data by ID from thread-local storage (legacy)
pub fn get_connection(connection_id: &str) -> Result<RawConnection, String> {
    let result = CONNECTIONS.with(|c| c.borrow().get(connection_id).cloned());

    match result {
        Some(conn) => Ok(conn),
        None => Err(format!(
            "Connection '{}' not found in registered connections.",
            connection_id
        )),
    }
}

/// Parse connection data from input JSON (provided by caller, e.g., agent testing service)
pub fn parse_connection_from_input(input: &Value) -> Option<RawConnection> {
    input
        .get("_connection")
        .and_then(|conn| serde_json::from_value(conn.clone()).ok())
}

/// Resolve a connection from the current capability input or thread-local storage.
///
/// This is the primary function agents should use to get connection data.
/// It tries the following sources in order:
/// 1. `_connection` field in the current capability input (set by executor wrapper)
/// 2. Thread-local connections storage (legacy - for testing service)
///
/// # Arguments
/// * `connection_id` - The connection identifier
///
/// # Returns
/// The resolved connection or an error if not found in any source
pub fn resolve_connection(connection_id: &str) -> Result<RawConnection, String> {
    // Primary: check current capability input (set by executor wrapper)
    if let Some(input) = runtara_dsl::agent_meta::get_current_input() {
        if let Some(conn) = parse_connection_from_input(&input) {
            return Ok(conn);
        }
    }

    // Fallback: thread-local storage (legacy - testing service)
    get_connection(connection_id)
}
