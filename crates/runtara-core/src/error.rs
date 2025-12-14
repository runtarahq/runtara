// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Error types for runtara-core.
//!
//! Provides a unified error type that maps to RPC error responses.

#![allow(dead_code)] // Variants and methods used in tests and for future expansion

use runtara_protocol::instance_proto::RpcError;
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

impl CoreError {
    /// Convert this error to an RpcError for protocol responses.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
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
