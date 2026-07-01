// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Hardened reqwest client for the internal proxy egress path.
//!
//! Two protections over a bare `reqwest::Client::new()`:
//!
//! * **Redirects are not followed by default** (`redirect::Policy::none`). The
//!   pre-flight SSRF guard and the base-URL pin only see the *first* hop, and
//!   reqwest does not strip application-defined credential headers
//!   (`X-API-Key`, `X-Shopify-Access-Token`, MCP api-key) across a cross-host
//!   redirect — so following a 3xx could exfiltrate a credential or reach the
//!   cloud-metadata endpoint. The proxy returns the 3xx to the agent instead
//!   (F2). A hardened opt-in follow loop is tracked as a follow-up.
//! * **A custom DNS resolver rejects a host outright if ANY resolved address is
//!   private/internal** — the same reject-if-any rule as the pre-flight check.
//!   Because reqwest connects only to addresses the resolver returns, the
//!   address that was vetted is the address that is dialed, closing the
//!   resolve-then-reconnect (DNS rebinding) TOCTOU (F5).

use std::net::SocketAddr;
use std::sync::Arc;

use crate::api::handlers::internal_proxy::host_is_allowlisted_for_egress;
use crate::api::handlers::proxy_url::is_private_ip;

/// reqwest DNS resolver that refuses to hand back addresses for a host that
/// resolves (even partially) to a private/internal range.
struct GuardedResolver;

impl reqwest::dns::Resolve for GuardedResolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        Box::pin(async move {
            let host = name.as_str().to_string();
            // Dev/test escape hatch (RUNTARA_PROXY_ALLOWED_HOSTS) — e.g. a
            // loopback Azurite/MinIO emulator — bypasses the private-IP check.
            let allowlisted = host_is_allowlisted_for_egress(&host);

            let addrs: Vec<SocketAddr> = tokio::net::lookup_host((host.as_str(), 0))
                .await
                .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { Box::new(e) })?
                .collect();

            if !allowlisted && addrs.iter().any(|a| is_private_ip(&a.ip())) {
                return Err(format!(
                    "egress blocked: host '{host}' resolves to a private/internal address"
                )
                .into());
            }

            let iter: reqwest::dns::Addrs = Box::new(addrs.into_iter());
            Ok(iter)
        })
    }
}

/// Build the shared hardened client used by the internal proxy (`ProxyState`).
/// Construct once at startup and reuse — it pools connections like any
/// `reqwest::Client`.
pub fn build_proxy_client() -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .dns_resolver(Arc::new(GuardedResolver))
        .build()
        .expect("hardened proxy client should build")
}
