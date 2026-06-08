// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Agents Library - Reusable agent implementations for workflows
//!
//! This library provides pre-compiled agent implementations that can be
//! linked against workflow-specific code to speed up compilation.
//!
//! This file is compiled once at startup into `libagents.rlib` and then
//! reused across all workflow compilations.

// Re-export all agent modules from agents/ subdirectory
// Only agents that genuinely cannot run as WASM components live here now — the
// native workers for the C-dependent agents (the WASM shells call back to the
// host via /api/internal/agents). Every pure/dual-target agent (transform,
// crypto, csv, datetime, text, xml, utils, http) has been removed; it ships as
// a standalone WASM component under crates/agents/runtara-agent-*.
#[cfg(feature = "native")]
#[path = "agents/compression.rs"]
pub mod compression;
#[path = "agents/extractors/mod.rs"]
pub mod extractors;
#[cfg(feature = "native")]
#[path = "agents/sftp.rs"]
pub mod sftp;
#[cfg(feature = "native")]
#[path = "agents/xlsx.rs"]
pub mod xlsx;

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
