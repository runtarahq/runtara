// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! QUIC servers for runtara-core.
//!
//! Provides two separate server components:
//! - Instance Server: Accepts connections from instances and routes protocol messages
//! - Management Server: Accepts connections from management clients (health checks, signals, etc.)

pub mod instance_server;
pub mod management_server;

pub use instance_server::{InstanceServerState, run_instance_server};
pub use management_server::{ManagementServerState, run_management_server};
