// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Capability dispatch for generated scenario binaries.
//!
//! This default implementation delegates to `registry::execute_capability()`,
//! which uses inventory-based dynamic dispatch.
//!
//! Product stdlibs (e.g., smo-stdlib) can override this module with a static
//! dispatch table that eliminates the inventory dependency from scenario binaries.

/// Execute a capability by module and capability_id.
///
/// Default implementation: delegates to the inventory-based registry.
pub fn execute_capability(
    module: &str,
    capability_id: &str,
    input: serde_json::Value,
) -> Result<serde_json::Value, String> {
    crate::registry::execute_capability(module, capability_id, input)
}
