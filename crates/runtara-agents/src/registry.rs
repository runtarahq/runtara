// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Connection metadata discovery. Workflow capabilities live in WASM components.

use runtara_dsl::agent_meta::ConnectionTypeMeta;

/// Every connection type: the compiled ones, then those agents declared
/// ([`crate::derived`]).
pub fn get_all_connection_types() -> impl Iterator<Item = &'static ConnectionTypeMeta> {
    let derived: Vec<&'static ConnectionTypeMeta> =
        crate::derived::all().iter().map(|t| t.meta).collect();
    crate::static_registry::CONNECTION_TYPES
        .iter()
        .copied()
        .chain(derived)
}

/// Find connection type metadata by integration ID, compiled or declared.
pub fn find_connection_type(integration_id: &str) -> Option<&'static ConnectionTypeMeta> {
    find_compiled_connection_type(integration_id)
        .or_else(|| crate::derived::find(integration_id).map(|t| t.meta))
}

/// Find a connection type compiled into runtara (not one an agent declared).
pub fn find_compiled_connection_type(integration_id: &str) -> Option<&'static ConnectionTypeMeta> {
    crate::static_registry::CONNECTION_TYPES
        .iter()
        .copied()
        .find(|m| m.integration_id == integration_id)
}
