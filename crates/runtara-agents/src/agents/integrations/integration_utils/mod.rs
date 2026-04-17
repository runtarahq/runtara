// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Shared scaffolding for third-party integration capabilities.
//!
//! This module concentrates the previously-duplicated HTTP client setup,
//! pagination, connection validation, and error taxonomy that every
//! integration under `integrations/` used to hand-roll. See
//! [`client`], [`pagination`], [`error`], [`connection`], and [`url`].

pub mod client;
pub mod connection;
pub mod error;
pub mod pagination;
pub mod url;

pub use client::{DEFAULT_TIMEOUT_MS, ProxyHttpClient, ProxyRequest};
pub use connection::require_connection;
pub use error::{ErrorCategory, IntegrationError};
pub use pagination::{Page, PageCursor, extract_page};
pub use url::urlencoded;
