// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Error type tests for runtara-management-sdk.

use runtara_management_sdk::SdkError;

#[test]
fn test_config_error_display() {
    let err = SdkError::Config("missing address".to_string());
    assert!(err.to_string().contains("configuration error"));
    assert!(err.to_string().contains("missing address"));
}

#[test]
fn test_connection_error_display() {
    let err = SdkError::Connection("connection refused".to_string());
    assert!(err.to_string().contains("connection error"));
    assert!(err.to_string().contains("connection refused"));
}

#[test]
fn test_timeout_error_display() {
    let err = SdkError::Timeout(5000);
    assert!(err.to_string().contains("timed out"));
    assert!(err.to_string().contains("5000"));
}

#[test]
fn test_server_error_display() {
    let err = SdkError::Server {
        code: "500".to_string(),
        message: "Internal error".to_string(),
    };
    let display = err.to_string();
    assert!(display.contains("server error"));
    assert!(display.contains("500"));
    assert!(display.contains("Internal error"));
}

#[test]
fn test_unexpected_response_error_display() {
    let err = SdkError::UnexpectedResponse("invalid format".to_string());
    assert!(err.to_string().contains("unexpected response"));
    assert!(err.to_string().contains("invalid format"));
}

#[test]
fn test_instance_not_found_error_display() {
    let err = SdkError::InstanceNotFound("inst-123".to_string());
    assert!(err.to_string().contains("instance not found"));
    assert!(err.to_string().contains("inst-123"));
}

#[test]
fn test_image_not_found_error_display() {
    let err = SdkError::ImageNotFound("img-456".to_string());
    assert!(err.to_string().contains("image not found"));
    assert!(err.to_string().contains("img-456"));
}

#[test]
fn test_invalid_input_error_display() {
    let err = SdkError::InvalidInput("bad json".to_string());
    assert!(err.to_string().contains("invalid input"));
    assert!(err.to_string().contains("bad json"));
}

#[test]
fn test_serialization_error_display() {
    let err = SdkError::Serialization("parse error".to_string());
    assert!(err.to_string().contains("serialization error"));
    assert!(err.to_string().contains("parse error"));
}

#[test]
fn test_protocol_error_display() {
    let err = SdkError::Protocol("frame error".to_string());
    assert!(err.to_string().contains("protocol error"));
    assert!(err.to_string().contains("frame error"));
}

#[test]
fn test_error_is_send_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<SdkError>();
}

#[test]
fn test_error_debug() {
    let err = SdkError::Timeout(1000);
    let debug_str = format!("{:?}", err);
    assert!(debug_str.contains("Timeout"));
    assert!(debug_str.contains("1000"));
}

// Test From implementations
#[test]
fn test_from_serde_json_error() {
    let json_err: Result<(), serde_json::Error> = serde_json::from_str::<()>("invalid");

    let sdk_err: SdkError = json_err.unwrap_err().into();
    assert!(matches!(sdk_err, SdkError::Serialization(_)));
}

#[test]
fn test_from_io_error() {
    let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file not found");
    let sdk_err: SdkError = io_err.into();
    assert!(matches!(sdk_err, SdkError::Connection(_)));
}
