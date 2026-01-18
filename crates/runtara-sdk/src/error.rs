// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! SDK-specific error types.

#![allow(dead_code)] // Structured error types used by consuming code

#[cfg(feature = "quic")]
use runtara_protocol::ClientError;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use thiserror::Error;

// ============================================================================
// Structured Error Types (mirrors runtara-core error types)
// ============================================================================

/// Error category for retry/routing decisions.
///
/// Two categories:
/// - **Transient**: Auto-retry likely to succeed (network, timeout, rate limit)
/// - **Permanent**: Don't auto-retry (validation, not found, auth, business rules)
///
/// To distinguish technical vs business errors within Permanent, use:
/// - `code`: e.g., `VALIDATION_*` for technical, `BUSINESS_*` or domain-specific codes
/// - `severity`: `Error` for technical failures, `Warning` for expected business outcomes
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCategory {
    /// Unknown category - use default retry policy.
    #[default]
    Unknown,
    /// Transient error - retry is likely to succeed (network, timeout, rate limit).
    Transient,
    /// Permanent error - don't retry (validation, not found, authorization, business rules).
    Permanent,
}

impl ErrorCategory {
    /// Returns the string representation of the category.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::Transient => "transient",
            Self::Permanent => "permanent",
        }
    }

    /// Parse a category from a string.
    pub fn from_str(s: &str) -> Self {
        match s {
            "transient" => Self::Transient,
            "permanent" => Self::Permanent,
            // Legacy: map "business" to "permanent" for backwards compatibility
            "business" => Self::Permanent,
            _ => Self::Unknown,
        }
    }
}

/// Error severity for logging/alerting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ErrorSeverity {
    /// Informational - expected errors that don't indicate a problem.
    Info,
    /// Warning - degraded but functional operation.
    Warning,
    /// Error - operation failed (default severity).
    #[default]
    Error,
    /// Critical - system-level failure requiring immediate attention.
    Critical,
}

impl ErrorSeverity {
    /// Returns the string representation of the severity.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Info => "info",
            Self::Warning => "warning",
            Self::Error => "error",
            Self::Critical => "critical",
        }
    }

    /// Parse a severity from a string.
    pub fn from_str(s: &str) -> Self {
        match s {
            "info" => Self::Info,
            "warning" => Self::Warning,
            "critical" => Self::Critical,
            _ => Self::Error,
        }
    }
}

/// Hint for retry behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum RetryHint {
    /// Unknown - use default retry policy.
    #[default]
    Unknown,
    /// Retry immediately - transient glitch likely resolved.
    RetryImmediately,
    /// Retry with exponential backoff - standard retry strategy.
    RetryWithBackoff,
    /// Retry after specific duration in milliseconds (e.g., rate limit).
    #[serde(rename = "retry_after")]
    RetryAfter(u64),
    /// Do not retry - permanent error.
    DoNotRetry,
}

impl RetryHint {
    /// Returns the string representation of the hint.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::RetryImmediately => "retry_immediately",
            Self::RetryWithBackoff => "retry_with_backoff",
            Self::RetryAfter(_) => "retry_after",
            Self::DoNotRetry => "do_not_retry",
        }
    }

    /// Returns the retry delay in milliseconds if this is a RetryAfter hint.
    pub fn retry_after_ms(&self) -> Option<u64> {
        match self {
            Self::RetryAfter(ms) => Some(*ms),
            _ => None,
        }
    }

    /// Returns true if this error should be retried.
    pub fn should_retry(&self) -> bool {
        matches!(
            self,
            Self::RetryImmediately | Self::RetryWithBackoff | Self::RetryAfter(_)
        )
    }
}

/// Structured error information from the server.
///
/// Provides detailed metadata about an error including categorization,
/// severity, and retry hints for intelligent error handling.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorInfo {
    /// Machine-readable error code (e.g., "RATE_LIMITED", "VALIDATION_ERROR").
    pub code: String,
    /// Human-readable error message.
    pub message: String,
    /// Error category for routing/retry decisions.
    #[serde(default)]
    pub category: ErrorCategory,
    /// Error severity for logging/alerting.
    #[serde(default)]
    pub severity: ErrorSeverity,
    /// Hint for retry behavior.
    #[serde(default)]
    pub retry_hint: RetryHint,
    /// Which step produced this error (if applicable).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_step_id: Option<String>,
    /// Additional context as key-value pairs.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub attributes: HashMap<String, String>,
}

impl ErrorInfo {
    /// Create a new error info with minimal fields.
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            category: ErrorCategory::Unknown,
            severity: ErrorSeverity::Error,
            retry_hint: RetryHint::Unknown,
            source_step_id: None,
            attributes: HashMap::new(),
        }
    }

    /// Create a transient error (retry recommended).
    pub fn transient(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            category: ErrorCategory::Transient,
            severity: ErrorSeverity::Error,
            retry_hint: RetryHint::RetryWithBackoff,
            source_step_id: None,
            attributes: HashMap::new(),
        }
    }

    /// Create a permanent error (don't retry).
    ///
    /// Use for: validation errors, not found, authorization failures, business rule violations.
    ///
    /// For business errors (expected outcomes like "credit limit exceeded"), use
    /// `.with_severity(ErrorSeverity::Warning)` to distinguish from technical failures.
    pub fn permanent(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            category: ErrorCategory::Permanent,
            severity: ErrorSeverity::Error,
            retry_hint: RetryHint::DoNotRetry,
            source_step_id: None,
            attributes: HashMap::new(),
        }
    }

    /// Set the source step ID.
    pub fn with_step(mut self, step_id: impl Into<String>) -> Self {
        self.source_step_id = Some(step_id.into());
        self
    }

    /// Add an attribute.
    pub fn with_attr(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.attributes.insert(key.into(), value.into());
        self
    }

    /// Set the error severity.
    ///
    /// Use `ErrorSeverity::Warning` for expected business outcomes vs technical failures.
    pub fn with_severity(mut self, severity: ErrorSeverity) -> Self {
        self.severity = severity;
        self
    }

    /// Returns true if this is a transient error.
    pub fn is_transient(&self) -> bool {
        self.category == ErrorCategory::Transient
    }

    /// Returns true if this is a permanent error.
    pub fn is_permanent(&self) -> bool {
        self.category == ErrorCategory::Permanent
    }

    /// Returns true if this error should be retried.
    pub fn should_retry(&self) -> bool {
        self.retry_hint.should_retry()
    }
}

impl std::fmt::Display for ErrorInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{}] {}", self.code, self.message)
    }
}

impl std::error::Error for ErrorInfo {}

// ============================================================================
// SDK Error Types
// ============================================================================

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

    /// Server returned a structured error with full metadata
    #[error("structured error: {0}")]
    StructuredError(Box<ErrorInfo>),

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
