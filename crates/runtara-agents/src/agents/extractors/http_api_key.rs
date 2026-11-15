// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! HTTP API Key connection extractor

use super::{HttpConnectionConfig, HttpConnectionExtractor};
use runtara_agent_macro::ConnectionParams;
use runtara_dsl::derived_connection::{DEFAULT_VALUE_FORMAT, check_header_value_format};
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;

/// Parameters for HTTP API Key authentication
#[derive(Debug, Deserialize, ConnectionParams)]
#[connection(
    integration_id = "http_api_key",
    display_name = "HTTP API Key",
    description = "Authenticate HTTP requests using an API key header",
    category = "http",
    auth_type = "api_key"
)]
struct HttpApiKeyParams {
    /// API key value
    #[field(display_name = "API Key", description = "API key value", secret)]
    api_key: String,
    /// Header name for the API key (defaults to "X-API-Key")
    #[serde(default = "default_header_name")]
    #[field(
        display_name = "Header Name",
        description = "Header name for the API key",
        default = "X-API-Key"
    )]
    header_name: String,
    /// What the header holds: the key alone (`{}`, the default) or with
    /// text around it (`Token {}`).
    #[serde(default)]
    #[field(
        display_name = "Value Format",
        description = "What the header holds, with {} where the key goes. Leave empty to send the key alone.",
        placeholder = "Token {}"
    )]
    value_format: Option<String>,
    /// Base URL prefix. Required + https-validated: the proxy pins every
    /// credentialed request to this host so the API key cannot be sent to an
    /// attacker-chosen destination.
    #[serde(default)]
    #[field(
        display_name = "Base URL",
        description = "Base URL prefix for all requests (must be https)",
        placeholder = "https://api.example.com",
        is_url,
        is_required
    )]
    base_url: Option<String>,
}

fn default_header_name() -> String {
    "X-API-Key".to_string()
}

/// Extractor for HTTP API Key connections
pub struct HttpApiKeyExtractor;

impl HttpConnectionExtractor for HttpApiKeyExtractor {
    fn integration_id(&self) -> &'static str {
        "http_api_key"
    }

    fn extract(&self, params: &Value) -> Result<HttpConnectionConfig, String> {
        let p: HttpApiKeyParams = serde_json::from_value(params.clone())
            .map_err(|e| format!("Invalid http_api_key connection parameters: {}", e))?;

        let format = match p.value_format.as_deref().filter(|f| !f.trim().is_empty()) {
            Some(format) => {
                check_header_value_format(format).map_err(|e| {
                    format!("Invalid http_api_key connection parameters: value_format {e}")
                })?;
                format
            }
            None => DEFAULT_VALUE_FORMAT,
        };
        let mut headers = HashMap::new();
        headers.insert(p.header_name, format.replacen("{}", &p.api_key, 1));
        headers.insert("Content-Type".to_string(), "application/json".to_string());

        Ok(HttpConnectionConfig {
            headers,
            query_parameters: HashMap::new(),
            url_prefix: p.base_url.unwrap_or_default(),
            rate_limit_config: None,
        })
    }
}
