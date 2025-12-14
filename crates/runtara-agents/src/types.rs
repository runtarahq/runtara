// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Shared types used across agents

use base64::{Engine as _, engine::general_purpose};
use runtara_agent_macro::CapabilityOutput;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Represents a base64-encoded file payload that can flow through mappings
#[derive(Debug, Clone, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(
    display_name = "File Data",
    description = "Base64-encoded file with optional metadata"
)]
pub struct FileData {
    #[field(display_name = "Content", description = "Base64-encoded file content")]
    pub content: String,

    #[field(
        display_name = "Filename",
        description = "Original filename (optional)"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filename: Option<String>,

    #[field(
        display_name = "MIME Type",
        description = "MIME type (e.g., 'text/plain', 'text/csv', 'application/xml')"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "mimeType")]
    pub mime_type: Option<String>,
}

impl FileData {
    /// Decode the base64 content to raw bytes
    pub fn decode(&self) -> Result<Vec<u8>, String> {
        general_purpose::STANDARD
            .decode(&self.content)
            .map_err(|e| format!("Failed to decode base64 file content: {}", e))
    }

    /// Create FileData from raw bytes
    pub fn from_bytes(data: Vec<u8>, filename: Option<String>, mime_type: Option<String>) -> Self {
        FileData {
            content: general_purpose::STANDARD.encode(&data),
            filename,
            mime_type,
        }
    }

    /// Try to parse a Value as FileData
    pub fn from_value(value: &Value) -> Result<Self, String> {
        match value {
            Value::String(s) => Ok(FileData {
                content: s.clone(),
                filename: None,
                mime_type: None,
            }),
            Value::Object(_) => serde_json::from_value(value.clone())
                .map_err(|e| format!("Invalid file data structure: {}", e)),
            Value::Array(arr) => {
                let mut bytes = Vec::with_capacity(arr.len());
                for v in arr {
                    let num = v
                        .as_u64()
                        .ok_or_else(|| "Byte array must contain only numbers".to_string())?;
                    if num > 255 {
                        return Err("Byte values must be in the range 0-255".to_string());
                    }
                    bytes.push(num as u8);
                }
                Ok(FileData::from_bytes(bytes, None, None))
            }
            _ => Err(
                "File data must be a string (base64), byte array, or object with content field"
                    .to_string(),
            ),
        }
    }
}

/// Token usage statistics for LLM capabilities
#[derive(Debug, Clone, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(
    display_name = "LLM Usage",
    description = "Token count statistics from LLM API calls"
)]
#[serde(rename_all = "camelCase")]
pub struct LlmUsage {
    #[field(
        display_name = "Prompt Tokens",
        description = "Token count for input prompt",
        example = "150"
    )]
    pub prompt_tokens: i32,

    #[field(
        display_name = "Completion Tokens",
        description = "Token count for generated response",
        example = "50"
    )]
    pub completion_tokens: i32,

    #[field(
        display_name = "Total Tokens",
        description = "Combined token count",
        example = "200"
    )]
    pub total_tokens: i32,
}
