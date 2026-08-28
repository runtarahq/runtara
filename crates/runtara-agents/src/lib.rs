// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Agents Library - Reusable agent implementations for workflows
//!
//! This library provides pre-compiled agent implementations that can be
//! linked against workflow-specific code to speed up compilation.
//!
//! This file is compiled once at startup into `libagents.rlib` and then
//! reused across all workflow compilations.

// Re-export all agent modules from agents/ subdirectory.
//
// Only SFTP is left: libssh2 is a C library with no wasm32-wasip2 target, so
// its WASM shell still calls back to the host via /api/internal/agents. Every
// other agent runs entirely in the sandbox as a standalone WASM component under
// crates/agents/runtara-agent-*. Compression and XLSX were the last two to
// move — their C dependencies turned out to be optional (`zip`'s bzip2/zstd/
// lzma backends, which those capabilities never used) or absent (`calamine` is
// pure Rust).
#[path = "agents/extractors/mod.rs"]
pub mod extractors;
#[cfg(feature = "native")]
#[path = "agents/sftp.rs"]
pub mod sftp;

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
