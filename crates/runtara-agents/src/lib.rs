// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Host support for connection schemas, shared types, and server file storage.
//! Workflow capabilities execute in standalone WASM components.

#[path = "agents/extractors/mod.rs"]
pub mod extractors;

// Shared types
pub mod types;

// Shared connection management
pub mod connections;

// Standalone S3-compatible client used by the server's file-storage service
// (default file storage, attachments). Not a workflow agent — the S3 *agent*
// capabilities now live in the `runtara-agent-s3-storage` WASM component.
pub mod s3_client;

// Re-export shared infrastructure
pub mod registry;
mod static_registry;

// Re-export commonly used types for workflow code
pub use serde;
pub use serde_json;
