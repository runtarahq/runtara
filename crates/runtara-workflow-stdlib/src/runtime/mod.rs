// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Runtime module for workflow execution
//!
//! Provides the workflow runtime context that integrates with runtara-core
//! via runtara-sdk. This is the only supported runtime mode.
//!
//! The workflow runtime wraps runtara-sdk to provide:
//! - Instance registration and lifecycle management
//! - Checkpointing for crash recovery
//! - Signal handling (pause, cancel, resume)
//! - Heartbeat/tick for liveness monitoring

mod context;
mod error;

pub use context::RuntimeContext;
pub use error::{Error, Result};

// Re-export SDK types for workflows
pub use runtara_sdk::{RuntaraSdk, SdkConfig, SdkError, durable, register_sdk, sdk};
