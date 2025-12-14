// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Error types for runtara-management-sdk.

use thiserror::Error;

/// Result type using SdkError.
pub type Result<T> = std::result::Result<T, SdkError>;

/// Errors that can occur when using the management SDK.
#[derive(Debug, Error)]
pub enum SdkError {
    /// Configuration error (missing or invalid values).
    #[error("configuration error: {0}")]
    Config(String),

    /// Connection to runtara-core failed.
    #[error("connection error: {0}")]
    Connection(String),

    /// Request timed out.
    #[error("request timed out after {0}ms")]
    Timeout(u64),

    /// Server returned an error response.
    #[error("server error [{code}]: {message}")]
    Server { code: String, message: String },

    /// Unexpected response from server.
    #[error("unexpected response: {0}")]
    UnexpectedResponse(String),

    /// Instance not found.
    #[error("instance not found: {0}")]
    InstanceNotFound(String),

    /// Image not found.
    #[error("image not found: {0}")]
    ImageNotFound(String),

    /// Invalid input.
    #[error("invalid input: {0}")]
    InvalidInput(String),

    /// Serialization/deserialization error.
    #[error("serialization error: {0}")]
    Serialization(String),

    /// Protocol error.
    #[error("protocol error: {0}")]
    Protocol(String),
}

impl From<runtara_protocol::client::ClientError> for SdkError {
    fn from(err: runtara_protocol::client::ClientError) -> Self {
        SdkError::Connection(err.to_string())
    }
}

impl From<serde_json::Error> for SdkError {
    fn from(err: serde_json::Error) -> Self {
        SdkError::Serialization(err.to_string())
    }
}

impl From<prost::DecodeError> for SdkError {
    fn from(err: prost::DecodeError) -> Self {
        SdkError::Protocol(err.to_string())
    }
}

impl From<runtara_protocol::frame::FrameError> for SdkError {
    fn from(err: runtara_protocol::frame::FrameError) -> Self {
        SdkError::Protocol(err.to_string())
    }
}

impl From<std::io::Error> for SdkError {
    fn from(err: std::io::Error) -> Self {
        SdkError::Connection(err.to_string())
    }
}

impl From<quinn::WriteError> for SdkError {
    fn from(err: quinn::WriteError) -> Self {
        SdkError::Connection(err.to_string())
    }
}

impl From<quinn::ClosedStream> for SdkError {
    fn from(err: quinn::ClosedStream) -> Self {
        SdkError::Connection(err.to_string())
    }
}
