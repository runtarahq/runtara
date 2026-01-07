// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Agents Library - Reusable agent implementations for scenarios
//!
//! This library provides pre-compiled agent implementations that can be
//! linked against scenario-specific code to speed up compilation.
//!
//! This file is compiled once at startup into `libagents.rlib` and then
//! reused across all scenario compilations.

// Re-export all agent modules from agents/ subdirectory
#[path = "agents/csv.rs"]
pub mod csv;
#[path = "agents/datetime.rs"]
pub mod datetime;
#[path = "agents/extractors/mod.rs"]
pub mod extractors;
#[path = "agents/http.rs"]
pub mod http;
#[path = "agents/sftp.rs"]
pub mod sftp;
#[path = "agents/text.rs"]
pub mod text;
#[path = "agents/transform.rs"]
pub mod transform;
#[path = "agents/utils.rs"]
pub mod utils;
#[path = "agents/xml.rs"]
pub mod xml;

// Shared types
pub mod types;

// Shared connection management
pub mod connections;

// Re-export shared infrastructure
pub mod registry;

// Re-export commonly used types for scenario code
pub use serde;
pub use serde_json;
