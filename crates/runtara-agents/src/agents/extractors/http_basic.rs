// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! HTTP Basic authentication connection extractor

use super::{HttpConnectionConfig, HttpConnectionExtractor};
use runtara_agent_macro::ConnectionParams;
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;

/// Parameters for HTTP Basic authentication (RFC 7617)
#[derive(Debug, Deserialize, ConnectionParams)]
#[connection(
    integration_id = "http_basic",
    display_name = "HTTP Basic Auth",
    description = "Authenticate HTTP requests with a username and password (HTTP Basic)",
    category = "http",
    auth_type = "username_password"
)]
struct HttpBasicParams {
    /// Username (may not contain a colon)
    #[field(display_name = "Username", description = "Username")]
    username: String,

    /// Password
    #[field(display_name = "Password", description = "Password", secret)]
    password: String,

    /// Base URL prefix. Required + https-validated: the proxy pins every
    /// credentialed request to this host so the credentials cannot be sent to
    /// an attacker-chosen destination.
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

/// The `Authorization` value for HTTP Basic: `Basic base64(username:password)`.
/// A username with a colon cannot be encoded unambiguously (RFC 7617 §2).
pub fn http_basic_auth(username: &str, password: &str) -> Result<String, String> {
    if username.contains(':') {
        return Err("HTTP Basic username may not contain ':'".to_string());
    }
    Ok(format!(
        "Basic {}",
        base64::Engine::encode(
            &base64::engine::general_purpose::STANDARD,
            format!("{username}:{password}")
        )
    ))
}

/// HTTP extractor for HTTP Basic connections
pub struct HttpBasicExtractor;

impl HttpConnectionExtractor for HttpBasicExtractor {
    fn integration_id(&self) -> &'static str {
        "http_basic"
    }

    fn extract(&self, params: &Value) -> Result<HttpConnectionConfig, String> {
        let p: HttpBasicParams = serde_json::from_value(params.clone())
            .map_err(|e| format!("Invalid http_basic connection parameters: {}", e))?;

        let mut headers = HashMap::new();
        headers.insert(
            "Authorization".to_string(),
            http_basic_auth(&p.username, &p.password)?,
        );
        headers.insert("Content-Type".to_string(), "application/json".to_string());

        Ok(HttpConnectionConfig {
            headers,
            query_parameters: HashMap::new(),
            url_prefix: p.base_url.unwrap_or_default(),
            rate_limit_config: None,
        })
    }
}
