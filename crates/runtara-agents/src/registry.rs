// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Connection metadata discovery. Workflow capabilities live in WASM components.

use runtara_dsl::agent_meta::ConnectionTypeMeta;

/// Get all statically registered connection type metadata.
pub fn get_all_connection_types() -> impl Iterator<Item = &'static ConnectionTypeMeta> {
    crate::static_registry::CONNECTION_TYPES.iter().copied()
}

/// Find connection type metadata by integration ID.
pub fn find_connection_type(integration_id: &str) -> Option<&'static ConnectionTypeMeta> {
    get_all_connection_types().find(|m| m.integration_id == integration_id)
}
