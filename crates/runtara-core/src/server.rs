// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! HTTP server for runtara-core.
//!
//! Provides the instance server component over HTTP/JSON.

#[allow(missing_docs)]
/// HTTP server for the instance protocol.
pub mod http_server;

pub use http_server::instance_http_router;

use crate::instance_handlers::InstanceHandlerState;

/// Shared state for instance server.
pub type InstanceServerState = InstanceHandlerState;
