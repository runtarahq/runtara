// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Error types for runtara-environment.

use thiserror::Error;

/// Environment errors.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum Error {
    /// Database operation failed.
    #[error("Database error: {0}")]
    Database(#[from] sqlx::Error),

    /// I/O operation failed.
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    /// JSON serialization/deserialization failed.
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    /// Runner (container/process execution) failed.
    #[error("Runner error: {0}")]
    Runner(#[from] crate::runner::RunnerError),

    /// Core persistence operation failed.
    #[error("Core error: {0}")]
    Core(#[from] runtara_core::error::CoreError),

    /// Image was not found.
    #[error("Image not found: {0}")]
    ImageNotFound(String),

    /// Instance was not found.
    #[error("Instance not found: {0}")]
    InstanceNotFound(String),

    /// Request validation failed.
    #[error("Invalid request: {0}")]
    InvalidRequest(String),

    /// Other error.
    #[error("{0}")]
    Other(String),
}

/// Result type using Environment Error.
pub type Result<T> = std::result::Result<T, Error>;
