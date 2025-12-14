// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Configuration for the management SDK.

use std::net::SocketAddr;
use std::time::Duration;

use crate::error::{Result, SdkError};

/// Configuration for the ManagementSdk.
#[derive(Debug, Clone)]
pub struct SdkConfig {
    /// Server address to connect to.
    pub server_addr: SocketAddr,
    /// Server name for TLS verification.
    pub server_name: String,
    /// Skip TLS certificate verification (development only).
    pub skip_cert_verification: bool,
    /// Connection timeout.
    pub connect_timeout: Duration,
    /// Request timeout.
    pub request_timeout: Duration,
}

impl Default for SdkConfig {
    fn default() -> Self {
        Self {
            server_addr: "127.0.0.1:8002".parse().unwrap(), // Environment server default port
            server_name: "localhost".to_string(),
            skip_cert_verification: false,
            connect_timeout: Duration::from_secs(10),
            request_timeout: Duration::from_secs(30),
        }
    }
}

impl SdkConfig {
    /// Create a new configuration with default values.
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a configuration for localhost development.
    ///
    /// This enables certificate verification skipping.
    pub fn localhost() -> Self {
        Self {
            skip_cert_verification: true,
            ..Self::default()
        }
    }

    /// Create a configuration from environment variables.
    ///
    /// Environment variables:
    /// - `RUNTARA_ENVIRONMENT_ADDR`: Server address (default: "127.0.0.1:8002")
    /// - `RUNTARA_SERVER_NAME`: Server name for TLS (default: "localhost")
    /// - `RUNTARA_SKIP_CERT_VERIFICATION`: Skip TLS verification (default: "false")
    /// - `RUNTARA_CONNECT_TIMEOUT_MS`: Connection timeout in milliseconds (default: 10000)
    /// - `RUNTARA_REQUEST_TIMEOUT_MS`: Request timeout in milliseconds (default: 30000)
    pub fn from_env() -> Result<Self> {
        let server_addr = std::env::var("RUNTARA_ENVIRONMENT_ADDR")
            .unwrap_or_else(|_| "127.0.0.1:8002".to_string())
            .parse()
            .map_err(|e| SdkError::Config(format!("invalid RUNTARA_ENVIRONMENT_ADDR: {}", e)))?;

        let server_name =
            std::env::var("RUNTARA_SERVER_NAME").unwrap_or_else(|_| "localhost".to_string());

        let skip_cert_verification = std::env::var("RUNTARA_SKIP_CERT_VERIFICATION")
            .map(|v| v.to_lowercase() == "true" || v == "1")
            .unwrap_or(false);

        let connect_timeout_ms: u64 = std::env::var("RUNTARA_CONNECT_TIMEOUT_MS")
            .unwrap_or_else(|_| "10000".to_string())
            .parse()
            .map_err(|e| SdkError::Config(format!("invalid RUNTARA_CONNECT_TIMEOUT_MS: {}", e)))?;

        let request_timeout_ms: u64 = std::env::var("RUNTARA_REQUEST_TIMEOUT_MS")
            .unwrap_or_else(|_| "30000".to_string())
            .parse()
            .map_err(|e| SdkError::Config(format!("invalid RUNTARA_REQUEST_TIMEOUT_MS: {}", e)))?;

        Ok(Self {
            server_addr,
            server_name,
            skip_cert_verification,
            connect_timeout: Duration::from_millis(connect_timeout_ms),
            request_timeout: Duration::from_millis(request_timeout_ms),
        })
    }

    /// Set the server address.
    pub fn with_server_addr(mut self, addr: SocketAddr) -> Self {
        self.server_addr = addr;
        self
    }

    /// Set the server name for TLS.
    pub fn with_server_name(mut self, name: impl Into<String>) -> Self {
        self.server_name = name.into();
        self
    }

    /// Enable or disable certificate verification skipping.
    pub fn with_skip_cert_verification(mut self, skip: bool) -> Self {
        self.skip_cert_verification = skip;
        self
    }

    /// Set the connection timeout.
    pub fn with_connect_timeout(mut self, timeout: Duration) -> Self {
        self.connect_timeout = timeout;
        self
    }

    /// Set the request timeout.
    pub fn with_request_timeout(mut self, timeout: Duration) -> Self {
        self.request_timeout = timeout;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = SdkConfig::default();
        assert_eq!(config.server_addr, "127.0.0.1:8002".parse().unwrap());
        assert_eq!(config.server_name, "localhost");
        assert!(!config.skip_cert_verification);
    }

    #[test]
    fn test_localhost_config() {
        let config = SdkConfig::localhost();
        assert!(config.skip_cert_verification);
    }

    #[test]
    fn test_builder_methods() {
        let config = SdkConfig::new()
            .with_server_addr("192.168.1.100:8000".parse().unwrap())
            .with_server_name("myserver")
            .with_skip_cert_verification(true)
            .with_connect_timeout(Duration::from_secs(5))
            .with_request_timeout(Duration::from_secs(60));

        assert_eq!(config.server_addr, "192.168.1.100:8000".parse().unwrap());
        assert_eq!(config.server_name, "myserver");
        assert!(config.skip_cert_verification);
        assert_eq!(config.connect_timeout, Duration::from_secs(5));
        assert_eq!(config.request_timeout, Duration::from_secs(60));
    }
}
