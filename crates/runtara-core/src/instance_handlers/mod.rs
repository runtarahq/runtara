// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Instance protocol handlers for runtara-core.
//!
//! These handlers process requests from instances (registration, checkpoints,
//! events, signals, etc.), split into focused submodules:
//!
//! - [`crate::instance_handlers::handle_register_instance`] — registration
//! - [`crate::instance_handlers::handle_checkpoint`] and
//!   [`crate::instance_handlers::handle_get_checkpoint`] — checkpoint access
//! - [`crate::instance_handlers::handle_sleep`] — durable sleep
//! - [`crate::instance_handlers::handle_poll_signals`] and
//!   [`crate::instance_handlers::handle_signal_ack`] — lifecycle signals
//! - [`crate::instance_handlers::handle_instance_event`] and
//!   [`crate::instance_handlers::handle_retry_attempt`] — event ingestion
//! - [`crate::instance_handlers::handle_get_instance_status`] — status queries
//!
//! Request/response types and [`crate::instance_handlers::InstanceHandlerState`]
//! are re-exported alongside the handlers.

mod checkpoint;
mod event;
mod registration;
mod signal;
mod state;
mod status;
mod types;

/// In-memory [`Persistence`](crate::persistence::Persistence) mock for handler
/// tests.
///
/// Compiled for this crate's own tests, and for downstream crates that enable
/// the `test-support` feature — chiefly `runtara-server`, whose instance HTTP
/// router drives these same handlers and needs to test that wiring without a
/// external service.
#[cfg(any(test, feature = "test-support"))]
pub mod mock_persistence;

pub use self::checkpoint::{handle_checkpoint, handle_get_checkpoint, handle_sleep};
pub use self::event::{handle_instance_event, handle_retry_attempt};
pub use self::registration::handle_register_instance;
pub use self::signal::{handle_poll_signals, handle_signal_ack};
pub use self::state::{InstanceEventObserver, InstanceHandlerState};
pub use self::status::handle_get_instance_status;
pub use self::types::*;
