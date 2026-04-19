use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

/// Event received from Valkey stream
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValkeyEvent {
    /// Unique event ID (from stream)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event_id: Option<String>,

    /// Event type (e.g., "trigger_workflow", "webhook", etc.)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event_type: Option<String>,

    /// Target workflow ID to invoke
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workflow_id: Option<String>,

    /// JSON payload for workflow inputs
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inputs: Option<Value>,

    /// Additional metadata
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,

    /// Raw stream data (all fields from the stream)
    #[serde(flatten)]
    pub raw_data: HashMap<String, String>,
}

impl ValkeyEvent {
    /// Parse event from Redis stream entry fields
    pub fn from_stream_fields(fields: HashMap<String, String>) -> Self {
        let event_type = fields.get("event_type").cloned();
        let workflow_id = fields.get("workflow_id").cloned();

        // Try to parse inputs field as JSON
        let inputs = fields
            .get("inputs")
            .and_then(|s| serde_json::from_str(s).ok());

        // Try to parse metadata field as JSON
        let metadata = fields
            .get("metadata")
            .and_then(|s| serde_json::from_str(s).ok());

        ValkeyEvent {
            event_id: None, // Will be set by caller with stream ID
            event_type,
            workflow_id,
            inputs,
            metadata,
            raw_data: fields,
        }
    }

    /// Set the event ID (from stream entry ID)
    pub fn with_event_id(mut self, event_id: String) -> Self {
        self.event_id = Some(event_id);
        self
    }
}
