//! Step event reading for the API.
//!
//! Reads step execution events from Redis for debugging workflow executions.

use redis::{AsyncCommands, Client};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Represents a single step execution event
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StepEvent {
    /// Sequential number (execution order)
    pub sequence: u32,
    /// Step identifier
    pub step_id: String,
    /// Step type (Agent, Conditional, Split, etc.)
    pub step_type: String,
    /// Timestamp when step started (Unix milliseconds)
    pub timestamp_ms: i64,
    /// Step execution duration in milliseconds (None if still running)
    pub duration_ms: Option<u64>,
    /// Step status: "running", "completed", "failed"
    pub status: String,
    /// Step inputs (JSON string, captured only in track-events mode)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inputs: Option<String>,
    /// Step outputs (JSON string, captured only in track-events mode)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outputs: Option<String>,
    /// Error message (if status is "failed")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Reads step execution events from Redis (async operations)
pub struct StepEventReader {
    client: Option<Arc<Client>>,
    instance_id: String,
}

impl StepEventReader {
    /// Creates a new step event reader
    pub fn new(client: Option<Arc<Client>>, instance_id: String) -> Self {
        Self {
            client,
            instance_id,
        }
    }

    /// Retrieves step events with optional filtering
    pub async fn get_events(
        &self,
        step_id_filter: Option<&str>,
        status_filter: Option<&str>,
        limit: Option<usize>,
    ) -> Result<Vec<StepEvent>, String> {
        let client = self.client.as_ref().ok_or("Redis client not available")?;

        let mut conn = client
            .get_multiplexed_async_connection()
            .await
            .map_err(|e| format!("Failed to connect to Redis: {}", e))?;

        let key = format!("step_events:{}", self.instance_id);
        let events_data: Vec<(String, String)> = conn
            .hgetall(&key)
            .await
            .map_err(|e| format!("Failed to get events: {}", e))?;

        let mut events: Vec<StepEvent> = Vec::new();

        for (_, json) in events_data {
            if let Ok(event) = serde_json::from_str::<StepEvent>(&json) {
                // Apply filters
                if let Some(filter_step_id) = step_id_filter
                    && event.step_id != filter_step_id
                {
                    continue;
                }
                if let Some(filter_status) = status_filter
                    && event.status != filter_status
                {
                    continue;
                }
                events.push(event);
            }
        }

        // Sort by sequence
        events.sort_by_key(|e| e.sequence);

        // Apply limit
        if let Some(max_events) = limit {
            events.truncate(max_events);
        }

        Ok(events)
    }
}
