// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Instance protocol handlers for runtara-core.
//!
//! These handlers process requests from instances (registration, checkpoints,
//! events, signals, etc.), split into focused submodules:
//!
//! - [`registration`]: `handle_register_instance`
//! - [`checkpoint`]: `handle_checkpoint`, `handle_get_checkpoint`, `handle_sleep`
//! - [`signal`]: `handle_poll_signals`, `handle_signal_ack`
//! - [`event`]: `handle_instance_event`, `handle_retry_attempt`
//! - [`status`]: `handle_get_instance_status`
//! - [`types`]: plain Rust request/response types and enums
//! - [`state`]: the shared [`InstanceHandlerState`] handed to every handler
//! - [`mappers`]: enum-to-string helpers used by the HTTP layer

mod checkpoint;
mod event;
mod mappers;
mod registration;
mod signal;
mod state;
mod status;
mod types;

#[cfg(test)]
pub(crate) mod mock_persistence;

pub use self::checkpoint::{handle_checkpoint, handle_get_checkpoint, handle_sleep};
pub(crate) use self::event::event_json_from_bytes;
pub use self::event::{handle_instance_event, handle_retry_attempt};
pub use self::mappers::{map_event_type, map_signal_type, map_status};
pub use self::registration::handle_register_instance;
pub use self::signal::{handle_poll_signals, handle_signal_ack};
pub use self::state::InstanceHandlerState;
pub use self::status::handle_get_instance_status;
pub use self::types::*;
