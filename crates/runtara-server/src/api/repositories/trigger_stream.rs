//! Trigger Stream Publisher
//!
//! Publishes trigger events to Redis/Valkey streams for async workflow execution.
//! Stream naming: runtara:triggers:{tenant_id}

use redis::AsyncCommands;
use redis::aio::ConnectionManager;

use crate::api::dto::trigger_event::TriggerEvent;

/// Publisher for trigger events to Redis streams
pub struct TriggerStreamPublisher {
    /// Shared Redis connection manager (built once at startup; cloned per
    /// publish to reuse the existing connection pool).
    manager: ConnectionManager,
}

impl TriggerStreamPublisher {
    /// Create a new publisher from a shared connection manager.
    pub fn new(manager: ConnectionManager) -> Self {
        Self { manager }
    }

    /// Publish a trigger event to the tenant's trigger stream
    ///
    /// Returns the stream entry ID on success
    pub async fn publish(
        &self,
        tenant_id: &str,
        event: &TriggerEvent,
    ) -> Result<String, TriggerStreamError> {
        // Serialize event to JSON
        let event_json = serde_json::to_string(event)
            .map_err(|e| TriggerStreamError::SerializationError(e.to_string()))?;

        // Reuse the shared connection manager — no new TCP per call.
        let mut redis_conn = self.manager.clone();

        // Construct Redis stream key
        let stream_key = format!("runtara:triggers:{}", tenant_id);

        // Add to Redis stream using XADD with auto-generated ID
        // Store event_type for filtering and full event data as JSON
        let stream_id: String = redis_conn
            .xadd(
                &stream_key,
                "*", // Auto-generate ID
                &[
                    ("event_type", "trigger"),
                    ("trigger_type", event.trigger_type()),
                    ("instance_id", &event.instance_id),
                    ("workflow_id", &event.workflow_id),
                    ("data", &event_json),
                ],
            )
            .await
            .map_err(|e| TriggerStreamError::RedisError(e.to_string()))?;

        tracing::debug!(
            stream_key = %stream_key,
            stream_id = %stream_id,
            instance_id = %event.instance_id,
            workflow_id = %event.workflow_id,
            trigger_type = %event.trigger_type(),
            "Published trigger event to stream"
        );

        Ok(stream_id)
    }

    /// Get the stream key for a tenant
    pub fn stream_key(tenant_id: &str) -> String {
        format!("runtara:triggers:{}", tenant_id)
    }
}

/// Errors that can occur when publishing to the trigger stream
#[derive(Debug)]
#[allow(clippy::enum_variant_names)]
pub enum TriggerStreamError {
    /// Failed to serialize event to JSON
    SerializationError(String),
    /// Failed to connect to Redis
    ConnectionError(String),
    /// Redis operation failed
    RedisError(String),
}

impl std::fmt::Display for TriggerStreamError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TriggerStreamError::SerializationError(msg) => {
                write!(f, "Serialization error: {}", msg)
            }
            TriggerStreamError::ConnectionError(msg) => {
                write!(f, "Redis connection error: {}", msg)
            }
            TriggerStreamError::RedisError(msg) => {
                write!(f, "Redis operation error: {}", msg)
            }
        }
    }
}

impl std::error::Error for TriggerStreamError {}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_stream_key() {
        assert_eq!(
            TriggerStreamPublisher::stream_key("tenant-123"),
            "runtara:triggers:tenant-123"
        );
    }

    #[test]
    fn test_trigger_event_serialization() {
        let event = TriggerEvent::http_api(
            "instance-1".to_string(),
            "tenant-1".to_string(),
            "workflow-1".to_string(),
            Some(1),
            json!({"input": "value"}),
            false,
            None,
            false,
        );

        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("instance-1"));
        assert!(json.contains("workflow-1"));
    }
}
