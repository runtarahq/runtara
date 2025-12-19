// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! SDK backend implementations.
//!
//! This module provides different backends for SDK operations:
//! - `quic`: QUIC-based communication with runtara-core (default)
//! - `embedded`: Direct database calls for embedded deployments

#[cfg(feature = "quic")]
pub mod quic;

#[cfg(feature = "embedded")]
pub mod embedded;

#[cfg(feature = "quic")]
use std::any::Any;

use async_trait::async_trait;

use crate::error::Result;
use crate::types::{CheckpointResult, StatusResponse};

/// Backend trait for SDK operations.
///
/// This trait abstracts the communication layer, allowing the SDK to work
/// with either QUIC-based remote communication or direct embedded calls.
#[async_trait]
pub trait SdkBackend: Send + Sync {
    /// Return self as Any for downcasting to concrete types.
    /// Only used with QUIC feature for QUIC-specific operations.
    #[cfg(feature = "quic")]
    fn as_any(&self) -> &dyn Any;
    /// Connect to the backend (no-op for embedded).
    async fn connect(&self) -> Result<()>;

    /// Check if connected.
    async fn is_connected(&self) -> bool;

    /// Close the connection (no-op for embedded).
    async fn close(&self);

    /// Register an instance.
    async fn register(&self, checkpoint_id: Option<&str>) -> Result<()>;

    /// Checkpoint with the given ID and state.
    async fn checkpoint(&self, checkpoint_id: &str, state: &[u8]) -> Result<CheckpointResult>;

    /// Get a checkpoint by ID (read-only).
    async fn get_checkpoint(&self, checkpoint_id: &str) -> Result<Option<Vec<u8>>>;

    /// Send a heartbeat event.
    async fn heartbeat(&self) -> Result<()>;

    /// Send a completed event.
    async fn completed(&self, output: &[u8]) -> Result<()>;

    /// Send a failed event.
    async fn failed(&self, error: &str) -> Result<()>;

    /// Send a suspended event.
    async fn suspended(&self) -> Result<()>;

    /// Record a retry attempt.
    async fn record_retry_attempt(
        &self,
        checkpoint_id: &str,
        attempt_number: u32,
        error_message: Option<&str>,
    ) -> Result<()>;

    /// Get instance status.
    async fn get_status(&self) -> Result<StatusResponse>;

    /// Get the instance ID.
    fn instance_id(&self) -> &str;

    /// Get the tenant ID.
    fn tenant_id(&self) -> &str;
}
