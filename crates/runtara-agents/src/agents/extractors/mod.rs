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

pub mod connection_types;
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
    &connection_types::ShopifyExtractor,
    &connection_types::ShopifyClientCredentialsExtractor,
    &connection_types::OpenAiExtractor,
    &connection_types::MicrosoftEntraClientCredentialsExtractor,
    &connection_types::HttpOAuth2ClientCredentialsExtractor,
    &connection_types::HttpOAuth2AuthorizationCodeExtractor,
    &connection_types::MailgunExtractor,
    &connection_types::HubSpotExtractor,
    &connection_types::HubSpotAccessTokenExtractor,
    &connection_types::StripeExtractor,
    &connection_types::McpExtractor,
];

/// Returns all integration_ids that have a registered `HttpConnectionExtractor`.
pub fn get_http_extractor_ids() -> Vec<&'static str> {
    HTTP_EXTRACTORS
        .iter()
        .map(|extractor| extractor.integration_id())
        .collect()
}

/// The one agent whose integration list is defined by the host extractor
/// registry above rather than by its own component metadata: the generic HTTP
/// client accepts any integration that has a registered
/// [`HttpConnectionExtractor`], and a wasm component cannot see that registry.
/// Its `meta.json` therefore declares an empty list — see the module docs on
/// `runtara-agent-http`.
const HOST_DYNAMIC_HTTP_AGENT: &str = "http";

/// Host-resolved integration ids for `agent_id`.
///
/// Every agent with a static list declared in its component metadata gets
/// `declared` back unchanged; only the host-dynamic agent is resolved against
/// the extractor registry. This is the single place the platform decides what
/// an agent's integration ids really are — callers must not re-derive it.
pub fn resolve_integration_ids(agent_id: &str, declared: &[String]) -> Vec<String> {
    if runtara_dsl::agent_meta::canonical_agent_id(agent_id)
        == runtara_dsl::agent_meta::canonical_agent_id(HOST_DYNAMIC_HTTP_AGENT)
    {
        return get_http_extractor_ids()
            .into_iter()
            .map(String::from)
            .collect();
    }
    declared.to_vec()
}

/// Rebuild `catalog` with host-resolved integration ids.
///
/// Call once at boot, on the raw catalog the component dispatcher parses from
/// the `meta.json` sidecars, and hand the result to every consumer: the
/// connection-picker endpoint, `IntegrationCompatibility`, graph validation and
/// the agents API all read `integration_ids` and must see the same answer.
pub fn augment_catalog(
    catalog: &runtara_dsl::agent_meta::AgentCatalog,
) -> runtara_dsl::agent_meta::AgentCatalog {
    let agents = catalog
        .agents()
        .iter()
        .map(|agent| {
            let mut agent = agent.clone();
            agent.integration_ids = resolve_integration_ids(&agent.id, &agent.integration_ids);
            agent
        })
        .collect();
    runtara_dsl::agent_meta::AgentCatalog::from_agents(agents)
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
