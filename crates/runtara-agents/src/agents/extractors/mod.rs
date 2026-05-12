// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Connection extractors module
//!
//! This module provides type-safe extraction of HTTP connection configuration
//! from raw connection parameters. Each integration type has its own extractor
//! that validates and transforms connection parameters into `HttpConnectionConfig`.
//!
//! Extractors are registered in a small static list for deterministic discovery.

use serde_json::Value;
use std::collections::HashMap;

pub(crate) mod http_api_key;
pub(crate) mod http_bearer;
pub(crate) mod sftp;

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

static HTTP_EXTRACTORS: &[&dyn HttpConnectionExtractor] = &[
    &HttpBearerExtractor,
    &HttpApiKeyExtractor,
    #[cfg(feature = "integrations")]
    &crate::integrations::connection_types::ShopifyExtractor,
    #[cfg(feature = "integrations")]
    &crate::integrations::connection_types::ShopifyClientCredentialsExtractor,
    #[cfg(feature = "integrations")]
    &crate::integrations::connection_types::OpenAiExtractor,
    #[cfg(feature = "integrations")]
    &crate::integrations::connection_types::MicrosoftEntraClientCredentialsExtractor,
    #[cfg(feature = "integrations")]
    &crate::integrations::connection_types::MailgunExtractor,
    #[cfg(feature = "integrations")]
    &crate::integrations::connection_types::HubSpotExtractor,
    #[cfg(feature = "integrations")]
    &crate::integrations::connection_types::HubSpotAccessTokenExtractor,
    #[cfg(feature = "integrations")]
    &crate::integrations::connection_types::StripeExtractor,
];

/// Returns all integration_ids that have a registered `HttpConnectionExtractor`.
pub fn get_http_extractor_ids() -> Vec<&'static str> {
    HTTP_EXTRACTORS
        .iter()
        .map(|extractor| extractor.integration_id())
        .collect()
}

/// Extract HTTP connection config from a raw connection
///
/// Looks up the appropriate extractor based on `integration_id` and applies it.
pub fn extract_http_config(
    integration_id: &str,
    parameters: &Value,
    rate_limit_config: Option<Value>,
) -> Result<HttpConnectionConfig, String> {
    for extractor in HTTP_EXTRACTORS {
        if extractor.integration_id() == integration_id {
            let mut config = extractor.extract(parameters)?;
            config.rate_limit_config = rate_limit_config;
            return Ok(config);
        }
    }
    Err(format!(
        "No extractor found for integration_id: '{}'. Available extractors: {:?}",
        integration_id,
        get_http_extractor_ids()
    ))
}
