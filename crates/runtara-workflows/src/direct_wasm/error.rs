// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Shared error type for direct workflow compilation.

use std::fmt;

use super::manifest::DirectManifestError;
use super::support::{DirectWorkflowSupportReport, UnsupportedWorkflowFeature};

/// Errors returned by the opt-in direct compiler.
#[derive(Debug)]
pub enum DirectCompileError {
    /// Manifest construction failed.
    Manifest(DirectManifestError),
    /// Support report serialization failed.
    Serialize(serde_json::Error),
    /// The current direct compiler cannot emit this workflow yet.
    Unsupported {
        /// Deterministic support report with exact unsupported features.
        report: Box<DirectWorkflowSupportReport>,
    },
    /// Filesystem write or metadata read failed.
    Io(std::io::Error),
    /// Component-model artifact emission failed.
    Component(String),
}

impl fmt::Display for DirectCompileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DirectCompileError::Manifest(err) => write!(f, "{err}"),
            DirectCompileError::Serialize(err) => {
                write!(
                    f,
                    "failed to serialize direct workflow artifact metadata: {err}"
                )
            }
            DirectCompileError::Unsupported { report } => write!(
                f,
                "direct workflow compiler does not support this graph yet: {}",
                unsupported_summary(&report.unsupported)
            ),
            DirectCompileError::Io(err) => {
                write!(f, "direct workflow artifact write failed: {err}")
            }
            DirectCompileError::Component(err) => {
                write!(f, "direct workflow component emission failed: {err}")
            }
        }
    }
}

impl std::error::Error for DirectCompileError {}

impl From<DirectManifestError> for DirectCompileError {
    fn from(value: DirectManifestError) -> Self {
        Self::Manifest(value)
    }
}

impl From<serde_json::Error> for DirectCompileError {
    fn from(value: serde_json::Error) -> Self {
        Self::Serialize(value)
    }
}

impl From<std::io::Error> for DirectCompileError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

fn unsupported_summary(unsupported: &[UnsupportedWorkflowFeature]) -> String {
    if unsupported.is_empty() {
        return "no unsupported features reported".to_string();
    }

    unsupported
        .iter()
        .map(|feature| {
            let step = feature.step_id.as_deref().unwrap_or("<graph>");
            format!("{step}:{}", feature.feature)
        })
        .collect::<Vec<_>>()
        .join(", ")
}
