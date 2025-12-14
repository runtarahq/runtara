// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! HTTP API Key connection extractor

use super::{HttpConnectionConfig, HttpConnectionExtractor};
use runtara_agent_macro::ConnectionParams;
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;

/// Parameters for HTTP API Key authentication
#[derive(Debug, Deserialize, ConnectionParams)]
#[connection(
    integration_id = "http_api_key",
    display_name = "HTTP API Key",
    description = "Authenticate HTTP requests using an API key header",
    category = "http"
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
    /// Optional base URL prefix
    #[serde(default)]
    #[field(
        display_name = "Base URL",
        description = "Optional base URL prefix for all requests",
        placeholder = "https://api.example.com"
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

        let mut headers = HashMap::new();
        headers.insert(p.header_name, p.api_key);
        headers.insert("Content-Type".to_string(), "application/json".to_string());

        Ok(HttpConnectionConfig {
            headers,
            query_parameters: HashMap::new(),
            url_prefix: p.base_url.unwrap_or_default(),
            rate_limit_config: None,
        })
    }
}

inventory::submit! {
    &HttpApiKeyExtractor as &'static dyn HttpConnectionExtractor
}
