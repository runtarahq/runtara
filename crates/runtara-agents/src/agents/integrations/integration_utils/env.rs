// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Scenario-runtime environment lookups for integration capabilities.
//!
//! These env vars are stable for the lifetime of a scenario process — set
//! once by the runner at launch time and never mutated. Caching the values in
//! `OnceLock`s replaces the per-call `env::var` reads that capabilities used
//! to perform.

use std::sync::OnceLock;

/// Tenant ID forwarded from the host (empty string if unset).
pub fn tenant_id() -> &'static str {
    static TENANT_ID: OnceLock<String> = OnceLock::new();
    TENANT_ID
        .get_or_init(|| std::env::var("RUNTARA_TENANT_ID").unwrap_or_default())
        .as_str()
}

/// Base URL of the connection service (used by integrations to resolve
/// `integration_id` from an abstract `connection_id`).
pub fn connection_service_url() -> &'static str {
    static URL: OnceLock<String> = OnceLock::new();
    URL.get_or_init(|| {
        std::env::var("CONNECTION_SERVICE_URL")
            .unwrap_or_else(|_| "http://127.0.0.1:7002/api/connections".to_string())
    })
    .as_str()
}

/// Base URL of the internal object-model API.
pub fn object_model_base_url() -> &'static str {
    static URL: OnceLock<String> = OnceLock::new();
    URL.get_or_init(|| {
        std::env::var("RUNTARA_OBJECT_MODEL_URL")
            .unwrap_or_else(|_| "http://127.0.0.1:7002/api/internal/object-model".to_string())
    })
    .as_str()
}
