// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Error types for runtara-core.
//!
//! Provides a unified error type that maps to RPC error responses,
//! including structured error types with category, severity, and retry hints.

#![allow(dead_code)] // Variants and methods used in tests and for future expansion

#[cfg(feature = "server")]
use runtara_protocol::instance_proto::{self as proto, RpcError};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;

// ============================================================================
// Structured Error Types
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
    pub fn parse(s: &str) -> Self {
        match s {
            "transient" => Self::Transient,
            "permanent" => Self::Permanent,
            // Legacy: map "business" to "permanent" for backwards compatibility
            "business" => Self::Permanent,
            _ => Self::Unknown,
        }
    }

    /// Convert to protocol buffer enum value.
    #[cfg(feature = "server")]
    pub fn to_proto(&self) -> i32 {
        match self {
            Self::Unknown => proto::ErrorCategory::Unknown as i32,
            Self::Transient => proto::ErrorCategory::Transient as i32,
            Self::Permanent => proto::ErrorCategory::Permanent as i32,
        }
    }

    /// Parse from protocol buffer enum value.
    #[cfg(feature = "server")]
    pub fn from_proto(value: i32) -> Self {
        match value {
            x if x == proto::ErrorCategory::Transient as i32 => Self::Transient,
            x if x == proto::ErrorCategory::Permanent as i32 => Self::Permanent,
            // Legacy: map Business to Permanent for backwards compatibility
            x if x == proto::ErrorCategory::Business as i32 => Self::Permanent,
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
    pub fn parse(s: &str) -> Self {
        match s {
            "info" => Self::Info,
            "warning" => Self::Warning,
            "critical" => Self::Critical,
            _ => Self::Error,
        }
    }

    /// Convert to protocol buffer enum value.
    #[cfg(feature = "server")]
    pub fn to_proto(&self) -> i32 {
        match self {
            Self::Info => proto::ErrorSeverity::Info as i32,
            Self::Warning => proto::ErrorSeverity::Warning as i32,
            Self::Error => proto::ErrorSeverity::Error as i32,
            Self::Critical => proto::ErrorSeverity::Critical as i32,
        }
    }

    /// Parse from protocol buffer enum value.
    #[cfg(feature = "server")]
    pub fn from_proto(value: i32) -> Self {
        match value {
            x if x == proto::ErrorSeverity::Info as i32 => Self::Info,
            x if x == proto::ErrorSeverity::Warning as i32 => Self::Warning,
            x if x == proto::ErrorSeverity::Critical as i32 => Self::Critical,
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

    /// Convert to protocol buffer enum value.
    #[cfg(feature = "server")]
    pub fn to_proto(&self) -> i32 {
        match self {
            Self::Unknown => proto::RetryHint::Unknown as i32,
            Self::RetryImmediately => proto::RetryHint::RetryImmediately as i32,
            Self::RetryWithBackoff => proto::RetryHint::RetryWithBackoff as i32,
            Self::RetryAfter(_) => proto::RetryHint::RetryAfter as i32,
            Self::DoNotRetry => proto::RetryHint::DoNotRetry as i32,
        }
    }

    /// Parse from protocol buffer enum value.
    #[cfg(feature = "server")]
    pub fn from_proto(value: i32, retry_after_ms: Option<u64>) -> Self {
        match value {
            x if x == proto::RetryHint::RetryImmediately as i32 => Self::RetryImmediately,
            x if x == proto::RetryHint::RetryWithBackoff as i32 => Self::RetryWithBackoff,
            x if x == proto::RetryHint::RetryAfter as i32 => {
                Self::RetryAfter(retry_after_ms.unwrap_or(0))
            }
            x if x == proto::RetryHint::DoNotRetry as i32 => Self::DoNotRetry,
            _ => Self::Unknown,
        }
    }
}

/// Structured error with metadata for intelligent error handling.
///
/// This extends the basic error information with:
/// - Category: transient vs permanent (business errors are a subset of permanent)
/// - Severity: info/warning/error/critical for alerting
/// - Retry hint: whether and how to retry
/// - Attributes: additional context key-value pairs
/// - Cause: nested error for error chains
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StructuredError {
    /// Machine-readable error code (e.g., "RATE_LIMITED", "VALIDATION_ERROR")
    pub code: String,
    /// Human-readable error message
    pub message: String,
    /// Error category for routing/retry decisions
    #[serde(default)]
    pub category: ErrorCategory,
    /// Error severity for logging/alerting
    #[serde(default)]
    pub severity: ErrorSeverity,
    /// Hint for retry behavior
    #[serde(default)]
    pub retry_hint: RetryHint,
    /// Which step produced this error (if applicable)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_step_id: Option<String>,
    /// Additional context as key-value pairs
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub attributes: HashMap<String, String>,
    /// Nested cause error (for error chains)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cause: Option<Box<StructuredError>>,
}

impl StructuredError {
    /// Create a new structured error with minimal info.
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            category: ErrorCategory::Unknown,
            severity: ErrorSeverity::Error,
            retry_hint: RetryHint::Unknown,
            source_step_id: None,
            attributes: HashMap::new(),
            cause: None,
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
            cause: None,
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
            cause: None,
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

    /// Set a cause error.
    pub fn with_cause(mut self, cause: StructuredError) -> Self {
        self.cause = Some(Box::new(cause));
        self
    }

    /// Set the severity.
    pub fn with_severity(mut self, severity: ErrorSeverity) -> Self {
        self.severity = severity;
        self
    }

    /// Set the retry hint.
    pub fn with_retry_hint(mut self, hint: RetryHint) -> Self {
        self.retry_hint = hint;
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

    /// Convert to protocol StructuredError message.
    #[cfg(feature = "server")]
    pub fn to_proto(&self) -> proto::StructuredError {
        use runtara_protocol::prost::Message;

        proto::StructuredError {
            code: self.code.clone(),
            message: self.message.clone(),
            metadata: Some(proto::ErrorMetadata {
                category: self.category.to_proto(),
                severity: self.severity.to_proto(),
                retry_hint: self.retry_hint.to_proto(),
                retry_after_ms: self.retry_hint.retry_after_ms(),
                error_code: Some(self.code.clone()),
                attributes: self.attributes.clone(),
            }),
            source_step_id: self.source_step_id.clone(),
            cause: self.cause.as_ref().map(|c| c.to_proto().encode_to_vec()),
        }
    }

    /// Convert from protocol StructuredError message.
    #[cfg(feature = "server")]
    pub fn from_proto(proto_err: &proto::StructuredError) -> Self {
        use runtara_protocol::prost::Message;

        let (category, severity, retry_hint) = if let Some(meta) = &proto_err.metadata {
            (
                ErrorCategory::from_proto(meta.category),
                ErrorSeverity::from_proto(meta.severity),
                RetryHint::from_proto(meta.retry_hint, meta.retry_after_ms),
            )
        } else {
            (
                ErrorCategory::Unknown,
                ErrorSeverity::Error,
                RetryHint::Unknown,
            )
        };

        Self {
            code: proto_err.code.clone(),
            message: proto_err.message.clone(),
            category,
            severity,
            retry_hint,
            source_step_id: proto_err.source_step_id.clone(),
            attributes: proto_err
                .metadata
                .as_ref()
                .map(|m| m.attributes.clone())
                .unwrap_or_default(),
            cause: proto_err.cause.as_ref().and_then(|bytes| {
                proto::StructuredError::decode(bytes.as_slice())
                    .ok()
                    .map(|p| Box::new(Self::from_proto(&p)))
            }),
        }
    }
}

impl fmt::Display for StructuredError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}] {}", self.code, self.message)
    }
}

impl std::error::Error for StructuredError {}

// ============================================================================
// CoreError (existing)
// ============================================================================

/// Result type using CoreError
pub type Result<T> = std::result::Result<T, CoreError>;

/// Core errors that can occur during request processing.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum CoreError {
    /// Instance was not found in the database.
    InstanceNotFound {
        /// The instance ID that was not found.
        instance_id: String,
    },

    /// Instance already exists (duplicate registration).
    InstanceAlreadyExists {
        /// The instance ID that already exists.
        instance_id: String,
    },

    /// Instance is in an invalid state for the requested operation.
    InvalidInstanceState {
        /// The instance ID.
        instance_id: String,
        /// The expected status.
        expected: String,
        /// The actual status.
        actual: String,
    },

    /// Checkpoint was not found.
    CheckpointNotFound {
        /// The instance ID.
        instance_id: String,
        /// The checkpoint ID that was not found.
        checkpoint_id: Option<String>,
    },

    /// Checkpoint save failed.
    CheckpointSaveFailed {
        /// The instance ID.
        instance_id: String,
        /// The reason for failure.
        reason: String,
    },

    /// Signal delivery failed.
    SignalDeliveryFailed {
        /// The instance ID.
        instance_id: String,
        /// The signal type that failed.
        signal_type: String,
        /// The reason for failure.
        reason: String,
    },

    /// Input validation failed.
    ValidationError {
        /// The field that failed validation.
        field: String,
        /// The validation error message.
        message: String,
    },

    /// Database operation failed.
    DatabaseError {
        /// The operation that failed.
        operation: String,
        /// Error details.
        details: String,
    },
}

impl CoreError {
    /// Convert this error to an RpcError for protocol responses.
    #[cfg(feature = "server")]
    pub fn to_rpc_error(&self) -> RpcError {
        RpcError {
            code: self.error_code().to_string(),
            message: self.to_string(),
        }
    }

    /// Get the error code string for this error type.
    pub fn error_code(&self) -> &'static str {
        match self {
            Self::InstanceNotFound { .. } => "INSTANCE_NOT_FOUND",
            Self::InstanceAlreadyExists { .. } => "INSTANCE_ALREADY_EXISTS",
            Self::InvalidInstanceState { .. } => "INVALID_INSTANCE_STATE",
            Self::CheckpointNotFound { .. } => "CHECKPOINT_NOT_FOUND",
            Self::CheckpointSaveFailed { .. } => "CHECKPOINT_SAVE_FAILED",
            Self::SignalDeliveryFailed { .. } => "SIGNAL_DELIVERY_FAILED",
            Self::ValidationError { .. } => "VALIDATION_ERROR",
            Self::DatabaseError { .. } => "DATABASE_ERROR",
        }
    }
}

impl fmt::Display for CoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InstanceNotFound { instance_id } => {
                write!(f, "Instance '{}' not found", instance_id)
            }
            Self::InstanceAlreadyExists { instance_id } => {
                write!(f, "Instance '{}' already exists", instance_id)
            }
            Self::InvalidInstanceState {
                instance_id,
                expected,
                actual,
            } => {
                write!(
                    f,
                    "Instance '{}' is in invalid state: expected '{}', got '{}'",
                    instance_id, expected, actual
                )
            }
            Self::CheckpointNotFound {
                instance_id,
                checkpoint_id,
            } => {
                if let Some(cp_id) = checkpoint_id {
                    write!(
                        f,
                        "Checkpoint '{}' not found for instance '{}'",
                        cp_id, instance_id
                    )
                } else {
                    write!(f, "No checkpoints found for instance '{}'", instance_id)
                }
            }
            Self::CheckpointSaveFailed {
                instance_id,
                reason,
            } => {
                write!(
                    f,
                    "Failed to save checkpoint for instance '{}': {}",
                    instance_id, reason
                )
            }
            Self::SignalDeliveryFailed {
                instance_id,
                signal_type,
                reason,
            } => {
                write!(
                    f,
                    "Failed to deliver {} signal to instance '{}': {}",
                    signal_type, instance_id, reason
                )
            }
            Self::ValidationError { field, message } => {
                write!(f, "Validation error for '{}': {}", field, message)
            }
            Self::DatabaseError { operation, details } => {
                write!(f, "Database error during '{}': {}", operation, details)
            }
        }
    }
}

impl std::error::Error for CoreError {}

impl From<sqlx::Error> for CoreError {
    fn from(err: sqlx::Error) -> Self {
        CoreError::DatabaseError {
            operation: "query".to_string(),
            details: err.to_string(),
        }
    }
}

impl From<serde_json::Error> for CoreError {
    fn from(err: serde_json::Error) -> Self {
        CoreError::DatabaseError {
            operation: "json".to_string(),
            details: err.to_string(),
        }
    }
}

impl From<CoreError> for StructuredError {
    fn from(err: CoreError) -> Self {
        let (category, severity) = match &err {
            CoreError::InstanceNotFound { .. } => (ErrorCategory::Permanent, ErrorSeverity::Error),
            CoreError::InstanceAlreadyExists { .. } => {
                (ErrorCategory::Permanent, ErrorSeverity::Warning)
            }
            CoreError::InvalidInstanceState { .. } => {
                (ErrorCategory::Permanent, ErrorSeverity::Error)
            }
            CoreError::CheckpointNotFound { .. } => {
                (ErrorCategory::Permanent, ErrorSeverity::Error)
            }
            CoreError::CheckpointSaveFailed { .. } => {
                (ErrorCategory::Transient, ErrorSeverity::Error)
            }
            CoreError::SignalDeliveryFailed { .. } => {
                (ErrorCategory::Transient, ErrorSeverity::Warning)
            }
            CoreError::ValidationError { .. } => (ErrorCategory::Permanent, ErrorSeverity::Error),
            CoreError::DatabaseError { .. } => (ErrorCategory::Transient, ErrorSeverity::Critical),
        };

        let retry_hint = if category == ErrorCategory::Transient {
            RetryHint::RetryWithBackoff
        } else {
            RetryHint::DoNotRetry
        };

        Self {
            code: err.error_code().to_string(),
            message: err.to_string(),
            category,
            severity,
            retry_hint,
            source_step_id: None,
            attributes: HashMap::new(),
            cause: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(feature = "server")]
    fn test_core_error_to_rpc_error_codes() {
        let test_cases = vec![
            (
                CoreError::InstanceNotFound {
                    instance_id: "test-id".to_string(),
                },
                "INSTANCE_NOT_FOUND",
            ),
            (
                CoreError::InstanceAlreadyExists {
                    instance_id: "test-id".to_string(),
                },
                "INSTANCE_ALREADY_EXISTS",
            ),
            (
                CoreError::InvalidInstanceState {
                    instance_id: "test-id".to_string(),
                    expected: "running".to_string(),
                    actual: "pending".to_string(),
                },
                "INVALID_INSTANCE_STATE",
            ),
            (
                CoreError::CheckpointNotFound {
                    instance_id: "test-id".to_string(),
                    checkpoint_id: Some("cp-1".to_string()),
                },
                "CHECKPOINT_NOT_FOUND",
            ),
            (
                CoreError::CheckpointSaveFailed {
                    instance_id: "test-id".to_string(),
                    reason: "disk full".to_string(),
                },
                "CHECKPOINT_SAVE_FAILED",
            ),
            (
                CoreError::SignalDeliveryFailed {
                    instance_id: "test-id".to_string(),
                    signal_type: "cancel".to_string(),
                    reason: "timeout".to_string(),
                },
                "SIGNAL_DELIVERY_FAILED",
            ),
            (
                CoreError::ValidationError {
                    field: "instance_id".to_string(),
                    message: "invalid UUID".to_string(),
                },
                "VALIDATION_ERROR",
            ),
            (
                CoreError::DatabaseError {
                    operation: "insert".to_string(),
                    details: "connection refused".to_string(),
                },
                "DATABASE_ERROR",
            ),
        ];

        for (error, expected_code) in test_cases {
            let rpc_error = error.to_rpc_error();
            assert_eq!(
                rpc_error.code, expected_code,
                "Error {:?} should have code {}",
                error, expected_code
            );
            assert!(!rpc_error.message.is_empty(), "Message should not be empty");
        }
    }

    #[test]
    fn test_core_error_display() {
        // Test InstanceNotFound
        let err = CoreError::InstanceNotFound {
            instance_id: "abc-123".to_string(),
        };
        assert_eq!(err.to_string(), "Instance 'abc-123' not found");

        // Test InstanceAlreadyExists
        let err = CoreError::InstanceAlreadyExists {
            instance_id: "abc-123".to_string(),
        };
        assert_eq!(err.to_string(), "Instance 'abc-123' already exists");

        // Test InvalidInstanceState
        let err = CoreError::InvalidInstanceState {
            instance_id: "abc-123".to_string(),
            expected: "running".to_string(),
            actual: "pending".to_string(),
        };
        assert_eq!(
            err.to_string(),
            "Instance 'abc-123' is in invalid state: expected 'running', got 'pending'"
        );

        // Test CheckpointNotFound with checkpoint_id
        let err = CoreError::CheckpointNotFound {
            instance_id: "abc-123".to_string(),
            checkpoint_id: Some("cp-1".to_string()),
        };
        assert_eq!(
            err.to_string(),
            "Checkpoint 'cp-1' not found for instance 'abc-123'"
        );

        // Test CheckpointNotFound without checkpoint_id
        let err = CoreError::CheckpointNotFound {
            instance_id: "abc-123".to_string(),
            checkpoint_id: None,
        };
        assert_eq!(
            err.to_string(),
            "No checkpoints found for instance 'abc-123'"
        );

        // Test ValidationError
        let err = CoreError::ValidationError {
            field: "instance_id".to_string(),
            message: "must be a valid UUID".to_string(),
        };
        assert_eq!(
            err.to_string(),
            "Validation error for 'instance_id': must be a valid UUID"
        );

        // Test DatabaseError
        let err = CoreError::DatabaseError {
            operation: "insert".to_string(),
            details: "connection refused".to_string(),
        };
        assert_eq!(
            err.to_string(),
            "Database error during 'insert': connection refused"
        );
    }

    #[test]
    fn test_error_code_method() {
        assert_eq!(
            CoreError::InstanceNotFound {
                instance_id: "x".to_string()
            }
            .error_code(),
            "INSTANCE_NOT_FOUND"
        );
        assert_eq!(
            CoreError::ValidationError {
                field: "x".to_string(),
                message: "y".to_string()
            }
            .error_code(),
            "VALIDATION_ERROR"
        );
    }
}
