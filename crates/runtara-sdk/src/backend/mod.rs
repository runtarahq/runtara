// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! SDK backend implementations.
//!
//! This module provides different backends for SDK operations:
//! - `http`: HTTP-based communication with runtara-core
//! - `embedded`: Direct database calls for embedded deployments

#![allow(dead_code)] // Trait methods used internally by durable_sleep implementation

#[cfg(feature = "embedded")]
pub mod embedded;

#[cfg(feature = "http")]
pub mod http;

use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Utc};

use crate::error::Result;
use crate::types::{CheckpointResult, CustomSignal, Signal, SignalType, StatusResponse};

/// Backend trait for SDK operations.
///
/// This trait abstracts the communication layer, allowing the SDK to work
/// with either HTTP-based remote communication or direct embedded calls.
#[async_trait]
pub trait SdkBackend: Send + Sync {
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

    /// Suspend with durable sleep - saves checkpoint and schedules wake.
    ///
    /// This method:
    /// 1. Saves checkpoint state for resume
    /// 2. Sets sleep_until for wake scheduler
    /// 3. Marks instance as suspended with termination_reason "sleeping"
    ///
    /// After calling this, the instance should exit. The environment will
    /// relaunch the instance when the wake time arrives.
    async fn sleep_until(
        &self,
        checkpoint_id: &str,
        wake_at: DateTime<Utc>,
        state: &[u8],
    ) -> Result<()>;

    /// Send a custom event with arbitrary subtype and payload.
    ///
    /// This is a fire-and-forget operation - the event is stored by core
    /// but no response is expected. Core treats the subtype as an opaque string.
    async fn send_custom_event(&self, subtype: &str, payload: Vec<u8>) -> Result<()>;

    /// Record a retry attempt.
    async fn record_retry_attempt(
        &self,
        checkpoint_id: &str,
        attempt_number: u32,
        error_message: Option<&str>,
    ) -> Result<()>;

    /// Get instance status.
    async fn get_status(&self) -> Result<StatusResponse>;

    /// Poll for pending signals (instance-wide and/or custom).
    ///
    /// If `checkpoint_id` is `Some`, polls for a custom signal scoped to that checkpoint.
    /// If `None`, polls for instance-wide signals (cancel/pause/resume).
    ///
    /// Returns `(instance_signal, custom_signal)`.
    async fn poll_signals(
        &self,
        checkpoint_id: Option<&str>,
    ) -> Result<(Option<Signal>, Option<CustomSignal>)>;

    /// Acknowledge a received signal.
    async fn acknowledge_signal(&self, signal_type: SignalType) -> Result<()>;

    /// Get the status of another instance by ID.
    async fn get_instance_status(&self, instance_id: &str) -> Result<StatusResponse>;

    /// Get the instance ID.
    fn instance_id(&self) -> &str;

    /// Get the tenant ID.
    fn tenant_id(&self) -> &str;

    /// Set the sleep_until timestamp for durable sleep.
    async fn set_sleep_until(&self, sleep_until: DateTime<Utc>) -> Result<()>;

    /// Clear the sleep_until timestamp.
    async fn clear_sleep(&self) -> Result<()>;

    /// Get the current sleep_until timestamp for this instance.
    async fn get_sleep_until(&self) -> Result<Option<DateTime<Utc>>>;

    /// Perform a durable sleep with checkpoint and remaining time calculation.
    ///
    /// This method:
    /// 1. Saves a checkpoint with the provided state
    /// 2. Sets sleep_until = now + duration
    /// 3. On resume, calculates remaining time from stored sleep_until
    /// 4. Sleeps for the remaining duration
    /// 5. Clears sleep_until when done
    async fn durable_sleep(
        &self,
        duration: Duration,
        checkpoint_id: &str,
        state: &[u8],
    ) -> Result<()>;
}
