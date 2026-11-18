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

    /// Reference to the workflow to be invoked
    #[schema(example = "workflow-456")]
    pub workflow_id: String,

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
    /// Reference to the workflow to be invoked
    #[schema(example = "workflow-456")]
    pub workflow_id: String,

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
    /// Reference to the workflow to be invoked
    #[schema(example = "workflow-456")]
    pub workflow_id: String,

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
// Helper Functions
// ============================================================================

fn default_active() -> bool {
    true
}

fn default_single_instance() -> bool {
    false
}
