// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! HTTP Bearer token connection extractor

use super::{HttpConnectionConfig, HttpConnectionExtractor};
use runtara_agent_macro::ConnectionParams;
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;

/// Parameters for HTTP Bearer token authentication
#[derive(Debug, Deserialize, ConnectionParams)]
#[connection(
    integration_id = "http_bearer",
    display_name = "HTTP Bearer Token",
    description = "Authenticate HTTP requests using a Bearer token",
    category = "http"
)]
struct HttpBearerParams {
    /// Bearer token for authentication
    #[field(
        display_name = "Token",
        description = "Bearer token for authentication",
        secret
    )]
    token: String,
    /// Optional base URL prefix
    #[serde(default)]
    #[field(
        display_name = "Base URL",
        description = "Optional base URL prefix for all requests",
        placeholder = "https://api.example.com"
    )]
    base_url: Option<String>,
}

/// Extractor for HTTP Bearer token connections
pub struct HttpBearerExtractor;

impl HttpConnectionExtractor for HttpBearerExtractor {
    fn integration_id(&self) -> &'static str {
        "http_bearer"
    }

    fn extract(&self, params: &Value) -> Result<HttpConnectionConfig, String> {
        let p: HttpBearerParams = serde_json::from_value(params.clone())
            .map_err(|e| format!("Invalid http_bearer connection parameters: {}", e))?;

        let mut headers = HashMap::new();
        headers.insert("Authorization".to_string(), format!("Bearer {}", p.token));
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
    &HttpBearerExtractor as &'static dyn HttpConnectionExtractor
}
