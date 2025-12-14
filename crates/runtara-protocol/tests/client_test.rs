// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Client configuration tests for runtara-protocol.

use runtara_protocol::client::{RuntaraClient, RuntaraClientConfig};

#[test]
fn test_default_config() {
    let config = RuntaraClientConfig::default();

    assert_eq!(config.server_addr, "127.0.0.1:8001".parse().unwrap());
    assert_eq!(config.server_name, "localhost");
    assert!(config.enable_0rtt);
    assert!(!config.dangerous_skip_cert_verification);
    assert_eq!(config.keep_alive_interval_ms, 10_000);
    assert_eq!(config.idle_timeout_ms, 30_000);
    assert_eq!(config.connect_timeout_ms, 10_000);
}

#[tokio::test]
async fn test_client_creation_with_config() {
    let mut config = RuntaraClientConfig::default();
    config.dangerous_skip_cert_verification = true;

    let client = RuntaraClient::new(config);
    assert!(client.is_ok());
}

#[tokio::test]
async fn test_localhost_client() {
    let client = RuntaraClient::localhost();
    assert!(client.is_ok());
}

#[tokio::test]
async fn test_custom_server_address() {
    let mut config = RuntaraClientConfig::default();
    config.server_addr = "192.168.1.100:8000".parse().unwrap();
    config.dangerous_skip_cert_verification = true;

    let client = RuntaraClient::new(config);
    assert!(client.is_ok());
}

#[test]
fn test_config_clone() {
    let config1 = RuntaraClientConfig::default();
    let config2 = config1.clone();

    assert_eq!(config1.server_addr, config2.server_addr);
    assert_eq!(config1.server_name, config2.server_name);
    assert_eq!(config1.enable_0rtt, config2.enable_0rtt);
    assert_eq!(
        config1.dangerous_skip_cert_verification,
        config2.dangerous_skip_cert_verification
    );
}

#[tokio::test]
async fn test_client_is_connected_before_connect() {
    let client = RuntaraClient::localhost().unwrap();

    // Client should not be connected before calling connect()
    assert!(!client.is_connected().await);
}

#[tokio::test]
async fn test_client_close_without_connect() {
    let client = RuntaraClient::localhost().unwrap();

    // Closing without connecting should not panic
    client.close().await;
}

#[tokio::test]
async fn test_config_with_disabled_keepalive() {
    let mut config = RuntaraClientConfig::default();
    config.keep_alive_interval_ms = 0;
    config.dangerous_skip_cert_verification = true;

    let client = RuntaraClient::new(config);
    assert!(client.is_ok());
}

#[tokio::test]
async fn test_config_with_custom_timeouts() {
    let mut config = RuntaraClientConfig::default();
    config.idle_timeout_ms = 60_000;
    config.connect_timeout_ms = 5_000;
    config.dangerous_skip_cert_verification = true;

    let client = RuntaraClient::new(config);
    assert!(client.is_ok());
}

// Test that client can be created with various server names
#[tokio::test]
async fn test_config_server_name_variations() {
    for server_name in &["localhost", "example.com", "192.168.1.1", "test-server"] {
        let mut config = RuntaraClientConfig::default();
        config.server_name = server_name.to_string();
        config.dangerous_skip_cert_verification = true;

        let client = RuntaraClient::new(config);
        assert!(
            client.is_ok(),
            "Failed to create client with server_name: {}",
            server_name
        );
    }
}
