// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! SDK configuration for connecting to runtara-core.

use std::env;
use std::net::SocketAddr;

use crate::error::{Result, SdkError};

/// SDK configuration for connecting to runtara-core.
#[derive(Debug, Clone)]
pub struct SdkConfig {
    /// Instance ID (required) - unique identifier for this instance
    pub instance_id: String,
    /// Tenant ID (required) - tenant this instance belongs to
    pub tenant_id: String,
    /// Server address (default: "127.0.0.1:8001")
    pub server_addr: SocketAddr,
    /// Server name for TLS verification (default: "localhost")
    pub server_name: String,
    /// Skip TLS certificate verification (default: false, use true for dev)
    pub skip_cert_verification: bool,
    /// Connection timeout in milliseconds (default: 10_000)
    pub connect_timeout_ms: u64,
    /// Request timeout in milliseconds (default: 30_000)
    pub request_timeout_ms: u64,
    /// Signal poll interval in milliseconds (default: 1_000)
    pub signal_poll_interval_ms: u64,
    /// Background heartbeat interval in milliseconds (default: 30_000).
    /// Set to 0 to disable automatic heartbeats.
    /// Heartbeats run in a background task and keep the instance alive
    /// during long-running operations that don't checkpoint frequently.
    pub heartbeat_interval_ms: u64,
}

impl SdkConfig {
    /// Load configuration from environment variables.
    ///
    /// # Required Environment Variables
    /// - `RUNTARA_INSTANCE_ID` - Unique identifier for this instance
    /// - `RUNTARA_TENANT_ID` - Tenant this instance belongs to
    ///
    /// # Optional Environment Variables
    /// - `RUNTARA_SERVER_ADDR` - Server address (default: "127.0.0.1:8001")
    /// - `RUNTARA_SERVER_NAME` - Server name for TLS (default: "localhost")
    /// - `RUNTARA_SKIP_CERT_VERIFICATION` - Skip TLS verification (default: false)
    /// - `RUNTARA_CONNECT_TIMEOUT_MS` - Connection timeout (default: 10000)
    /// - `RUNTARA_REQUEST_TIMEOUT_MS` - Request timeout (default: 30000)
    /// - `RUNTARA_SIGNAL_POLL_INTERVAL_MS` - Signal poll interval (default: 1000)
    /// - `RUNTARA_HEARTBEAT_INTERVAL_MS` - Background heartbeat interval (default: 30000, 0 to disable)
    pub fn from_env() -> Result<Self> {
        let instance_id = env::var("RUNTARA_INSTANCE_ID")
            .map_err(|_| SdkError::Config("RUNTARA_INSTANCE_ID is required".to_string()))?;

        let tenant_id = env::var("RUNTARA_TENANT_ID")
            .map_err(|_| SdkError::Config("RUNTARA_TENANT_ID is required".to_string()))?;

        let server_addr = env::var("RUNTARA_SERVER_ADDR")
            .unwrap_or_else(|_| "127.0.0.1:8001".to_string())
            .parse()
            .map_err(|e| SdkError::Config(format!("invalid RUNTARA_SERVER_ADDR: {}", e)))?;

        let server_name =
            env::var("RUNTARA_SERVER_NAME").unwrap_or_else(|_| "localhost".to_string());

        let skip_cert_verification = env::var("RUNTARA_SKIP_CERT_VERIFICATION")
            .map(|v| v == "true" || v == "1")
            .unwrap_or(false);

        let connect_timeout_ms = env::var("RUNTARA_CONNECT_TIMEOUT_MS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(10_000);

        let request_timeout_ms = env::var("RUNTARA_REQUEST_TIMEOUT_MS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(30_000);

        let signal_poll_interval_ms = env::var("RUNTARA_SIGNAL_POLL_INTERVAL_MS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(1_000);

        let heartbeat_interval_ms = env::var("RUNTARA_HEARTBEAT_INTERVAL_MS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(30_000);

        Ok(Self {
            instance_id,
            tenant_id,
            server_addr,
            server_name,
            skip_cert_verification,
            connect_timeout_ms,
            request_timeout_ms,
            signal_poll_interval_ms,
            heartbeat_interval_ms,
        })
    }

    /// Create a configuration for local development.
    ///
    /// This sets up reasonable defaults for local development:
    /// - Connects to `127.0.0.1:8001`
    /// - Skips TLS certificate verification
    pub fn localhost(instance_id: impl Into<String>, tenant_id: impl Into<String>) -> Self {
        Self {
            instance_id: instance_id.into(),
            tenant_id: tenant_id.into(),
            server_addr: "127.0.0.1:8001".parse().unwrap(),
            server_name: "localhost".to_string(),
            skip_cert_verification: true,
            connect_timeout_ms: 10_000,
            request_timeout_ms: 30_000,
            signal_poll_interval_ms: 1_000,
            heartbeat_interval_ms: 30_000,
        }
    }

    /// Create a new configuration with the given instance and tenant IDs.
    pub fn new(instance_id: impl Into<String>, tenant_id: impl Into<String>) -> Self {
        Self {
            instance_id: instance_id.into(),
            tenant_id: tenant_id.into(),
            server_addr: "127.0.0.1:8001".parse().unwrap(),
            server_name: "localhost".to_string(),
            skip_cert_verification: false,
            connect_timeout_ms: 10_000,
            request_timeout_ms: 30_000,
            signal_poll_interval_ms: 1_000,
            heartbeat_interval_ms: 30_000,
        }
    }

    /// Set the server address.
    pub fn with_server_addr(mut self, addr: SocketAddr) -> Self {
        self.server_addr = addr;
        self
    }

    /// Set the server name for TLS verification.
    pub fn with_server_name(mut self, name: impl Into<String>) -> Self {
        self.server_name = name.into();
        self
    }

    /// Skip TLS certificate verification (for development only!).
    pub fn with_skip_cert_verification(mut self, skip: bool) -> Self {
        self.skip_cert_verification = skip;
        self
    }

    /// Set the signal poll interval.
    pub fn with_signal_poll_interval_ms(mut self, interval_ms: u64) -> Self {
        self.signal_poll_interval_ms = interval_ms;
        self
    }

    /// Set the background heartbeat interval.
    /// Set to 0 to disable automatic heartbeats.
    pub fn with_heartbeat_interval_ms(mut self, interval_ms: u64) -> Self {
        self.heartbeat_interval_ms = interval_ms;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_localhost_config() {
        let config = SdkConfig::localhost("test-instance", "test-tenant");
        assert_eq!(config.instance_id, "test-instance");
        assert_eq!(config.tenant_id, "test-tenant");
        assert!(config.skip_cert_verification);
        assert_eq!(config.server_addr, "127.0.0.1:8001".parse().unwrap());
    }

    #[test]
    fn test_builder_pattern() {
        let config = SdkConfig::new("inst", "tenant")
            .with_server_addr("192.168.1.1:8000".parse().unwrap())
            .with_skip_cert_verification(true)
            .with_signal_poll_interval_ms(500);

        assert_eq!(config.server_addr, "192.168.1.1:8000".parse().unwrap());
        assert!(config.skip_cert_verification);
        assert_eq!(config.signal_poll_interval_ms, 500);
    }

    #[test]
    fn test_heartbeat_interval_default() {
        let config = SdkConfig::new("inst", "tenant");
        assert_eq!(config.heartbeat_interval_ms, 30_000);
    }

    #[test]
    fn test_heartbeat_interval_localhost_default() {
        let config = SdkConfig::localhost("inst", "tenant");
        assert_eq!(config.heartbeat_interval_ms, 30_000);
    }

    #[test]
    fn test_heartbeat_interval_builder() {
        let config = SdkConfig::new("inst", "tenant").with_heartbeat_interval_ms(15_000);
        assert_eq!(config.heartbeat_interval_ms, 15_000);
    }

    #[test]
    fn test_heartbeat_interval_disabled() {
        let config = SdkConfig::new("inst", "tenant").with_heartbeat_interval_ms(0);
        assert_eq!(config.heartbeat_interval_ms, 0);
    }

    #[test]
    fn test_heartbeat_interval_custom_value() {
        let config = SdkConfig::new("inst", "tenant").with_heartbeat_interval_ms(60_000);
        assert_eq!(config.heartbeat_interval_ms, 60_000);
    }
}
