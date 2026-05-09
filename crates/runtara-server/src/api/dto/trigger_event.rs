//! Trigger event types for stream-based workflow execution
//!
//! These types define the event schema for all workflow triggers:
//! HTTP API, HTTP webhooks, cron schedules, email triggers, and application webhooks.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Event published to Valkey stream to trigger workflow execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TriggerEvent {
    /// Pre-generated instance ID (UUID string)
    pub instance_id: String,

    /// Tenant identifier
    pub tenant_id: String,

    /// Target workflow ID
    pub workflow_id: String,

    /// Specific version to execute (None = use current/latest)
    pub version: Option<i32>,

    /// Workflow input data (JSON)
    pub inputs: Value,

    /// Source of this trigger
    pub trigger: TriggerSource,

    /// Unix timestamp in milliseconds when the trigger was requested
    pub requested_at: i64,

    /// Whether step-event tracking is enabled for this execution
    pub track_events: bool,

    /// Whether debug mode is enabled (pause at breakpoints)
    #[serde(default)]
    pub debug: bool,
}

/// Source/type of trigger that initiated the execution
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TriggerSource {
    /// Direct HTTP API call to /execute endpoint
    HttpApi {
        /// Optional correlation ID for request tracing
        correlation_id: Option<String>,
    },

    /// HTTP event/webhook trigger via /events/http/{trigger_id}/{action}
    HttpEvent {
        /// Trigger ID from invocation_trigger table
        trigger_id: String,
        /// Action name from URL path
        action: String,
        /// HTTP method (GET, POST, etc.)
        method: String,
        /// HTTP headers as key-value pairs
        headers: Vec<(String, String)>,
    },

    /// Cron-scheduled trigger
    Cron {
        /// Trigger ID from invocation_trigger table
        trigger_id: String,
        /// Cron expression (e.g., "0 * * * *")
        schedule: String,
        /// Unix timestamp when this execution was scheduled
        scheduled_at: i64,
    },

    /// Email-triggered execution
    Email {
        /// Trigger ID from invocation_trigger table
        trigger_id: String,
        /// Sender email address
        from: String,
        /// Email subject (if available)
        subject: Option<String>,
    },

    /// Application webhook trigger (e.g., Shopify, external systems)
    Application {
        /// Trigger ID from invocation_trigger table
        trigger_id: String,
        /// Connection ID for the external application
        connection_id: String,
        /// Event type from the application (e.g., "orders/create")
        event_type: String,
    },

    /// Replay of a previous execution
    Replay {
        /// Original instance ID being replayed
        original_instance_id: String,
    },

    /// Recovery of a failed/interrupted execution with checkpoints
    Recovery {
        /// Number of checkpoints available for replay
        checkpoint_count: u64,
    },
}

impl TriggerEvent {
    /// Create a new HTTP API trigger event
    #[allow(clippy::too_many_arguments)]
    pub fn http_api(
        instance_id: String,
        tenant_id: String,
        workflow_id: String,
        version: Option<i32>,
        inputs: Value,
        track_events: bool,
        correlation_id: Option<String>,
        debug: bool,
    ) -> Self {
        Self {
            instance_id,
            tenant_id,
            workflow_id,
            version,
            inputs,
            trigger: TriggerSource::HttpApi { correlation_id },
            requested_at: chrono::Utc::now().timestamp_millis(),
            track_events,
            debug,
        }
    }

    /// Create a new HTTP event trigger
    #[allow(clippy::too_many_arguments)]
    pub fn http_event(
        instance_id: String,
        tenant_id: String,
        workflow_id: String,
        version: Option<i32>,
        inputs: Value,
        track_events: bool,
        trigger_id: String,
        action: String,
        method: String,
        headers: Vec<(String, String)>,
        debug: bool,
    ) -> Self {
        Self {
            instance_id,
            tenant_id,
            workflow_id,
            version,
            inputs,
            trigger: TriggerSource::HttpEvent {
                trigger_id,
                action,
                method,
                headers,
            },
            requested_at: chrono::Utc::now().timestamp_millis(),
            track_events,
            debug,
        }
    }

    /// Create a new cron trigger event
    #[allow(clippy::too_many_arguments)]
    pub fn cron(
        instance_id: String,
        tenant_id: String,
        workflow_id: String,
        version: Option<i32>,
        inputs: Value,
        track_events: bool,
        trigger_id: String,
        schedule: String,
        debug: bool,
    ) -> Self {
        let now = chrono::Utc::now().timestamp_millis();
        Self {
            instance_id,
            tenant_id,
            workflow_id,
            version,
            inputs,
            trigger: TriggerSource::Cron {
                trigger_id,
                schedule,
                scheduled_at: now,
            },
            requested_at: now,
            track_events,
            debug,
        }
    }

    /// Get the trigger ID if this event came from a registered trigger
    pub fn trigger_id(&self) -> Option<&str> {
        match &self.trigger {
            TriggerSource::HttpApi { .. } => None,
            TriggerSource::HttpEvent { trigger_id, .. } => Some(trigger_id),
            TriggerSource::Cron { trigger_id, .. } => Some(trigger_id),
            TriggerSource::Email { trigger_id, .. } => Some(trigger_id),
            TriggerSource::Application { trigger_id, .. } => Some(trigger_id),
            TriggerSource::Replay { .. } => None,
            TriggerSource::Recovery { .. } => None,
        }
    }

    /// Get the trigger type as a string for logging/metrics
    pub fn trigger_type(&self) -> &'static str {
        match &self.trigger {
            TriggerSource::HttpApi { .. } => "http_api",
            TriggerSource::HttpEvent { .. } => "http_event",
            TriggerSource::Cron { .. } => "cron",
            TriggerSource::Email { .. } => "email",
            TriggerSource::Application { .. } => "application",
            TriggerSource::Replay { .. } => "replay",
            TriggerSource::Recovery { .. } => "recovery",
        }
    }

    /// Check if this is a recovery trigger (execution record already exists)
    pub fn is_recovery(&self) -> bool {
        matches!(&self.trigger, TriggerSource::Recovery { .. })
    }

    /// Create a recovery trigger event
    pub fn recovery(
        instance_id: String,
        tenant_id: String,
        workflow_id: String,
        version: Option<i32>,
        inputs: Value,
        track_events: bool,
        checkpoint_count: u64,
    ) -> Self {
        Self {
            instance_id,
            tenant_id,
            workflow_id,
            version,
            inputs,
            trigger: TriggerSource::Recovery { checkpoint_count },
            requested_at: chrono::Utc::now().timestamp_millis(),
            track_events,
            debug: false,
        }
    }

    /// Create a replay trigger event.
    #[allow(clippy::too_many_arguments)]
    pub fn replay(
        instance_id: String,
        tenant_id: String,
        workflow_id: String,
        version: Option<i32>,
        inputs: Value,
        track_events: bool,
        original_instance_id: String,
        debug: bool,
    ) -> Self {
        Self {
            instance_id,
            tenant_id,
            workflow_id,
            version,
            inputs,
            trigger: TriggerSource::Replay {
                original_instance_id,
            },
            requested_at: chrono::Utc::now().timestamp_millis(),
            track_events,
            debug,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_trigger_event_serialization() {
        let event = TriggerEvent::http_api(
            "test-instance-id".to_string(),
            "test-tenant".to_string(),
            "test-workflow".to_string(),
            Some(1),
            json!({"key": "value"}),
            false,
            Some("correlation-123".to_string()),
            false,
        );

        let json = serde_json::to_string(&event).unwrap();
        let parsed: TriggerEvent = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.instance_id, "test-instance-id");
        assert_eq!(parsed.workflow_id, "test-workflow");
        assert!(matches!(parsed.trigger, TriggerSource::HttpApi { .. }));
    }

    #[test]
    fn test_trigger_source_serialization() {
        let source = TriggerSource::Cron {
            trigger_id: "trigger-1".to_string(),
            schedule: "0 * * * *".to_string(),
            scheduled_at: 1234567890,
        };

        let json = serde_json::to_string(&source).unwrap();
        assert!(json.contains("\"type\":\"cron\""));

        let parsed: TriggerSource = serde_json::from_str(&json).unwrap();
        assert!(matches!(parsed, TriggerSource::Cron { .. }));
    }

    #[test]
    fn test_replay_event_serialization() {
        let event = TriggerEvent::replay(
            "new-instance-id".to_string(),
            "test-tenant".to_string(),
            "test-workflow".to_string(),
            Some(3),
            json!({"data": {"key": "value"}, "variables": {}}),
            true,
            "original-instance-id".to_string(),
            false,
        );

        let json = serde_json::to_string(&event).unwrap();
        let parsed: TriggerEvent = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.instance_id, "new-instance-id");
        assert_eq!(parsed.workflow_id, "test-workflow");
        assert_eq!(parsed.version, Some(3));
        assert_eq!(parsed.trigger_type(), "replay");
        assert!(matches!(
            parsed.trigger,
            TriggerSource::Replay {
                original_instance_id
            } if original_instance_id == "original-instance-id"
        ));
    }
}
