// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Instance output handling.
//!
//! Instances communicate their exit state via output.json file.
//! Environment reads this file to determine the next action.

use serde::{Deserialize, Serialize};
use std::path::Path;

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

    /// Read instance output from file.
    pub async fn read_from_file(path: &Path) -> std::io::Result<Self> {
        let content = tokio::fs::read_to_string(path).await?;
        serde_json::from_str(&content)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    }

    /// Write instance output to file.
    pub async fn write_to_file(&self, path: &Path) -> std::io::Result<()> {
        let content = serde_json::to_string_pretty(self)?;
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::write(path, content).await
    }
}

/// Get the output file path for an instance.
pub fn output_file_path(data_dir: &Path, tenant_id: &str, instance_id: &str) -> std::path::PathBuf {
    data_dir
        .join(tenant_id)
        .join("runs")
        .join(instance_id)
        .join("output.json")
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
    fn test_deserialize() {
        let json = r#"{"status":"suspended","checkpoint_id":"cp-123"}"#;
        let output: InstanceOutput = serde_json::from_str(json).unwrap();
        assert_eq!(output.status, InstanceOutputStatus::Suspended);
        assert_eq!(output.checkpoint_id, Some("cp-123".to_string()));
    }
}
