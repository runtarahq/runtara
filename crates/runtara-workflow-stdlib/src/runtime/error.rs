// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Error types for workflow execution

use std::fmt;

/// Error type for workflow execution
#[derive(Debug)]
pub enum Error {
    /// Step execution failed
    StepFailed(String),
    /// Invalid input
    InvalidInput(String),
    /// Agent error
    AgentError(String),
    /// IO error
    IoError(std::io::Error),
    /// JSON error
    JsonError(serde_json::Error),
    /// Other error
    Other(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::StepFailed(msg) => write!(f, "Step failed: {}", msg),
            Error::InvalidInput(msg) => write!(f, "Invalid input: {}", msg),
            Error::AgentError(msg) => write!(f, "Agent error: {}", msg),
            Error::IoError(e) => write!(f, "IO error: {}", e),
            Error::JsonError(e) => write!(f, "JSON error: {}", e),
            Error::Other(msg) => write!(f, "{}", msg),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::IoError(e) => Some(e),
            Error::JsonError(e) => Some(e),
            _ => None,
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::IoError(e)
    }
}

impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Error::JsonError(e)
    }
}

impl From<String> for Error {
    fn from(s: String) -> Self {
        Error::Other(s)
    }
}

impl From<&str> for Error {
    fn from(s: &str) -> Self {
        Error::Other(s.to_string())
    }
}

/// Result type for workflow execution
pub type Result<T> = std::result::Result<T, Error>;
