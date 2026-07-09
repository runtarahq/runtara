//! Trigger Stream Publisher
//!
//! Publishes trigger events to Redis/Valkey streams for async workflow execution.
//! Stream naming: {trigger_stream_prefix}:{tenant_id}, where the prefix is the
//! same `VALKEY_TRIGGER_STREAM_PREFIX`-derived value the trigger worker consumes
//! from — so publisher and consumer always agree on the key.

use redis::AsyncCommands;
use redis::aio::ConnectionManager;

use crate::api::dto::trigger_event::TriggerEvent;
use crate::valkey::ValkeyConfig;

/// Publisher for trigger events to Redis streams
pub struct TriggerStreamPublisher {
    /// Shared Redis connection manager (built once at startup; cloned per
    /// publish to reuse the existing connection pool).
    manager: ConnectionManager,
    /// Shared Valkey configuration. Holding the whole config (rather than
    /// copying out individual fields like prefix/maxlen) means the stream key
    /// is always built via `ValkeyConfig::trigger_stream_key` — the same
    /// method the trigger worker uses to compute its consuming key — and any
    /// future config field the publisher needs doesn't require a constructor
    /// signature change.
    config: ValkeyConfig,
}

impl TriggerStreamPublisher {
    /// Create a new publisher from a shared connection manager and the
    /// process's Valkey configuration.
    pub fn new(manager: ConnectionManager, config: ValkeyConfig) -> Self {
        Self { manager, config }
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
        //
        // Residual risk: MAXLEN trimming is approximate and can remove entries
        // that are still in the trigger consumer group's PEL (claimed-but-unacked,
        // or never-yet-read) if the consumer falls behind by more than
        // `trigger_stream_maxlen` events. `StreamConsumer::claim_pending_events`
        // (valkey/stream.rs) logs a warning when XAUTOCLAIM reports such entries,
        // so a sustained backlog is observable rather than silently dropping
        // trigger events with no signal.
        let stream_id: String = redis_conn
            .xadd_maxlen(
                &stream_key,
                redis::streams::StreamMaxlen::Approx(self.config.trigger_stream_maxlen),
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

    /// Get the stream key for a tenant. Delegates to `ValkeyConfig::trigger_stream_key`
    /// — the same method the trigger worker uses to compute its consuming key —
    /// so there is exactly one implementation of the key format.
    pub fn stream_key(&self, tenant_id: &str) -> String {
        self.config.trigger_stream_key(tenant_id)
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

    fn test_config(prefix: &str) -> ValkeyConfig {
        ValkeyConfig {
            host: "localhost".to_string(),
            port: 6379,
            user: None,
            password: None,
            stream_name: "runtara-events".to_string(),
            consumer_group: "runtara-workers".to_string(),
            trigger_stream_prefix: prefix.to_string(),
            trigger_consumer_group: "runtara-trigger-workers".to_string(),
            trigger_stream_maxlen: crate::valkey::DEFAULT_TRIGGER_STREAM_MAXLEN,
            tls: false,
            tls_insecure: false,
            tls_ca_cert: None,
        }
    }

    #[test]
    fn test_stream_key_uses_configured_prefix() {
        // The publisher's stream_key delegates to ValkeyConfig::trigger_stream_key
        // — the same key the trigger worker computes to consume from — so there
        // is exactly one implementation of the key format to keep in sync.
        assert_eq!(
            test_config("runtara:triggers").trigger_stream_key("tenant-123"),
            "runtara:triggers:tenant-123"
        );
        // A custom prefix (e.g. for multi-server isolation) is honored.
        assert_eq!(
            test_config("srvA:triggers").trigger_stream_key("tenant-123"),
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
