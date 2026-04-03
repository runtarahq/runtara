//! Error types for Object Store operations

use thiserror::Error;

/// Errors that can occur during object store operations
#[derive(Debug, Error)]
pub enum ObjectStoreError {
    #[error("Validation error: {0}")]
    Validation(String),

    #[error("Schema not found: {0}")]
    SchemaNotFound(String),

    #[error("Instance not found: {0}")]
    InstanceNotFound(String),

    #[error("Conflict: {0}")]
    Conflict(String),

    #[error("Database error: {0}")]
    Database(String),

    #[error("SQL error: {0}")]
    Sql(#[from] sqlx::Error),

    #[error("Invalid condition: {0}")]
    InvalidCondition(String),

    #[error("Connection error: {0}")]
    Connection(String),

    #[error("JSON serialization error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Bulk operation failed: {0}")]
    BulkOperation(String),
}

impl ObjectStoreError {
    pub fn validation(msg: impl Into<String>) -> Self {
        Self::Validation(msg.into())
    }

    pub fn schema_not_found(msg: impl Into<String>) -> Self {
        Self::SchemaNotFound(msg.into())
    }

    pub fn instance_not_found(msg: impl Into<String>) -> Self {
        Self::InstanceNotFound(msg.into())
    }

    pub fn conflict(msg: impl Into<String>) -> Self {
        Self::Conflict(msg.into())
    }

    pub fn database(msg: impl Into<String>) -> Self {
        Self::Database(msg.into())
    }

    pub fn bulk_operation(msg: impl Into<String>) -> Self {
        Self::BulkOperation(msg.into())
    }
}

pub type Result<T> = std::result::Result<T, ObjectStoreError>;
