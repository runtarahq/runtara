// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Connection extractors module
//!
//! This module provides type-safe extraction of HTTP connection configuration
//! from raw connection parameters. Each integration type has its own extractor
//! that validates and transforms connection parameters into `HttpConnectionConfig`.
//!
//! Extractors are registered using the `inventory` crate for automatic discovery.

use serde_json::Value;
use std::collections::HashMap;

mod http_api_key;
mod http_bearer;
mod sftp;

#[cfg(test)]
mod tests;

// Re-export extractors to ensure they're linked and registered
pub use http_api_key::HttpApiKeyExtractor;
pub use http_bearer::HttpBearerExtractor;

// SFTP connection type is registered for schema purposes (doesn't implement HttpConnectionExtractor)
#[allow(unused_imports)]
use sftp::SftpParams;

/// Configuration extracted from a connection for HTTP requests
#[derive(Debug, Clone, Default)]
pub struct HttpConnectionConfig {
    /// HTTP headers to include in requests
    pub headers: HashMap<String, String>,
    /// Query parameters to include in requests
    pub query_parameters: HashMap<String, String>,
    /// URL prefix to prepend to relative URLs
    pub url_prefix: String,
    /// Rate limit configuration (passed through from connection)
    pub rate_limit_config: Option<Value>,
}

/// Trait for extracting HTTP connection configuration from raw parameters
pub trait HttpConnectionExtractor: Send + Sync {
    /// Returns the integration_id this extractor handles
    fn integration_id(&self) -> &'static str;

    /// Extracts HTTP connection configuration from raw parameters
    fn extract(&self, params: &Value) -> Result<HttpConnectionConfig, String>;
}

// Collect all extractors via inventory (skipped on WASM targets)
#[cfg(not(target_family = "wasm"))]
inventory::collect!(&'static dyn HttpConnectionExtractor);

/// Returns all integration_ids that have a registered `HttpConnectionExtractor`.
#[cfg(not(target_family = "wasm"))]
pub fn get_http_extractor_ids() -> Vec<&'static str> {
    inventory::iter::<&'static dyn HttpConnectionExtractor>
        .into_iter()
        .map(|e| e.integration_id())
        .collect()
}

/// Returns all integration_ids (empty on WASM).
#[cfg(target_family = "wasm")]
pub fn get_http_extractor_ids() -> Vec<&'static str> {
    vec![]
}

/// Extract HTTP connection config from a raw connection
///
/// Looks up the appropriate extractor based on `integration_id` and applies it.
#[cfg(not(target_family = "wasm"))]
pub fn extract_http_config(
    integration_id: &str,
    parameters: &Value,
    rate_limit_config: Option<Value>,
) -> Result<HttpConnectionConfig, String> {
    for extractor in inventory::iter::<&'static dyn HttpConnectionExtractor> {
        if extractor.integration_id() == integration_id {
            let mut config = extractor.extract(parameters)?;
            config.rate_limit_config = rate_limit_config;
            return Ok(config);
        }
    }
    Err(format!(
        "No extractor found for integration_id: '{}'. Available extractors: {:?}",
        integration_id,
        inventory::iter::<&'static dyn HttpConnectionExtractor>()
            .map(|e| e.integration_id())
            .collect::<Vec<_>>()
    ))
}

/// Extract HTTP connection config (not available on WASM).
#[cfg(target_family = "wasm")]
pub fn extract_http_config(
    integration_id: &str,
    _parameters: &Value,
    _rate_limit_config: Option<Value>,
) -> Result<HttpConnectionConfig, String> {
    Err(format!(
        "inventory not available in WASM: extract_http_config for '{}'",
        integration_id
    ))
}
