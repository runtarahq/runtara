// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Runtara SDK - High-level client for instance communication with runtara-core.
//!
//! This crate provides an ergonomic API for scenarios/instances to communicate
//! with the runtara-core durable execution engine. It wraps the low-level
//! `runtara-protocol` QUIC client and provides strongly-typed methods for all
//! instance lifecycle operations.
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
//! #[tokio::main]
//! async fn main() -> runtara_sdk::Result<()> {
//!     let mut sdk = RuntaraSdk::localhost("my-instance", "my-tenant")?;
//!
//!     // Connect and register
//!     sdk.connect().await?;
//!     sdk.register(None).await?;
//!
//!     // Process items with checkpointing
//!     for i in 0..items.len() {
//!         let state = serde_json::to_vec(&my_state)?;
//!         let result = sdk.checkpoint(&format!("item-{}", i), &state).await?;
//!
//!         // Check for pause/cancel signals
//!         if result.should_cancel() {
//!             return Err(SdkError::Cancelled.into());
//!         }
//!         if result.should_pause() {
//!             sdk.suspended().await?;
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
//!     sdk.completed(b"result data").await?;
//!     Ok(())
//! }
//! ```
//!
//! # Checkpointing
//!
//! The `checkpoint()` method handles both save and resume semantics, and also
//! returns pending signal information for efficient pause/cancel detection:
//!
//! ```ignore
//! // checkpoint() returns CheckpointResult with:
//! // - existing_state() -> Some(&[u8]) if checkpoint exists (resume case)
//! // - existing_state() -> None if new checkpoint was just saved
//! // - should_pause() / should_cancel() for pending signals
//! for i in 0..items.len() {
//!     let state = serde_json::to_vec(&my_state)?;
//!     let result = sdk.checkpoint(&format!("step-{}", i), &state).await?;
//!
//!     if result.should_pause() {
//!         sdk.suspended().await?;
//!         return Ok(()); // Exit cleanly - will be resumed later
//!     }
//!
//!     if let Some(existing) = result.existing_state() {
//!         my_state = serde_json::from_slice(existing)?;
//!         continue; // Skip - already processed
//!     }
//!     // Process item...
//! }
//! ```
//!
//! # Durable Sleep
//!
//! The SDK supports durable sleep:
//!
//! ```ignore
//! use std::time::Duration;
//!
//! // Sleep is always handled in-process
//! sdk.sleep(
//!     Duration::from_secs(60),    // duration
//!     "after-sleep",              // checkpoint ID for resume
//!     &serialized_state,          // state to restore
//! ).await?;
//!
//! // Continue execution after sleep completes
//! ```
//!
//! # Signal Handling
//!
//! Instances can receive cancel, pause, and resume signals:
//!
//! ```ignore
//! // Simple cancellation check (returns Err(SdkError::Cancelled) if cancelled)
//! sdk.check_cancelled().await?;
//!
//! // Manual signal polling
//! if let Some(signal) = sdk.poll_signal().await? {
//!     match signal.signal_type {
//!         SignalType::Cancel => {
//!             sdk.acknowledge_signal(SignalType::Cancel, true).await?;
//!             return Err(SdkError::Cancelled);
//!         }
//!         SignalType::Pause => {
//!             sdk.checkpoint("paused", &state).await?;
//!             sdk.acknowledge_signal(SignalType::Pause, true).await?;
//!             sdk.suspended().await?;
//!         }
//!         SignalType::Resume => {
//!             sdk.acknowledge_signal(SignalType::Resume, true).await?;
//!         }
//!     }
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
//! | `RUNTARA_SERVER_ADDR` | No | `127.0.0.1:8001` | Server address |
//! | `RUNTARA_SERVER_NAME` | No | `localhost` | TLS server name |
//! | `RUNTARA_SKIP_CERT_VERIFICATION` | No | `false` | Skip TLS verification |
//! | `RUNTARA_CONNECT_TIMEOUT_MS` | No | `10000` | Connection timeout |
//! | `RUNTARA_REQUEST_TIMEOUT_MS` | No | `30000` | Request timeout |
//! | `RUNTARA_SIGNAL_POLL_INTERVAL_MS` | No | `1000` | Signal poll rate limit |
//!
//! ## Programmatic Configuration
//!
//! ```ignore
//! use runtara_sdk::SdkConfig;
//!
//! let config = SdkConfig::new("my-instance", "my-tenant")
//!     .with_server_addr("192.168.1.100:7001".parse()?)
//!     .with_skip_cert_verification(true)
//!     .with_signal_poll_interval_ms(500);
//!
//! let sdk = RuntaraSdk::new(config)?;
//! ```

mod backend;
mod client;
mod error;
mod registry;
mod types;

#[cfg(feature = "quic")]
mod config;
#[cfg(feature = "quic")]
mod events;
#[cfg(feature = "quic")]
mod signals;

// Main types
pub use client::RuntaraSdk;
pub use error::{Result, SdkError};
pub use types::{
    CheckpointResult, CustomSignal, InstanceStatus, RetryConfig, RetryStrategy, Signal, SignalType,
    StatusResponse,
};

// QUIC-specific exports
#[cfg(feature = "quic")]
pub use config::SdkConfig;

// Global SDK registry for #[durable] macro
pub use registry::{register_sdk, sdk, try_sdk};

// Re-export the #[durable] macro
pub use runtara_sdk_macros::durable;

// Re-export protocol client config for advanced usage
#[cfg(feature = "quic")]
pub use runtara_protocol::RuntaraClientConfig;

// Re-export persistence trait for embedded mode
#[cfg(feature = "embedded")]
pub use runtara_core::persistence::Persistence;
