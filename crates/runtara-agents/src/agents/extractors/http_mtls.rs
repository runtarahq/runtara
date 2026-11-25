// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! HTTP mutual-TLS connection registration.
//!
//! The extractor makes this type available to the generic HTTP agent. The
//! server-side connection resolver consumes the PEM fields and configures the
//! TLS client; the component only ever receives the opaque connection id.

use super::{HttpConnectionConfig, HttpConnectionExtractor};
use runtara_agent_macro::ConnectionParams;
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;

/// Parameters for an HTTP connection authenticated with mutual TLS.
#[derive(Deserialize, ConnectionParams)]
#[connection(
    integration_id = "http_mtls",
    display_name = "HTTP mTLS",
    description = "Authenticate HTTPS requests with a client certificate and private key",
    category = "http",
    auth_type = "custom"
)]
#[allow(dead_code)] // Macro-derived descriptor owns this schema; extraction reads only Base URL.
struct HttpMtlsParams {
    /// Base URL used to pin every credentialed request to the configured API.
    #[field(
        display_name = "Base URL",
        description = "HTTPS base URL for all requests",
        placeholder = "https://api.example.com",
        is_url,
        is_required
    )]
    base_url: String,

    /// Leaf-first client certificate chain, held write-only by the service.
    #[field(
        display_name = "Client Certificate",
        description = "PEM client certificate chain, with the leaf certificate first",
        secret,
        control = "secret_textarea"
    )]
    client_certificate_pem: String,

    /// An unencrypted client private key compatible with Rustls.
    #[field(
        display_name = "Client Private Key",
        description = "Unencrypted PEM private key (PKCS#8, RSA PKCS#1, or SEC1 EC)",
        secret,
        control = "secret_textarea"
    )]
    client_private_key_pem: String,

    /// Extra trusted CA certificates, when the upstream uses private PKI.
    #[serde(default)]
    #[field(
        display_name = "Server CA Certificate",
        description = "Optional PEM CA certificate bundle for a private server trust chain",
        secret,
        clearable,
        control = "secret_textarea"
    )]
    server_ca_pem: Option<String>,
}

/// Extractor for mTLS-authenticated HTTP connections.
pub struct HttpMtlsExtractor;

impl HttpConnectionExtractor for HttpMtlsExtractor {
    fn integration_id(&self) -> &'static str {
        "http_mtls"
    }

    fn extract(&self, params: &Value) -> Result<HttpConnectionConfig, String> {
        // Do not deserialize/clone write-only PEM material into the extractor:
        // it is deliberately consumed only by server-side auth resolution.
        let base_url = params
            .get("base_url")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| "Invalid http_mtls connection parameters: missing Base URL".to_string())?
            .to_string();

        Ok(HttpConnectionConfig {
            headers: HashMap::new(),
            query_parameters: HashMap::new(),
            url_prefix: base_url,
            rate_limit_config: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtara_dsl::form::FieldAccessMode;
    use serde_json::json;

    #[test]
    fn extractor_exposes_only_the_pinned_url_and_no_tls_material() {
        let config = HttpMtlsExtractor
            .extract(&json!({
                "base_url": "https://api.example.com/v1",
                "client_certificate_pem": "certificate",
                "client_private_key_pem": "private-key",
                "server_ca_pem": "ca",
            }))
            .expect("base URL is enough for host-side extraction");

        assert_eq!(config.url_prefix, "https://api.example.com/v1");
        assert!(config.headers.is_empty());
        assert!(config.query_parameters.is_empty());
    }

    #[test]
    fn descriptor_keeps_pem_fields_write_only() {
        let meta = &__CONNECTION_META_HttpMtlsParams;
        let base_url = meta
            .fields
            .iter()
            .find(|field| field.name == "base_url")
            .expect("base URL field");
        assert!(base_url.is_required);
        assert!(base_url.is_url);

        for field_name in ["client_certificate_pem", "client_private_key_pem"] {
            let field = meta
                .fields
                .iter()
                .find(|field| field.name == field_name)
                .expect("required PEM field");
            assert!(field.is_secret);
            assert_eq!(field.access, FieldAccessMode::Write);
            assert!(!field.behavior.clearable);
        }

        let server_ca = meta
            .fields
            .iter()
            .find(|field| field.name == "server_ca_pem")
            .expect("optional CA field");
        assert!(server_ca.is_secret);
        assert_eq!(server_ca.access, FieldAccessMode::Write);
        assert!(server_ca.behavior.clearable);
    }
}
