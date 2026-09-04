// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Error types for runtara-core.
//!
//! Provides a unified error type that maps to RPC error responses.

use std::fmt;

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

/// What kind of failure a [`CoreError`] is, independent of any transport.
///
/// Core does not know about HTTP, but it is the only place that can say what a
/// given variant *means* — whether the caller asked for something absent, sent
/// something malformed, or hit a server that is temporarily unwell. A transport
/// turns that into its own vocabulary: `runtara-server`'s instance API maps
/// these to 404 / 409 / 400 / 503.
///
/// The point of naming the classification here is that
/// [`CoreError::classify`] matches exhaustively, so a new variant fails to
/// compile until someone decides what it means — a guarantee that would be lost
/// if each transport classified the variants itself behind a `_` arm.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoreErrorClass {
    /// The caller named something that is not there.
    Missing,
    /// The thing exists but is not in a state that accepts this request.
    Conflict,
    /// The request itself is malformed.
    Invalid,
    /// The server could not do its job. Retrying may work.
    Unavailable,
}

impl CoreError {
    /// Classify this error for a transport to render.
    ///
    /// See [`CoreErrorClass`] for why this lives in core rather than in the
    /// layer that maps it onto status codes.
    pub fn classify(&self) -> CoreErrorClass {
        match self {
            Self::InstanceNotFound { .. } | Self::CheckpointNotFound { .. } => {
                CoreErrorClass::Missing
            }
            Self::InvalidInstanceState { .. } | Self::InstanceAlreadyExists { .. } => {
                CoreErrorClass::Conflict
            }
            Self::ValidationError { .. } => CoreErrorClass::Invalid,
            Self::DatabaseError { .. }
            | Self::CheckpointSaveFailed { .. }
            | Self::SignalDeliveryFailed { .. } => CoreErrorClass::Unavailable,
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

impl From<serde_json::Error> for CoreError {
    fn from(err: serde_json::Error) -> Self {
        CoreError::DatabaseError {
            operation: "json".to_string(),
            details: err.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_core_error_codes() {
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
            assert_eq!(
                error.error_code(),
                expected_code,
                "Error {:?} should have code {}",
                error,
                expected_code
            );
            assert!(!error.to_string().is_empty(), "Message should not be empty");
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
