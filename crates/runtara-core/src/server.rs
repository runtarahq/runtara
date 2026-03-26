// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! QUIC server for runtara-core.
//!
//! Provides the instance server component:
//! - Instance Server: Accepts connections from instances and routes protocol messages

pub mod instance_server;

#[cfg(feature = "http")]
#[allow(missing_docs)]
/// HTTP server for the instance protocol (alternative to QUIC).
pub mod http_server;

pub use instance_server::{InstanceServerState, handle_connection, run_instance_server};
