// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Configuration tests for runtara-sdk.
//!
//! These tests are specific to the QUIC backend.

#![cfg(feature = "quic")]

use runtara_sdk::SdkConfig;

#[test]
fn test_new_config() {
    let config = SdkConfig::new("test-instance", "test-tenant");

    assert_eq!(config.instance_id, "test-instance");
    assert_eq!(config.tenant_id, "test-tenant");
    assert_eq!(config.server_addr, "127.0.0.1:8001".parse().unwrap());
    assert_eq!(config.server_name, "localhost");
    assert!(!config.skip_cert_verification);
    assert_eq!(config.connect_timeout_ms, 10_000);
    assert_eq!(config.request_timeout_ms, 30_000);
    assert_eq!(config.signal_poll_interval_ms, 1_000);
}

#[test]
fn test_localhost_config() {
    let config = SdkConfig::localhost("test-instance", "test-tenant");

    assert_eq!(config.instance_id, "test-instance");
    assert_eq!(config.tenant_id, "test-tenant");
    assert_eq!(config.server_addr, "127.0.0.1:8001".parse().unwrap());
    assert!(config.skip_cert_verification);
}

#[test]
fn test_with_server_addr() {
    let config =
        SdkConfig::new("inst", "tenant").with_server_addr("192.168.1.100:8000".parse().unwrap());

    assert_eq!(config.server_addr, "192.168.1.100:8000".parse().unwrap());
}

#[test]
fn test_with_server_name() {
    let config = SdkConfig::new("inst", "tenant").with_server_name("myserver.example.com");

    assert_eq!(config.server_name, "myserver.example.com");
}

#[test]
fn test_with_skip_cert_verification() {
    let config = SdkConfig::new("inst", "tenant").with_skip_cert_verification(true);

    assert!(config.skip_cert_verification);
}

#[test]
fn test_with_signal_poll_interval() {
    let config = SdkConfig::new("inst", "tenant").with_signal_poll_interval_ms(500);

    assert_eq!(config.signal_poll_interval_ms, 500);
}

#[test]
fn test_builder_chain() {
    let config = SdkConfig::new("my-instance", "my-tenant")
        .with_server_addr("10.0.0.1:9000".parse().unwrap())
        .with_server_name("production-server")
        .with_skip_cert_verification(false)
        .with_signal_poll_interval_ms(2000);

    assert_eq!(config.instance_id, "my-instance");
    assert_eq!(config.tenant_id, "my-tenant");
    assert_eq!(config.server_addr, "10.0.0.1:9000".parse().unwrap());
    assert_eq!(config.server_name, "production-server");
    assert!(!config.skip_cert_verification);
    assert_eq!(config.signal_poll_interval_ms, 2000);
}

#[test]
fn test_config_debug() {
    let config = SdkConfig::new("inst", "tenant");
    let debug_str = format!("{:?}", config);

    assert!(debug_str.contains("inst"));
    assert!(debug_str.contains("tenant"));
    assert!(debug_str.contains("127.0.0.1:8001"));
}

#[test]
fn test_config_clone() {
    let original = SdkConfig::new("inst", "tenant").with_skip_cert_verification(true);
    let cloned = original.clone();

    assert_eq!(original.instance_id, cloned.instance_id);
    assert_eq!(original.tenant_id, cloned.tenant_id);
    assert_eq!(original.server_addr, cloned.server_addr);
    assert_eq!(
        original.skip_cert_verification,
        cloned.skip_cert_verification
    );
}

#[test]
fn test_various_server_addresses() {
    let addresses = [
        "127.0.0.1:8001",
        "0.0.0.0:8000",
        "192.168.1.1:9000",
        "[::1]:7001",
    ];

    for addr in addresses {
        let config = SdkConfig::new("inst", "tenant").with_server_addr(addr.parse().unwrap());
        assert_eq!(config.server_addr.to_string(), addr);
    }
}
