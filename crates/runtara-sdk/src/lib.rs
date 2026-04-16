// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Runtara SDK - High-level client for instance communication with runtara-core.
//!
//! This crate provides an ergonomic API for scenarios/instances to communicate
//! with the runtara-core durable execution engine. It provides strongly-typed
//! methods for all instance lifecycle operations.
//!
//! # Features
//!
//! - **Instance Registration**: Self-register with runtara-core on startup
//! - **Checkpointing**: Save state for durability with automatic resume handling
//! - **Durable Sleep**: Request sleep with automatic checkpoint/wake
//! - **Lifecycle Events**: Send heartbeat, completed, failed events
//! - **Signal Handling**: Poll and handle cancel, pause, resume signals
//! - **Status Queries**: Query instance status and server health
//!
//! # Quick Start
//!
//! ```ignore
//! use runtara_sdk::RuntaraSdk;
//!
//! fn main() -> runtara_sdk::Result<()> {
//!     let mut sdk = RuntaraSdk::from_env()?;
//!
//!     // Connect and register
//!     sdk.connect()?;
//!     sdk.register(None)?;
//!
//!     // Process items with checkpointing
//!     for i in 0..items.len() {
//!         let state = serde_json::to_vec(&my_state)?;
//!         let result = sdk.checkpoint(&format!("item-{}", i), &state)?;
//!
//!         // Check for pause/cancel signals
//!         if result.should_cancel() {
//!             return Err(SdkError::Cancelled.into());
//!         }
//!         if result.should_pause() {
//!             sdk.suspended()?;
//!             return Ok(());
//!         }
//!
//!         if let Some(existing) = result.existing_state() {
//!             // Resuming - restore state and skip
//!             my_state = serde_json::from_slice(existing)?;
//!             continue;
//!         }
//!         // Fresh execution - process item
//!         process_item(&items[i]);
//!     }
//!
//!     sdk.completed(b"result data")?;
//!     Ok(())
//! }
//! ```
//!
//! # Configuration
//!
//! The SDK can be configured via environment variables or programmatically:
//!
//! ## Environment Variables
//!
//! | Variable | Required | Default | Description |
//! |----------|----------|---------|-------------|
//! | `RUNTARA_INSTANCE_ID` | Yes | - | Unique instance identifier |
//! | `RUNTARA_TENANT_ID` | Yes | - | Tenant identifier |
//! | `RUNTARA_HTTP_URL` | No | `http://127.0.0.1:8003` | HTTP API URL |
//! | `RUNTARA_REQUEST_TIMEOUT_MS` | No | `30000` | Request timeout |
//! | `RUNTARA_SIGNAL_POLL_INTERVAL_MS` | No | `1000` | Signal poll rate limit |
//!
//! ## Programmatic Configuration
//!
//! ```ignore
//! use runtara_sdk::HttpSdkConfig;
//!
//! let config = HttpSdkConfig {
//!     instance_id: "my-instance".to_string(),
//!     tenant_id: "my-tenant".to_string(),
//!     base_url: "http://192.168.1.100:8003".to_string(),
//!     request_timeout_ms: 30_000,
//!     signal_poll_interval_ms: 500,
//!     heartbeat_interval_ms: 30_000,
//! };
//!
//! let sdk = RuntaraSdk::new(config)?;
//! ```

mod backend;
mod client;
mod error;
mod registry;
mod types;

// Main types
pub use client::RuntaraSdk;
pub use error::{Result, SdkError};
pub use types::{
    CheckpointResult, CustomSignal, InstanceStatus, RetryConfig, RetryStrategy, Signal, SignalType,
    StatusResponse,
};

// HTTP config export
#[cfg(feature = "http")]
pub use backend::http::HttpSdkConfig;

// Global SDK registry for #[durable] macro
pub use registry::{register_sdk, sdk, stop_heartbeat, try_sdk};

// Cancellation/pause support - allows long-running operations to be interrupted
pub use registry::{
    acknowledge_cancellation, acknowledge_pause, acknowledge_shutdown, is_cancelled,
    trigger_cancellation, with_cancellation, with_cancellation_err,
};

// Re-export the #[durable] macro
pub use runtara_sdk_macros::durable;

// Re-export persistence trait for embedded mode
#[cfg(feature = "embedded")]
pub use runtara_core::persistence::Persistence;
