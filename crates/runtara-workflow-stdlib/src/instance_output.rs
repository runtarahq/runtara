// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Instance output handling for workflow binaries.
//!
//! Workflows communicate their exit state via output.json file.
//! Environment reads this file to determine the next action (scheduling wake, marking completed, etc.).
//!
//! The output file path is constructed from environment variables:
//! - DATA_DIR: Base data directory (defaults to current dir)
//! - RUNTARA_TENANT_ID: Tenant identifier
//! - RUNTARA_INSTANCE_ID: Instance identifier
//!
//! Output path: $DATA_DIR/$TENANT_ID/runs/$INSTANCE_ID/output.json

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Instance output status.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstanceOutputStatus {
    /// Instance completed successfully
    Completed,
    /// Instance failed with an error
    Failed,
    /// Instance suspended (paused via signal)
    Suspended,
    /// Instance is sleeping (durable sleep requested)
    Sleeping,
    /// Instance was cancelled
    Cancelled,
}

/// Instance output written to output.json on exit.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstanceOutput {
    /// The status/reason for exit
    pub status: InstanceOutputStatus,

    /// Result data (for completed status)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,

    /// Error message (for failed status)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,

    /// Checkpoint ID to resume from (for suspended/sleeping status)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checkpoint_id: Option<String>,

    /// Wake delay in milliseconds (for sleeping status)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wake_after_ms: Option<u64>,
}

impl InstanceOutput {
    /// Create a completed output.
    pub fn completed(result: serde_json::Value) -> Self {
        Self {
            status: InstanceOutputStatus::Completed,
            result: Some(result),
            error: None,
            checkpoint_id: None,
            wake_after_ms: None,
        }
    }

    /// Create a failed output.
    pub fn failed(error: impl Into<String>) -> Self {
        Self {
            status: InstanceOutputStatus::Failed,
            result: None,
            error: Some(error.into()),
            checkpoint_id: None,
            wake_after_ms: None,
        }
    }

    /// Create a suspended output.
    pub fn suspended(checkpoint_id: impl Into<String>) -> Self {
        Self {
            status: InstanceOutputStatus::Suspended,
            result: None,
            error: None,
            checkpoint_id: Some(checkpoint_id.into()),
            wake_after_ms: None,
        }
    }

    /// Create a sleeping output.
    pub fn sleeping(checkpoint_id: impl Into<String>, wake_after_ms: u64) -> Self {
        Self {
            status: InstanceOutputStatus::Sleeping,
            result: None,
            error: None,
            checkpoint_id: Some(checkpoint_id.into()),
            wake_after_ms: Some(wake_after_ms),
        }
    }

    /// Create a cancelled output.
    pub fn cancelled() -> Self {
        Self {
            status: InstanceOutputStatus::Cancelled,
            result: None,
            error: None,
            checkpoint_id: None,
            wake_after_ms: None,
        }
    }

    /// Write instance output to the standard output file location.
    ///
    /// Path is determined from environment variables:
    /// - DATA_DIR (defaults to ".")
    /// - RUNTARA_TENANT_ID
    /// - RUNTARA_INSTANCE_ID
    pub fn write_to_output_file(&self) -> std::io::Result<()> {
        let path = get_output_file_path();
        self.write_to_file(&path)
    }

    /// Write instance output to a specific file path.
    pub fn write_to_file(&self, path: &std::path::Path) -> std::io::Result<()> {
        let content = serde_json::to_string_pretty(self)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, content)
    }
}

/// Get the output file path for the current instance from environment variables.
pub fn get_output_file_path() -> PathBuf {
    let data_dir = std::env::var("DATA_DIR").unwrap_or_else(|_| ".".to_string());
    let tenant_id = std::env::var("RUNTARA_TENANT_ID").unwrap_or_else(|_| "default".to_string());
    let instance_id =
        std::env::var("RUNTARA_INSTANCE_ID").unwrap_or_else(|_| "unknown".to_string());

    PathBuf::from(data_dir)
        .join(tenant_id)
        .join("runs")
        .join(instance_id)
        .join("output.json")
}

/// Convenience function to write completed output.
pub fn write_completed(result: serde_json::Value) -> std::io::Result<()> {
    InstanceOutput::completed(result).write_to_output_file()
}

/// Convenience function to write failed output.
pub fn write_failed(error: impl Into<String>) -> std::io::Result<()> {
    InstanceOutput::failed(error).write_to_output_file()
}

/// Convenience function to write suspended output.
pub fn write_suspended(checkpoint_id: impl Into<String>) -> std::io::Result<()> {
    InstanceOutput::suspended(checkpoint_id).write_to_output_file()
}

/// Convenience function to write sleeping output.
pub fn write_sleeping(checkpoint_id: impl Into<String>, wake_after_ms: u64) -> std::io::Result<()> {
    InstanceOutput::sleeping(checkpoint_id, wake_after_ms).write_to_output_file()
}

/// Convenience function to write cancelled output.
pub fn write_cancelled() -> std::io::Result<()> {
    InstanceOutput::cancelled().write_to_output_file()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_serialize_completed() {
        let output = InstanceOutput::completed(serde_json::json!({"key": "value"}));
        let json = serde_json::to_string(&output).unwrap();
        assert!(json.contains("\"status\":\"completed\""));
        assert!(json.contains("\"result\""));
    }

    #[test]
    fn test_serialize_sleeping() {
        let output = InstanceOutput::sleeping("checkpoint-1", 3600000);
        let json = serde_json::to_string(&output).unwrap();
        assert!(json.contains("\"status\":\"sleeping\""));
        assert!(json.contains("\"checkpoint_id\":\"checkpoint-1\""));
        assert!(json.contains("\"wake_after_ms\":3600000"));
    }

    #[test]
    fn test_serialize_cancelled() {
        let output = InstanceOutput::cancelled();
        let json = serde_json::to_string(&output).unwrap();
        assert!(json.contains("\"status\":\"cancelled\""));
    }
}
