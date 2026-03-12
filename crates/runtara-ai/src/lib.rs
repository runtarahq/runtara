// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! AI/LLM integration for runtara workflows.
//!
//! This crate provides the `rig` integration layer for AI Agent steps:
//! - Provider dispatch (connection → rig CompletionModel)
//! - Shared types for conversation history, tool call logs, and usage tracking

pub mod provider;
pub mod types;

// Re-export rig for use in generated workflow code
pub use rig;
