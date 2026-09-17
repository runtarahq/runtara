// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Host support for connection schemas and shared types.
//! Workflow capabilities execute in standalone WASM components.

#[path = "agents/extractors/mod.rs"]
pub mod extractors;

// Shared types
pub mod types;

// Shared connection management
pub mod connections;

// Re-export shared infrastructure
pub mod registry;
mod static_registry;

// Re-export commonly used types for workflow code
pub use serde;
pub use serde_json;
