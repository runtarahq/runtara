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
