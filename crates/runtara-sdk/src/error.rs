// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! SDK-specific error types.

#[cfg(feature = "quic")]
use runtara_protocol::ClientError;
use thiserror::Error;

/// Errors that can occur in the SDK.
#[derive(Debug, Error)]
pub enum SdkError {
    /// Configuration error (missing or invalid environment variable)
    #[error("configuration error: {0}")]
    Config(String),

    /// Connection to runtara-core failed
    #[cfg(feature = "quic")]
    #[error("connection error: {0}")]
    Connection(#[from] ClientError),

    /// Registration with runtara-core failed
    #[error("registration failed: {0}")]
    Registration(String),

    /// Checkpoint operation failed
    #[error("checkpoint error: {0}")]
    Checkpoint(String),

    /// Sleep request failed
    #[error("sleep error: {0}")]
    Sleep(String),

    /// Event sending failed
    #[error("event error: {0}")]
    Event(String),

    /// Signal error
    #[error("signal error: {0}")]
    Signal(String),

    /// Status query failed
    #[error("status error: {0}")]
    Status(String),

    /// Server returned an error response
    #[error("server error: {code} - {message}")]
    Server {
        /// Error code from the server
        code: String,
        /// Error message from the server
        message: String,
    },

    /// Instance was cancelled
    #[error("instance cancelled")]
    Cancelled,

    /// Instance was paused
    #[error("instance paused")]
    Paused,

    /// Serialization/deserialization error
    #[error("serialization error: {0}")]
    Serialization(String),

    /// Unexpected response from server
    #[error("unexpected response: {0}")]
    UnexpectedResponse(String),

    /// Internal SDK error
    #[error("internal error: {0}")]
    Internal(String),
}

impl From<prost::DecodeError> for SdkError {
    fn from(err: prost::DecodeError) -> Self {
        SdkError::Serialization(err.to_string())
    }
}

/// Type alias for SDK results.
pub type Result<T> = std::result::Result<T, SdkError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_error_display() {
        let err = SdkError::Config("missing RUNTARA_INSTANCE_ID".to_string());
        assert_eq!(
            format!("{}", err),
            "configuration error: missing RUNTARA_INSTANCE_ID"
        );
    }

    #[test]
    fn test_registration_error_display() {
        let err = SdkError::Registration("instance already exists".to_string());
        assert_eq!(
            format!("{}", err),
            "registration failed: instance already exists"
        );
    }

    #[test]
    fn test_checkpoint_error_display() {
        let err = SdkError::Checkpoint("failed to save state".to_string());
        assert_eq!(format!("{}", err), "checkpoint error: failed to save state");
    }

    #[test]
    fn test_sleep_error_display() {
        let err = SdkError::Sleep("invalid duration".to_string());
        assert_eq!(format!("{}", err), "sleep error: invalid duration");
    }

    #[test]
    fn test_event_error_display() {
        let err = SdkError::Event("failed to send event".to_string());
        assert_eq!(format!("{}", err), "event error: failed to send event");
    }

    #[test]
    fn test_signal_error_display() {
        let err = SdkError::Signal("signal rejected".to_string());
        assert_eq!(format!("{}", err), "signal error: signal rejected");
    }

    #[test]
    fn test_status_error_display() {
        let err = SdkError::Status("not found".to_string());
        assert_eq!(format!("{}", err), "status error: not found");
    }

    #[test]
    fn test_server_error_display() {
        let err = SdkError::Server {
            code: "ERR_NOT_FOUND".to_string(),
            message: "Instance not found".to_string(),
        };
        assert_eq!(
            format!("{}", err),
            "server error: ERR_NOT_FOUND - Instance not found"
        );
    }

    #[test]
    fn test_cancelled_error_display() {
        let err = SdkError::Cancelled;
        assert_eq!(format!("{}", err), "instance cancelled");
    }

    #[test]
    fn test_paused_error_display() {
        let err = SdkError::Paused;
        assert_eq!(format!("{}", err), "instance paused");
    }

    #[test]
    fn test_serialization_error_display() {
        let err = SdkError::Serialization("invalid JSON".to_string());
        assert_eq!(format!("{}", err), "serialization error: invalid JSON");
    }

    #[test]
    fn test_unexpected_response_error_display() {
        let err = SdkError::UnexpectedResponse("expected Ack".to_string());
        assert_eq!(format!("{}", err), "unexpected response: expected Ack");
    }

    #[test]
    fn test_internal_error_display() {
        let err = SdkError::Internal("mutex poisoned".to_string());
        assert_eq!(format!("{}", err), "internal error: mutex poisoned");
    }

    #[test]
    fn test_error_debug() {
        let err = SdkError::Config("test".to_string());
        let debug_str = format!("{:?}", err);
        assert!(debug_str.contains("Config"));
        assert!(debug_str.contains("test"));
    }

    #[test]
    fn test_server_error_debug() {
        let err = SdkError::Server {
            code: "ERR_500".to_string(),
            message: "Internal error".to_string(),
        };
        let debug_str = format!("{:?}", err);
        assert!(debug_str.contains("Server"));
        assert!(debug_str.contains("ERR_500"));
        assert!(debug_str.contains("Internal error"));
    }

    #[test]
    fn test_from_prost_decode_error() {
        // Create a decode error by trying to decode invalid protobuf
        let invalid_bytes = vec![0xFF, 0xFF, 0xFF];
        let decode_result: std::result::Result<runtara_protocol::instance_proto::InstanceEvent, _> =
            prost::Message::decode(invalid_bytes.as_slice());

        if let Err(decode_error) = decode_result {
            let sdk_error: SdkError = decode_error.into();
            let msg = format!("{}", sdk_error);
            assert!(msg.starts_with("serialization error:"));
        }
    }

    #[test]
    fn test_result_type_alias() {
        fn returns_ok() -> Result<i32> {
            Ok(42)
        }

        fn returns_err() -> Result<i32> {
            Err(SdkError::Internal("test".to_string()))
        }

        assert!(returns_ok().is_ok());
        assert!(returns_err().is_err());
    }
}
