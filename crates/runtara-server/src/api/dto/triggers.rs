/// Triggers-related DTOs
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::ToSchema;

// ============================================================================
// Enums
// ============================================================================

/// Trigger type for invocation triggers
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema, sqlx::Type)]
#[sqlx(type_name = "varchar", rename_all = "SCREAMING_SNAKE_CASE")]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum TriggerType {
    /// Invocation triggered by an HTTP request
    Http,
    /// Invocation triggered periodically with a frequency specified by a CRON expression
    Cron,
    /// Invocation triggered by an incoming email event
    Email,
    /// Invocation triggered by an event in an external system connected via a connection
    Application,
    /// Conversational channel (Telegram, Slack, Teams) — session-based, bidirectional
    Channel,
}

// ============================================================================
// Models
// ============================================================================

/// Invocation trigger model
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, sqlx::FromRow)]
pub struct InvocationTrigger {
    /// Unique identifier for the invocation trigger (auto-generated)
    #[schema(example = "550e8400-e29b-41d4-a716-446655440000")]
    pub id: String,

    /// Tenant identifier for multi-tenancy support
    #[schema(example = "tenant-123")]
    pub tenant_id: Option<String>,

    /// Reference to the scenario to be invoked
    #[schema(example = "scenario-456")]
    pub scenario_id: String,

    /// Type of trigger
    #[schema(example = "CRON")]
    pub trigger_type: TriggerType,

    /// Whether the trigger is currently active
    #[schema(example = true)]
    pub active: bool,

    /// Trigger-specific configuration in JSON format
    #[schema(value_type = Option<Object>, example = json!({"expression": "0 0 * * *", "timezone": "UTC"}))]
    pub configuration: Option<Value>,

    /// Timestamp when the trigger was created
    #[schema(value_type = String, example = "2025-01-15T10:30:00Z")]
    pub created_at: DateTime<Utc>,

    /// Timestamp of the last trigger execution (system-managed)
    #[schema(value_type = Option<String>, example = "2025-01-15T12:00:00Z")]
    pub last_run: Option<DateTime<Utc>>,

    /// Timestamp when the trigger was last updated
    #[schema(value_type = String, example = "2025-01-15T10:30:00Z")]
    pub updated_at: DateTime<Utc>,

    /// Remote tenant identifier for external system triggers
    #[schema(example = "remote-tenant-789")]
    pub remote_tenant_id: Option<String>,

    /// Whether only a single instance of this trigger should run at a time
    #[schema(example = false)]
    pub single_instance: bool,
}

// ============================================================================
// Request DTOs
// ============================================================================

/// Request payload for creating a new invocation trigger
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct CreateInvocationTriggerRequest {
    /// Reference to the scenario to be invoked
    #[schema(example = "scenario-456")]
    pub scenario_id: String,

    /// Type of trigger
    #[schema(example = "CRON")]
    pub trigger_type: TriggerType,

    /// Whether the trigger should be active upon creation
    #[schema(example = true)]
    #[serde(default = "default_active")]
    pub active: bool,

    /// Trigger-specific configuration in JSON format
    #[schema(value_type = Option<Object>, example = json!({"expression": "0 0 * * *", "timezone": "UTC"}))]
    pub configuration: Option<Value>,

    /// Remote tenant identifier for external system triggers
    #[schema(example = "remote-tenant-789")]
    pub remote_tenant_id: Option<String>,

    /// Whether only a single instance of this trigger should run at a time
    #[schema(example = false)]
    #[serde(default = "default_single_instance")]
    pub single_instance: bool,
}

/// Request payload for updating an invocation trigger
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct UpdateInvocationTriggerRequest {
    /// Reference to the scenario to be invoked
    #[schema(example = "scenario-456")]
    pub scenario_id: String,

    /// Type of trigger
    #[schema(example = "CRON")]
    pub trigger_type: TriggerType,

    /// Whether the trigger is currently active
    #[schema(example = true)]
    pub active: bool,

    /// Trigger-specific configuration in JSON format
    #[schema(value_type = Option<Object>, example = json!({"expression": "0 0 * * *", "timezone": "UTC"}))]
    pub configuration: Option<Value>,

    /// Remote tenant identifier for external system triggers
    #[schema(example = "remote-tenant-789")]
    pub remote_tenant_id: Option<String>,

    /// Whether only a single instance of this trigger should run at a time
    #[schema(example = false)]
    pub single_instance: bool,
}

// ============================================================================
// Response DTO
// ============================================================================

/// Trigger response with computed fields (e.g. webhook_url for Channel triggers).
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct InvocationTriggerResponse {
    /// The trigger data.
    #[serde(flatten)]
    pub trigger: InvocationTrigger,

    /// Webhook URL for Channel triggers (computed from WEBHOOK_BASE_URL).
    /// Null for non-Channel triggers or when WEBHOOK_BASE_URL is not set.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub webhook_url: Option<String>,
}

impl InvocationTriggerResponse {
    /// Enrich a trigger with a computed webhook_url if applicable.
    pub fn from_trigger(trigger: InvocationTrigger, tenant_id: &str) -> Self {
        let webhook_url = compute_webhook_url(&trigger, tenant_id);
        Self {
            trigger,
            webhook_url,
        }
    }
}

/// Compute the webhook URL for a Channel trigger.
/// Returns None for non-Channel triggers or if WEBHOOK_BASE_URL is not set.
fn compute_webhook_url(trigger: &InvocationTrigger, tenant_id: &str) -> Option<String> {
    if trigger.trigger_type != TriggerType::Channel {
        return None;
    }

    let base_url = std::env::var("WEBHOOK_BASE_URL")
        .ok()?
        .trim_end_matches('/')
        .to_string();

    let config = trigger.configuration.as_ref()?;
    let connection_id = config.get("connection_id")?.as_str()?;

    // Determine platform from connection_id by looking up integration_id.
    // Since we don't have DB access here, derive from the trigger config
    // or default to a generic path. The platform is stored in config
    // when the webhook is registered.
    let platform = config
        .get("platform")
        .and_then(|p| p.as_str())
        .unwrap_or("channel");

    Some(format!(
        "{}/api/events/{}/webhook/{}/{}",
        base_url, tenant_id, platform, connection_id
    ))
}

// ============================================================================
// Helper Functions
// ============================================================================

fn default_active() -> bool {
    true
}

fn default_single_instance() -> bool {
    false
}
