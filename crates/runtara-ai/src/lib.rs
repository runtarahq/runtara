// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! AI/LLM integration for runtara workflows.
//!
//! This crate provides a synchronous completion abstraction:
//! - `CompletionModel` trait and request builder
//! - Message types (user, assistant, tool calls, tool results)
//! - `OneOrMany<T>` non-empty collection
//! - Provider implementations (OpenAI-compatible)
//! - Provider dispatch (connection → CompletionModel)
//! - Shared types for conversation history, tool call logs, and usage tracking

pub mod completion;
pub mod message;
pub mod one_or_many;
pub mod provider;
pub mod providers;
pub mod types;

// Re-export key types at crate root for convenience.
pub use completion::{CompletionError, CompletionModel, CompletionRequest, CompletionResponse};
pub use message::{AssistantContent, Message, UserContent};
pub use one_or_many::OneOrMany;
