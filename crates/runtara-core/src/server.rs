// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! QUIC server for runtara-core.
//!
//! Provides the instance server component:
//! - Instance Server: Accepts connections from instances and routes protocol messages

pub mod instance_server;

pub use instance_server::{InstanceServerState, run_instance_server};
