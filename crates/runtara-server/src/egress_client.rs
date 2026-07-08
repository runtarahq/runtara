// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Hardened reqwest client for the internal proxy egress path.
//!
//! The implementation (no-redirect policy + DNS-guarded resolver) moved to
//! `runtara_connections::net` so the OAuth token/refresh/revoke egress shares
//! the exact same guard; this module remains the server-side entry point.

/// Build the shared hardened client used by the internal proxy (`ProxyState`).
/// Construct once at startup and reuse — it pools connections like any
/// `reqwest::Client`.
pub fn build_proxy_client() -> reqwest::Client {
    runtara_connections::net::build_hardened_client()
}
