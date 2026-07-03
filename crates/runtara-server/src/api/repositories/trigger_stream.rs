//! Trigger Stream Publisher
//!
//! Publishes trigger events to Redis/Valkey streams for async workflow execution.
//! Stream naming: {trigger_stream_prefix}:{tenant_id}, where the prefix is the
//! same `VALKEY_TRIGGER_STREAM_PREFIX`-derived value the trigger worker consumes
//! from — so publisher and consumer always agree on the key.

use redis::AsyncCommands;
use redis::aio::ConnectionManager;

use crate::api::dto::trigger_event::TriggerEvent;

/// Publisher for trigger events to Redis streams
pub struct TriggerStreamPublisher {
    /// Shared Redis connection manager (built once at startup; cloned per
    /// publish to reuse the existing connection pool).
    manager: ConnectionManager,
    /// Stream key prefix (must match the trigger worker's configured prefix, so
    /// the server publishes where a worker actually reads).
    trigger_stream_prefix: String,
    /// Approximate cap on stream length applied at publish time (`MAXLEN ~ N`),
    /// so consumed-but-unacked-trimmed events don't accumulate without bound.
    trigger_stream_maxlen: usize,
}

impl TriggerStreamPublisher {
    /// Create a new publisher from a shared connection manager, the trigger
    /// stream prefix, and the approximate stream length cap (from
    /// `ValkeyConfig`).
    pub fn new(
        manager: ConnectionManager,
        trigger_stream_prefix: String,
        trigger_stream_maxlen: usize,
    ) -> Self {
        Self {
            manager,
            trigger_stream_prefix,
            trigger_stream_maxlen,
        }
    }

    /// The configured stream key prefix.
    pub fn stream_prefix(&self) -> &str {
        &self.trigger_stream_prefix
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

        // Construct Redis stream key from the configured prefix.
        let stream_key = self.stream_key(tenant_id);

        // Add to Redis stream using XADD with auto-generated ID, bounding the
        // stream to an approximate max length so it can't grow without limit.
        // Store event_type for filtering and full event data as JSON.
        let stream_id: String = redis_conn
            .xadd_maxlen(
                &stream_key,
                redis::streams::StreamMaxlen::Approx(self.trigger_stream_maxlen),
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

    /// Get the stream key for a tenant, using this publisher's configured prefix.
    pub fn stream_key(&self, tenant_id: &str) -> String {
        Self::build_stream_key(&self.trigger_stream_prefix, tenant_id)
    }

    /// Build a trigger stream key from a prefix and tenant id.
    fn build_stream_key(prefix: &str, tenant_id: &str) -> String {
        format!("{}:{}", prefix, tenant_id)
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
    fn test_build_stream_key() {
        assert_eq!(
            TriggerStreamPublisher::build_stream_key("runtara:triggers", "tenant-123"),
            "runtara:triggers:tenant-123"
        );
        // A custom prefix (e.g. for multi-server isolation) is honored.
        assert_eq!(
            TriggerStreamPublisher::build_stream_key("srvA:triggers", "tenant-123"),
            "srvA:triggers:tenant-123"
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
