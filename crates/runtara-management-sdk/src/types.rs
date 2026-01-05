// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! High-level types for the management SDK.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Instance status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstanceStatus {
    /// Status unknown.
    Unknown,
    /// Instance is queued, not yet started.
    Pending,
    /// Instance is currently executing.
    Running,
    /// Instance is sleeping/waiting for wake.
    Suspended,
    /// Instance finished successfully.
    Completed,
    /// Instance finished with error.
    Failed,
    /// Instance was cancelled.
    Cancelled,
}

impl InstanceStatus {
    /// Check if this is a terminal status.
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            InstanceStatus::Completed | InstanceStatus::Failed | InstanceStatus::Cancelled
        )
    }
}

impl From<i32> for InstanceStatus {
    fn from(value: i32) -> Self {
        match value {
            1 => InstanceStatus::Pending,
            2 => InstanceStatus::Running,
            3 => InstanceStatus::Suspended,
            4 => InstanceStatus::Completed,
            5 => InstanceStatus::Failed,
            6 => InstanceStatus::Cancelled,
            _ => InstanceStatus::Unknown,
        }
    }
}

impl From<InstanceStatus> for i32 {
    fn from(status: InstanceStatus) -> Self {
        match status {
            InstanceStatus::Unknown => 0,
            InstanceStatus::Pending => 1,
            InstanceStatus::Running => 2,
            InstanceStatus::Suspended => 3,
            InstanceStatus::Completed => 4,
            InstanceStatus::Failed => 5,
            InstanceStatus::Cancelled => 6,
        }
    }
}

/// Signal type for controlling instances.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SignalType {
    /// Cancel execution.
    Cancel,
    /// Pause execution (checkpoint and wait).
    Pause,
    /// Resume paused execution.
    Resume,
}

impl From<SignalType> for i32 {
    fn from(signal: SignalType) -> Self {
        match signal {
            SignalType::Cancel => 0,
            SignalType::Pause => 1,
            SignalType::Resume => 2,
        }
    }
}

/// Health status of runtara-core.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthStatus {
    /// Whether the server is healthy.
    pub healthy: bool,
    /// Server version.
    pub version: String,
    /// Uptime in milliseconds.
    pub uptime_ms: i64,
    /// Number of active instances.
    pub active_instances: u32,
}

/// Instance status response with full details.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstanceInfo {
    // Identity
    /// Instance ID.
    pub instance_id: String,
    /// Image UUID used for this execution.
    pub image_id: String,
    /// Human-readable image name (format: {scenario_id}:{version}).
    pub image_name: String,
    /// Tenant/org that owns this instance.
    pub tenant_id: String,

    // Status
    /// Current status.
    pub status: InstanceStatus,
    /// Last checkpoint ID (if any).
    pub checkpoint_id: Option<String>,

    // Timing
    /// When the instance was created/queued.
    pub created_at: DateTime<Utc>,
    /// When the instance started executing (if started).
    pub started_at: Option<DateTime<Utc>>,
    /// When the instance finished (if finished).
    pub finished_at: Option<DateTime<Utc>>,
    /// Last heartbeat timestamp from the executor.
    pub heartbeat_at: Option<DateTime<Utc>>,

    // Execution data
    /// Input data provided when starting the instance.
    pub input: Option<serde_json::Value>,
    /// Output data (if completed).
    pub output: Option<serde_json::Value>,
    /// Error message (if failed).
    pub error: Option<String>,
    /// Raw stderr output from the container (for debugging/logging).
    /// This is separate from `error` to allow product to decide whether to show it to users.
    pub stderr: Option<String>,

    // Retry tracking
    /// Current retry attempt number.
    pub retry_count: u32,
    /// Maximum allowed retries configured for this instance.
    pub max_retries: u32,

    // Execution metrics (available for terminal states)
    /// Peak memory usage during execution (in bytes).
    pub memory_peak_bytes: Option<u64>,
    /// Total CPU time consumed during execution (in microseconds).
    pub cpu_usage_usec: Option<u64>,
}

/// Summary of an instance (used in list results).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstanceSummary {
    /// Instance ID.
    pub instance_id: String,
    /// Tenant ID.
    pub tenant_id: String,
    /// Image ID that this instance was created from.
    pub image_id: String,
    /// Current status.
    pub status: InstanceStatus,
    /// When the instance was created.
    pub created_at: DateTime<Utc>,
    /// When the instance started executing.
    pub started_at: Option<DateTime<Utc>>,
    /// When the instance finished (completed, failed, or cancelled).
    pub finished_at: Option<DateTime<Utc>>,
    /// Whether the instance has an error.
    pub has_error: bool,
}

/// Result of listing instances.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListInstancesResult {
    /// List of instances.
    pub instances: Vec<InstanceSummary>,
    /// Total count (for pagination).
    pub total_count: u32,
}

/// Options for starting an instance.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StartInstanceOptions {
    /// Image ID to launch.
    pub image_id: String,
    /// Tenant ID.
    pub tenant_id: String,
    /// Optional custom instance ID.
    pub instance_id: Option<String>,
    /// Input data (JSON).
    pub input: Option<serde_json::Value>,
    /// Execution timeout in seconds.
    pub timeout_seconds: Option<u32>,
    /// Custom environment variables (override system vars).
    pub env: std::collections::HashMap<String, String>,
}

impl StartInstanceOptions {
    /// Create new options with required fields.
    pub fn new(image_id: impl Into<String>, tenant_id: impl Into<String>) -> Self {
        Self {
            image_id: image_id.into(),
            tenant_id: tenant_id.into(),
            ..Default::default()
        }
    }

    /// Set a custom instance ID.
    pub fn with_instance_id(mut self, id: impl Into<String>) -> Self {
        self.instance_id = Some(id.into());
        self
    }

    /// Set the input data.
    pub fn with_input(mut self, input: serde_json::Value) -> Self {
        self.input = Some(input);
        self
    }

    /// Set the timeout.
    pub fn with_timeout(mut self, seconds: u32) -> Self {
        self.timeout_seconds = Some(seconds);
        self
    }

    /// Set custom environment variables (override system vars).
    pub fn with_env(mut self, env: std::collections::HashMap<String, String>) -> Self {
        self.env = env;
        self
    }

    /// Add a single environment variable.
    pub fn with_env_var(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.insert(key.into(), value.into());
        self
    }
}

/// Result of starting an instance.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StartInstanceResult {
    /// Whether the start was successful.
    pub success: bool,
    /// Instance ID (if successful).
    pub instance_id: String,
    /// Error message (if failed).
    pub error: Option<String>,
}

/// Options for stopping an instance.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StopInstanceOptions {
    /// Instance ID to stop.
    pub instance_id: String,
    /// Grace period in seconds before force kill.
    pub grace_period_seconds: u32,
    /// Reason for stopping.
    pub reason: String,
}

impl StopInstanceOptions {
    /// Create new options with required fields.
    pub fn new(instance_id: impl Into<String>) -> Self {
        Self {
            instance_id: instance_id.into(),
            grace_period_seconds: 5,
            reason: String::new(),
        }
    }

    /// Set the grace period.
    pub fn with_grace_period(mut self, seconds: u32) -> Self {
        self.grace_period_seconds = seconds;
        self
    }

    /// Set the stop reason.
    pub fn with_reason(mut self, reason: impl Into<String>) -> Self {
        self.reason = reason.into();
        self
    }
}

/// Sort order for listing instances.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ListInstancesOrder {
    /// Newest instances first (default).
    CreatedAtDesc,
    /// Oldest instances first.
    CreatedAtAsc,
    /// Most recently finished first.
    FinishedAtDesc,
    /// Earliest finished first.
    FinishedAtAsc,
}

impl ListInstancesOrder {
    /// Convert to string value for proto.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::CreatedAtDesc => "created_at_desc",
            Self::CreatedAtAsc => "created_at_asc",
            Self::FinishedAtDesc => "finished_at_desc",
            Self::FinishedAtAsc => "finished_at_asc",
        }
    }
}

/// Options for listing instances.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ListInstancesOptions {
    /// Filter by tenant ID.
    pub tenant_id: Option<String>,
    /// Filter by status.
    pub status: Option<InstanceStatus>,
    /// Filter by image ID (exact UUID match).
    pub image_id: Option<String>,
    /// Filter by image name prefix (e.g., "scenario_id:" matches "scenario_id:1", "scenario_id:2").
    pub image_name_prefix: Option<String>,
    /// Filter by created_at >= value.
    pub created_after: Option<DateTime<Utc>>,
    /// Filter by created_at < value.
    pub created_before: Option<DateTime<Utc>>,
    /// Filter by finished_at >= value.
    pub finished_after: Option<DateTime<Utc>>,
    /// Filter by finished_at < value.
    pub finished_before: Option<DateTime<Utc>>,
    /// Sort order.
    pub order_by: Option<ListInstancesOrder>,
    /// Maximum results to return.
    pub limit: u32,
    /// Pagination offset.
    pub offset: u32,
}

impl ListInstancesOptions {
    /// Create new options.
    pub fn new() -> Self {
        Self {
            limit: 100,
            ..Default::default()
        }
    }

    /// Filter by tenant ID.
    pub fn with_tenant_id(mut self, tenant_id: impl Into<String>) -> Self {
        self.tenant_id = Some(tenant_id.into());
        self
    }

    /// Filter by status.
    pub fn with_status(mut self, status: InstanceStatus) -> Self {
        self.status = Some(status);
        self
    }

    /// Filter by image ID (exact UUID match).
    pub fn with_image_id(mut self, image_id: impl Into<String>) -> Self {
        self.image_id = Some(image_id.into());
        self
    }

    /// Filter by image name prefix (e.g., "scenario_id:" matches "scenario_id:1", "scenario_id:2").
    pub fn with_image_name_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.image_name_prefix = Some(prefix.into());
        self
    }

    /// Filter by created_at >= value.
    pub fn with_created_after(mut self, created_after: DateTime<Utc>) -> Self {
        self.created_after = Some(created_after);
        self
    }

    /// Filter by created_at < value.
    pub fn with_created_before(mut self, created_before: DateTime<Utc>) -> Self {
        self.created_before = Some(created_before);
        self
    }

    /// Filter by finished_at >= value.
    pub fn with_finished_after(mut self, finished_after: DateTime<Utc>) -> Self {
        self.finished_after = Some(finished_after);
        self
    }

    /// Filter by finished_at < value.
    pub fn with_finished_before(mut self, finished_before: DateTime<Utc>) -> Self {
        self.finished_before = Some(finished_before);
        self
    }

    /// Set the sort order.
    pub fn with_order_by(mut self, order_by: ListInstancesOrder) -> Self {
        self.order_by = Some(order_by);
        self
    }

    /// Set the limit.
    pub fn with_limit(mut self, limit: u32) -> Self {
        self.limit = limit;
        self
    }

    /// Set the offset.
    pub fn with_offset(mut self, offset: u32) -> Self {
        self.offset = offset;
        self
    }
}

/// Runner type for images.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunnerType {
    /// OCI container runner (default).
    #[default]
    Oci,
    /// Native process runner.
    Native,
    /// WebAssembly runner.
    Wasm,
}

impl From<RunnerType> for i32 {
    fn from(runner: RunnerType) -> Self {
        match runner {
            RunnerType::Oci => 0,
            RunnerType::Native => 1,
            RunnerType::Wasm => 2,
        }
    }
}

/// Options for registering an image.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RegisterImageOptions {
    /// Tenant ID that owns this image.
    pub tenant_id: String,
    /// Human-readable name (unique per tenant).
    pub name: String,
    /// Optional description.
    pub description: Option<String>,
    /// Compiled binary content.
    pub binary: Vec<u8>,
    /// Type of runner to use.
    pub runner_type: RunnerType,
    /// Optional metadata (JSON).
    pub metadata: Option<serde_json::Value>,
}

impl RegisterImageOptions {
    /// Create new options with required fields.
    pub fn new(tenant_id: impl Into<String>, name: impl Into<String>, binary: Vec<u8>) -> Self {
        Self {
            tenant_id: tenant_id.into(),
            name: name.into(),
            binary,
            ..Default::default()
        }
    }

    /// Set the description.
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Set the runner type.
    pub fn with_runner_type(mut self, runner_type: RunnerType) -> Self {
        self.runner_type = runner_type;
        self
    }

    /// Set metadata.
    pub fn with_metadata(mut self, metadata: serde_json::Value) -> Self {
        self.metadata = Some(metadata);
        self
    }
}

/// Result of registering an image.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegisterImageResult {
    /// Whether the registration was successful.
    pub success: bool,
    /// Assigned image ID (if successful).
    pub image_id: String,
    /// Error message (if failed).
    pub error: Option<String>,
}

/// Options for streaming image registration.
///
/// Use this for large binaries that shouldn't be held entirely in memory.
#[derive(Debug, Clone)]
pub struct RegisterImageStreamOptions {
    /// Tenant ID that owns this image.
    pub tenant_id: String,
    /// Human-readable name (unique per tenant).
    pub name: String,
    /// Optional description.
    pub description: Option<String>,
    /// Size of the binary in bytes.
    pub binary_size: u64,
    /// Type of runner to use.
    pub runner_type: RunnerType,
    /// Optional metadata (JSON).
    pub metadata: Option<serde_json::Value>,
    /// Optional SHA256 checksum for verification.
    pub sha256: Option<String>,
}

impl RegisterImageStreamOptions {
    /// Create new streaming options with required fields.
    pub fn new(tenant_id: impl Into<String>, name: impl Into<String>, binary_size: u64) -> Self {
        Self {
            tenant_id: tenant_id.into(),
            name: name.into(),
            binary_size,
            description: None,
            runner_type: RunnerType::default(),
            metadata: None,
            sha256: None,
        }
    }

    /// Set the description.
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Set the runner type.
    pub fn with_runner_type(mut self, runner_type: RunnerType) -> Self {
        self.runner_type = runner_type;
        self
    }

    /// Set metadata.
    pub fn with_metadata(mut self, metadata: serde_json::Value) -> Self {
        self.metadata = Some(metadata);
        self
    }

    /// Set SHA256 checksum for verification.
    pub fn with_sha256(mut self, sha256: impl Into<String>) -> Self {
        self.sha256 = Some(sha256.into());
        self
    }
}

/// Summary of an image (used in list results).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageSummary {
    /// Image ID.
    pub image_id: String,
    /// Tenant ID.
    pub tenant_id: String,
    /// Human-readable name.
    pub name: String,
    /// Description.
    pub description: Option<String>,
    /// Runner type.
    pub runner_type: RunnerType,
    /// When the image was created.
    pub created_at: DateTime<Utc>,
}

/// Options for listing images.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ListImagesOptions {
    /// Filter by tenant ID.
    pub tenant_id: Option<String>,
    /// Maximum results to return.
    pub limit: u32,
    /// Pagination offset.
    pub offset: u32,
}

impl ListImagesOptions {
    /// Create new options.
    pub fn new() -> Self {
        Self {
            limit: 100,
            ..Default::default()
        }
    }

    /// Filter by tenant ID.
    pub fn with_tenant_id(mut self, tenant_id: impl Into<String>) -> Self {
        self.tenant_id = Some(tenant_id.into());
        self
    }

    /// Set the limit.
    pub fn with_limit(mut self, limit: u32) -> Self {
        self.limit = limit;
        self
    }

    /// Set the offset.
    pub fn with_offset(mut self, offset: u32) -> Self {
        self.offset = offset;
        self
    }
}

/// Result of listing images.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListImagesResult {
    /// List of images.
    pub images: Vec<ImageSummary>,
    /// Total count (for pagination).
    pub total_count: u32,
}

// ============================================================================
// Agent Testing Types
// ============================================================================

/// Options for testing a capability.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TestCapabilityOptions {
    /// Tenant ID for isolation.
    pub tenant_id: String,
    /// Agent module name (e.g., "http", "utils", "transform").
    pub agent_id: String,
    /// Capability ID (e.g., "http-request", "random-double").
    pub capability_id: String,
    /// Capability input (JSON).
    pub input: serde_json::Value,
    /// Optional connection credentials.
    pub connection: Option<serde_json::Value>,
    /// Execution timeout in milliseconds.
    pub timeout_ms: Option<u32>,
}

impl TestCapabilityOptions {
    /// Create new options with required fields.
    pub fn new(
        tenant_id: impl Into<String>,
        agent_id: impl Into<String>,
        capability_id: impl Into<String>,
        input: serde_json::Value,
    ) -> Self {
        Self {
            tenant_id: tenant_id.into(),
            agent_id: agent_id.into(),
            capability_id: capability_id.into(),
            input,
            connection: None,
            timeout_ms: None,
        }
    }

    /// Set connection credentials.
    pub fn with_connection(mut self, connection: serde_json::Value) -> Self {
        self.connection = Some(connection);
        self
    }

    /// Set execution timeout in milliseconds.
    pub fn with_timeout(mut self, timeout_ms: u32) -> Self {
        self.timeout_ms = Some(timeout_ms);
        self
    }
}

/// Result of testing a capability.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestCapabilityResult {
    /// Whether the test succeeded.
    pub success: bool,
    /// Output value on success (JSON).
    pub output: Option<serde_json::Value>,
    /// Error message on failure.
    pub error: Option<String>,
    /// Execution time in milliseconds.
    pub execution_time_ms: u64,
}

/// Information about an agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentInfo {
    /// Agent module name (e.g., "http", "utils").
    pub id: String,
    /// Display name.
    pub name: String,
    /// Agent description.
    pub description: String,
    /// Whether the agent has side effects.
    pub has_side_effects: bool,
    /// Whether the agent supports connections.
    pub supports_connections: bool,
    /// Supported integration IDs for connections.
    pub integration_ids: Vec<String>,
    /// List of capabilities.
    pub capabilities: Vec<CapabilityInfo>,
}

/// Information about a capability.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilityInfo {
    /// Capability ID.
    pub id: String,
    /// Capability name.
    pub name: String,
    /// Display name.
    pub display_name: Option<String>,
    /// Description.
    pub description: Option<String>,
    /// Whether the capability has side effects.
    pub has_side_effects: bool,
    /// Whether the capability is idempotent.
    pub is_idempotent: bool,
    /// Whether the capability is rate limited.
    pub rate_limited: bool,
}

/// Information about a capability input field.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilityField {
    /// Field name.
    pub name: String,
    /// Field type.
    pub field_type: String,
    /// Display name.
    pub display_name: Option<String>,
    /// Description.
    pub description: Option<String>,
    /// Whether the field is required.
    pub required: bool,
    /// Default value.
    pub default_value: Option<serde_json::Value>,
    /// Example value.
    pub example: Option<String>,
}

// ============================================================================
// Checkpoint Types
// ============================================================================

/// Options for listing checkpoints.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ListCheckpointsOptions {
    /// Filter by specific checkpoint_id.
    pub checkpoint_id: Option<String>,
    /// Maximum results to return.
    pub limit: Option<u32>,
    /// Pagination offset.
    pub offset: Option<u32>,
    /// Filter checkpoints created after this time.
    pub created_after: Option<DateTime<Utc>>,
    /// Filter checkpoints created before this time.
    pub created_before: Option<DateTime<Utc>>,
}

impl ListCheckpointsOptions {
    /// Create new options with defaults.
    pub fn new() -> Self {
        Self::default()
    }

    /// Filter by checkpoint ID.
    pub fn with_checkpoint_id(mut self, checkpoint_id: impl Into<String>) -> Self {
        self.checkpoint_id = Some(checkpoint_id.into());
        self
    }

    /// Set the limit.
    pub fn with_limit(mut self, limit: u32) -> Self {
        self.limit = Some(limit);
        self
    }

    /// Set the offset.
    pub fn with_offset(mut self, offset: u32) -> Self {
        self.offset = Some(offset);
        self
    }

    /// Filter checkpoints created after this time.
    pub fn with_created_after(mut self, created_after: DateTime<Utc>) -> Self {
        self.created_after = Some(created_after);
        self
    }

    /// Filter checkpoints created before this time.
    pub fn with_created_before(mut self, created_before: DateTime<Utc>) -> Self {
        self.created_before = Some(created_before);
        self
    }
}

/// Summary of a checkpoint (for list results).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckpointSummary {
    /// Checkpoint ID.
    pub checkpoint_id: String,
    /// Instance ID this checkpoint belongs to.
    pub instance_id: String,
    /// When the checkpoint was created.
    pub created_at: DateTime<Utc>,
    /// Size of checkpoint data in bytes (for UI display).
    pub data_size_bytes: u64,
}

/// Result of listing checkpoints.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListCheckpointsResult {
    /// List of checkpoint summaries.
    pub checkpoints: Vec<CheckpointSummary>,
    /// Total count (for pagination).
    pub total_count: u32,
    /// Limit used in query.
    pub limit: u32,
    /// Offset used in query.
    pub offset: u32,
}

/// Full checkpoint with data.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Checkpoint {
    /// Checkpoint ID.
    pub checkpoint_id: String,
    /// Instance ID this checkpoint belongs to.
    pub instance_id: String,
    /// When the checkpoint was created.
    pub created_at: DateTime<Utc>,
    /// The checkpoint state data (parsed JSON).
    pub data: serde_json::Value,
}

// ============================================================================
// Event Types
// ============================================================================

/// Options for listing events.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ListEventsOptions {
    /// Filter by event type (e.g., "custom", "started", "completed").
    pub event_type: Option<String>,
    /// Filter by subtype (e.g., "step_debug_start", "step_debug_end", "workflow_log").
    pub subtype: Option<String>,
    /// Maximum results to return.
    pub limit: Option<u32>,
    /// Pagination offset.
    pub offset: Option<u32>,
    /// Filter events created after this time.
    pub created_after: Option<DateTime<Utc>>,
    /// Filter events created before this time.
    pub created_before: Option<DateTime<Utc>>,
    /// Full-text search in JSON payload content.
    pub payload_contains: Option<String>,
}

impl ListEventsOptions {
    /// Create new options with defaults.
    pub fn new() -> Self {
        Self::default()
    }

    /// Filter by event type.
    pub fn with_event_type(mut self, event_type: impl Into<String>) -> Self {
        self.event_type = Some(event_type.into());
        self
    }

    /// Filter by subtype.
    pub fn with_subtype(mut self, subtype: impl Into<String>) -> Self {
        self.subtype = Some(subtype.into());
        self
    }

    /// Set the limit.
    pub fn with_limit(mut self, limit: u32) -> Self {
        self.limit = Some(limit);
        self
    }

    /// Set the offset.
    pub fn with_offset(mut self, offset: u32) -> Self {
        self.offset = Some(offset);
        self
    }

    /// Filter events created after this time.
    pub fn with_created_after(mut self, created_after: DateTime<Utc>) -> Self {
        self.created_after = Some(created_after);
        self
    }

    /// Filter events created before this time.
    pub fn with_created_before(mut self, created_before: DateTime<Utc>) -> Self {
        self.created_before = Some(created_before);
        self
    }

    /// Full-text search in JSON payload.
    pub fn with_payload_contains(mut self, search: impl Into<String>) -> Self {
        self.payload_contains = Some(search.into());
        self
    }
}

/// Summary of an event (for list results).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventSummary {
    /// Event ID.
    pub id: i64,
    /// Instance ID this event belongs to.
    pub instance_id: String,
    /// Event type (e.g., "custom", "started", "completed").
    pub event_type: String,
    /// Associated checkpoint ID if applicable.
    pub checkpoint_id: Option<String>,
    /// Event payload as JSON (parsed from bytes).
    pub payload: Option<serde_json::Value>,
    /// When the event was created.
    pub created_at: DateTime<Utc>,
    /// Event subtype (e.g., "step_debug_start", "workflow_log").
    pub subtype: Option<String>,
}

/// Result of listing events.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListEventsResult {
    /// List of event summaries.
    pub events: Vec<EventSummary>,
    /// Total count (for pagination).
    pub total_count: u32,
    /// Limit used in query.
    pub limit: u32,
    /// Offset used in query.
    pub offset: u32,
}

// ============================================================================
// Tenant Metrics
// ============================================================================

/// Granularity for metrics aggregation buckets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MetricsGranularity {
    /// Hourly buckets (default).
    #[default]
    Hourly,
    /// Daily buckets.
    Daily,
}

impl From<MetricsGranularity> for i32 {
    fn from(granularity: MetricsGranularity) -> Self {
        match granularity {
            MetricsGranularity::Hourly => 0,
            MetricsGranularity::Daily => 1,
        }
    }
}

impl From<i32> for MetricsGranularity {
    fn from(value: i32) -> Self {
        match value {
            1 => MetricsGranularity::Daily,
            _ => MetricsGranularity::Hourly,
        }
    }
}

/// Options for getting tenant metrics.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GetTenantMetricsOptions {
    /// Tenant ID (required).
    pub tenant_id: String,
    /// Start of time range (default: 24 hours ago).
    pub start_time: Option<DateTime<Utc>>,
    /// End of time range (default: now).
    pub end_time: Option<DateTime<Utc>>,
    /// Bucket granularity (default: hourly).
    pub granularity: Option<MetricsGranularity>,
}

impl GetTenantMetricsOptions {
    /// Create new options with required tenant ID.
    pub fn new(tenant_id: impl Into<String>) -> Self {
        Self {
            tenant_id: tenant_id.into(),
            ..Default::default()
        }
    }

    /// Set the start time.
    pub fn with_start_time(mut self, start_time: DateTime<Utc>) -> Self {
        self.start_time = Some(start_time);
        self
    }

    /// Set the end time.
    pub fn with_end_time(mut self, end_time: DateTime<Utc>) -> Self {
        self.end_time = Some(end_time);
        self
    }

    /// Set the granularity.
    pub fn with_granularity(mut self, granularity: MetricsGranularity) -> Self {
        self.granularity = Some(granularity);
        self
    }
}

/// Result of tenant metrics aggregation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TenantMetricsResult {
    /// Tenant ID.
    pub tenant_id: String,
    /// Start of time range.
    pub start_time: DateTime<Utc>,
    /// End of time range.
    pub end_time: DateTime<Utc>,
    /// Bucket granularity used.
    pub granularity: MetricsGranularity,
    /// Time-bucketed metrics.
    pub buckets: Vec<MetricsBucket>,
}

/// A single time bucket of aggregated metrics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricsBucket {
    /// Start time of this bucket (UTC).
    pub bucket_time: DateTime<Utc>,

    // Counts
    /// Total number of invocations in this bucket.
    pub invocation_count: i64,
    /// Number of successful completions.
    pub success_count: i64,
    /// Number of failures.
    pub failure_count: i64,
    /// Number of cancellations.
    pub cancelled_count: i64,

    // Duration stats (seconds)
    /// Average execution duration in seconds.
    pub avg_duration_seconds: Option<f64>,
    /// Minimum execution duration in seconds.
    pub min_duration_seconds: Option<f64>,
    /// Maximum execution duration in seconds.
    pub max_duration_seconds: Option<f64>,

    // Memory stats (bytes)
    /// Average peak memory usage in bytes.
    pub avg_memory_bytes: Option<i64>,
    /// Maximum peak memory usage in bytes.
    pub max_memory_bytes: Option<i64>,

    // Calculated
    /// Success rate as percentage (0-100).
    pub success_rate_percent: Option<f64>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ========================================================================
    // InstanceStatus tests
    // ========================================================================

    #[test]
    fn test_instance_status_is_terminal() {
        assert!(!InstanceStatus::Unknown.is_terminal());
        assert!(!InstanceStatus::Pending.is_terminal());
        assert!(!InstanceStatus::Running.is_terminal());
        assert!(!InstanceStatus::Suspended.is_terminal());
        assert!(InstanceStatus::Completed.is_terminal());
        assert!(InstanceStatus::Failed.is_terminal());
        assert!(InstanceStatus::Cancelled.is_terminal());
    }

    #[test]
    fn test_instance_status_from_i32() {
        assert_eq!(InstanceStatus::from(0), InstanceStatus::Unknown);
        assert_eq!(InstanceStatus::from(1), InstanceStatus::Pending);
        assert_eq!(InstanceStatus::from(2), InstanceStatus::Running);
        assert_eq!(InstanceStatus::from(3), InstanceStatus::Suspended);
        assert_eq!(InstanceStatus::from(4), InstanceStatus::Completed);
        assert_eq!(InstanceStatus::from(5), InstanceStatus::Failed);
        assert_eq!(InstanceStatus::from(6), InstanceStatus::Cancelled);
        assert_eq!(InstanceStatus::from(99), InstanceStatus::Unknown);
    }

    #[test]
    fn test_instance_status_to_i32() {
        assert_eq!(i32::from(InstanceStatus::Unknown), 0);
        assert_eq!(i32::from(InstanceStatus::Pending), 1);
        assert_eq!(i32::from(InstanceStatus::Running), 2);
        assert_eq!(i32::from(InstanceStatus::Suspended), 3);
        assert_eq!(i32::from(InstanceStatus::Completed), 4);
        assert_eq!(i32::from(InstanceStatus::Failed), 5);
        assert_eq!(i32::from(InstanceStatus::Cancelled), 6);
    }

    #[test]
    fn test_instance_status_serde() {
        let status = InstanceStatus::Running;
        let json = serde_json::to_string(&status).unwrap();
        assert_eq!(json, "\"running\"");

        let deserialized: InstanceStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, InstanceStatus::Running);
    }

    // ========================================================================
    // SignalType tests
    // ========================================================================

    #[test]
    fn test_signal_type_to_i32() {
        assert_eq!(i32::from(SignalType::Cancel), 0);
        assert_eq!(i32::from(SignalType::Pause), 1);
        assert_eq!(i32::from(SignalType::Resume), 2);
    }

    #[test]
    fn test_signal_type_serde() {
        let signal = SignalType::Pause;
        let json = serde_json::to_string(&signal).unwrap();
        assert_eq!(json, "\"pause\"");

        let deserialized: SignalType = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, SignalType::Pause);
    }

    // ========================================================================
    // RunnerType tests
    // ========================================================================

    #[test]
    fn test_runner_type_default() {
        assert_eq!(RunnerType::default(), RunnerType::Oci);
    }

    #[test]
    fn test_runner_type_to_i32() {
        assert_eq!(i32::from(RunnerType::Oci), 0);
        assert_eq!(i32::from(RunnerType::Native), 1);
        assert_eq!(i32::from(RunnerType::Wasm), 2);
    }

    #[test]
    fn test_runner_type_serde() {
        let runner = RunnerType::Native;
        let json = serde_json::to_string(&runner).unwrap();
        assert_eq!(json, "\"native\"");

        let deserialized: RunnerType = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, RunnerType::Native);
    }

    // ========================================================================
    // StartInstanceOptions tests
    // ========================================================================

    #[test]
    fn test_start_instance_options_builder() {
        let opts = StartInstanceOptions::new("image-123", "tenant-1")
            .with_instance_id("custom-id")
            .with_input(json!({"key": "value"}))
            .with_timeout(60);

        assert_eq!(opts.image_id, "image-123");
        assert_eq!(opts.tenant_id, "tenant-1");
        assert_eq!(opts.instance_id, Some("custom-id".to_string()));
        assert_eq!(opts.input, Some(json!({"key": "value"})));
        assert_eq!(opts.timeout_seconds, Some(60));
    }

    #[test]
    fn test_start_instance_options_defaults() {
        let opts = StartInstanceOptions::new("image-123", "tenant-1");

        assert_eq!(opts.image_id, "image-123");
        assert_eq!(opts.tenant_id, "tenant-1");
        assert!(opts.instance_id.is_none());
        assert!(opts.input.is_none());
        assert!(opts.timeout_seconds.is_none());
    }

    // ========================================================================
    // StopInstanceOptions tests
    // ========================================================================

    #[test]
    fn test_stop_instance_options_builder() {
        let opts = StopInstanceOptions::new("instance-123")
            .with_grace_period(10)
            .with_reason("User requested stop");

        assert_eq!(opts.instance_id, "instance-123");
        assert_eq!(opts.grace_period_seconds, 10);
        assert_eq!(opts.reason, "User requested stop");
    }

    #[test]
    fn test_stop_instance_options_defaults() {
        let opts = StopInstanceOptions::new("instance-123");

        assert_eq!(opts.instance_id, "instance-123");
        assert_eq!(opts.grace_period_seconds, 5);
        assert_eq!(opts.reason, "");
    }

    // ========================================================================
    // ListInstancesOptions tests
    // ========================================================================

    #[test]
    fn test_list_instances_options_builder() {
        let opts = ListInstancesOptions::new()
            .with_tenant_id("tenant-1")
            .with_status(InstanceStatus::Running)
            .with_limit(50)
            .with_offset(10);

        assert_eq!(opts.tenant_id, Some("tenant-1".to_string()));
        assert_eq!(opts.status, Some(InstanceStatus::Running));
        assert_eq!(opts.limit, 50);
        assert_eq!(opts.offset, 10);
    }

    #[test]
    fn test_list_instances_options_defaults() {
        let opts = ListInstancesOptions::new();

        assert!(opts.tenant_id.is_none());
        assert!(opts.status.is_none());
        assert!(opts.image_id.is_none());
        assert!(opts.created_after.is_none());
        assert!(opts.created_before.is_none());
        assert!(opts.finished_after.is_none());
        assert!(opts.finished_before.is_none());
        assert!(opts.order_by.is_none());
        assert_eq!(opts.limit, 100);
        assert_eq!(opts.offset, 0);
    }

    #[test]
    fn test_list_instances_options_with_image_id() {
        let opts = ListInstancesOptions::new().with_image_id("image-123");

        assert_eq!(opts.image_id, Some("image-123".to_string()));
    }

    #[test]
    fn test_list_instances_options_with_date_filters() {
        use chrono::TimeZone;

        let created_after = Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap();
        let created_before = Utc.with_ymd_and_hms(2024, 12, 31, 23, 59, 59).unwrap();
        let finished_after = Utc.with_ymd_and_hms(2024, 6, 1, 0, 0, 0).unwrap();
        let finished_before = Utc.with_ymd_and_hms(2024, 6, 30, 23, 59, 59).unwrap();

        let opts = ListInstancesOptions::new()
            .with_created_after(created_after)
            .with_created_before(created_before)
            .with_finished_after(finished_after)
            .with_finished_before(finished_before);

        assert_eq!(opts.created_after, Some(created_after));
        assert_eq!(opts.created_before, Some(created_before));
        assert_eq!(opts.finished_after, Some(finished_after));
        assert_eq!(opts.finished_before, Some(finished_before));
    }

    #[test]
    fn test_list_instances_options_with_order_by() {
        let opts = ListInstancesOptions::new().with_order_by(ListInstancesOrder::FinishedAtDesc);

        assert_eq!(opts.order_by, Some(ListInstancesOrder::FinishedAtDesc));
    }

    #[test]
    fn test_list_instances_order_as_str() {
        assert_eq!(
            ListInstancesOrder::CreatedAtDesc.as_str(),
            "created_at_desc"
        );
        assert_eq!(ListInstancesOrder::CreatedAtAsc.as_str(), "created_at_asc");
        assert_eq!(
            ListInstancesOrder::FinishedAtDesc.as_str(),
            "finished_at_desc"
        );
        assert_eq!(
            ListInstancesOrder::FinishedAtAsc.as_str(),
            "finished_at_asc"
        );
    }

    #[test]
    fn test_list_instances_order_serde() {
        let order = ListInstancesOrder::FinishedAtDesc;
        let json = serde_json::to_string(&order).unwrap();
        assert_eq!(json, "\"finished_at_desc\"");

        let deserialized: ListInstancesOrder = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, ListInstancesOrder::FinishedAtDesc);
    }

    // ========================================================================
    // RegisterImageOptions tests
    // ========================================================================

    #[test]
    fn test_register_image_options_builder() {
        let binary = vec![1, 2, 3, 4];
        let opts = RegisterImageOptions::new("tenant-1", "my-image", binary.clone())
            .with_description("Test image")
            .with_runner_type(RunnerType::Wasm)
            .with_metadata(json!({"version": "1.0"}));

        assert_eq!(opts.tenant_id, "tenant-1");
        assert_eq!(opts.name, "my-image");
        assert_eq!(opts.binary, binary);
        assert_eq!(opts.description, Some("Test image".to_string()));
        assert_eq!(opts.runner_type, RunnerType::Wasm);
        assert_eq!(opts.metadata, Some(json!({"version": "1.0"})));
    }

    #[test]
    fn test_register_image_options_defaults() {
        let opts = RegisterImageOptions::new("tenant-1", "my-image", vec![1, 2, 3]);

        assert!(opts.description.is_none());
        assert_eq!(opts.runner_type, RunnerType::Oci);
        assert!(opts.metadata.is_none());
    }

    // ========================================================================
    // RegisterImageStreamOptions tests
    // ========================================================================

    #[test]
    fn test_register_image_stream_options_builder() {
        let opts = RegisterImageStreamOptions::new("tenant-1", "my-image", 1024)
            .with_description("Streaming image")
            .with_runner_type(RunnerType::Native)
            .with_metadata(json!({"tag": "latest"}))
            .with_sha256("abc123");

        assert_eq!(opts.tenant_id, "tenant-1");
        assert_eq!(opts.name, "my-image");
        assert_eq!(opts.binary_size, 1024);
        assert_eq!(opts.description, Some("Streaming image".to_string()));
        assert_eq!(opts.runner_type, RunnerType::Native);
        assert_eq!(opts.metadata, Some(json!({"tag": "latest"})));
        assert_eq!(opts.sha256, Some("abc123".to_string()));
    }

    // ========================================================================
    // ListImagesOptions tests
    // ========================================================================

    #[test]
    fn test_list_images_options_builder() {
        let opts = ListImagesOptions::new()
            .with_tenant_id("tenant-1")
            .with_limit(25)
            .with_offset(5);

        assert_eq!(opts.tenant_id, Some("tenant-1".to_string()));
        assert_eq!(opts.limit, 25);
        assert_eq!(opts.offset, 5);
    }

    #[test]
    fn test_list_images_options_defaults() {
        let opts = ListImagesOptions::new();

        assert!(opts.tenant_id.is_none());
        assert_eq!(opts.limit, 100);
        assert_eq!(opts.offset, 0);
    }

    // ========================================================================
    // TestCapabilityOptions tests
    // ========================================================================

    #[test]
    fn test_capability_options_builder() {
        let opts = TestCapabilityOptions::new(
            "tenant-1",
            "http",
            "http-request",
            json!({
                "url": "/api/users",
                "method": "GET"
            }),
        )
        .with_connection(json!({
            "integration_id": "bearer",
            "parameters": {"base_url": "https://api.example.com"}
        }))
        .with_timeout(5000);

        assert_eq!(opts.tenant_id, "tenant-1");
        assert_eq!(opts.agent_id, "http");
        assert_eq!(opts.capability_id, "http-request");
        assert_eq!(opts.input["url"], "/api/users");
        assert_eq!(opts.input["method"], "GET");
        assert!(opts.connection.is_some());
        assert_eq!(
            opts.connection.as_ref().unwrap()["integration_id"],
            "bearer"
        );
        assert_eq!(opts.timeout_ms, Some(5000));
    }

    #[test]
    fn test_capability_options_defaults() {
        let opts = TestCapabilityOptions::new("tenant-1", "utils", "random-double", json!({}));

        assert_eq!(opts.tenant_id, "tenant-1");
        assert_eq!(opts.agent_id, "utils");
        assert_eq!(opts.capability_id, "random-double");
        assert!(opts.connection.is_none());
        assert!(opts.timeout_ms.is_none());
    }

    #[test]
    fn test_capability_options_serde() {
        let opts = TestCapabilityOptions::new(
            "tenant-1",
            "utils",
            "random-double",
            json!({"min": 0, "max": 100}),
        );

        let json = serde_json::to_string(&opts).unwrap();
        let deserialized: TestCapabilityOptions = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.tenant_id, "tenant-1");
        assert_eq!(deserialized.agent_id, "utils");
        assert_eq!(deserialized.capability_id, "random-double");
        assert_eq!(deserialized.input, json!({"min": 0, "max": 100}));
    }

    // ========================================================================
    // TestCapabilityResult tests
    // ========================================================================

    #[test]
    fn test_capability_result_success() {
        let result = TestCapabilityResult {
            success: true,
            output: Some(json!({"data": "test"})),
            error: None,
            execution_time_ms: 42,
        };

        assert!(result.success);
        assert_eq!(result.output, Some(json!({"data": "test"})));
        assert!(result.error.is_none());
        assert_eq!(result.execution_time_ms, 42);
    }

    #[test]
    fn test_capability_result_failure() {
        let result = TestCapabilityResult {
            success: false,
            output: None,
            error: Some("Connection refused".to_string()),
            execution_time_ms: 100,
        };

        assert!(!result.success);
        assert!(result.output.is_none());
        assert_eq!(result.error, Some("Connection refused".to_string()));
    }

    #[test]
    fn test_capability_result_serde() {
        let result = TestCapabilityResult {
            success: true,
            output: Some(json!(42.5)),
            error: None,
            execution_time_ms: 15,
        };

        let json = serde_json::to_string(&result).unwrap();
        let deserialized: TestCapabilityResult = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.success, result.success);
        assert_eq!(deserialized.output, result.output);
        assert_eq!(deserialized.execution_time_ms, result.execution_time_ms);
    }

    // ========================================================================
    // AgentInfo tests
    // ========================================================================

    #[test]
    fn test_agent_info_serde() {
        let agent = AgentInfo {
            id: "http".to_string(),
            name: "HTTP Agent".to_string(),
            description: "Makes HTTP requests".to_string(),
            has_side_effects: true,
            supports_connections: true,
            integration_ids: vec!["bearer".to_string(), "basic".to_string()],
            capabilities: vec![CapabilityInfo {
                id: "http-request".to_string(),
                name: "http-request".to_string(),
                display_name: Some("HTTP Request".to_string()),
                description: Some("Make an HTTP request".to_string()),
                has_side_effects: true,
                is_idempotent: false,
                rate_limited: true,
            }],
        };

        let json = serde_json::to_string(&agent).unwrap();
        let deserialized: AgentInfo = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.id, "http");
        assert_eq!(deserialized.integration_ids.len(), 2);
        assert_eq!(deserialized.capabilities.len(), 1);
        assert_eq!(deserialized.capabilities[0].id, "http-request");
    }

    // ========================================================================
    // CapabilityField tests
    // ========================================================================

    #[test]
    fn test_capability_field_serde() {
        let field = CapabilityField {
            name: "url".to_string(),
            field_type: "string".to_string(),
            display_name: Some("URL".to_string()),
            description: Some("The request URL".to_string()),
            required: true,
            default_value: None,
            example: Some("/api/users".to_string()),
        };

        let json = serde_json::to_string(&field).unwrap();
        let deserialized: CapabilityField = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.name, "url");
        assert_eq!(deserialized.field_type, "string");
        assert!(deserialized.required);
        assert_eq!(deserialized.example, Some("/api/users".to_string()));
    }

    #[test]
    fn test_capability_field_with_default() {
        let field = CapabilityField {
            name: "method".to_string(),
            field_type: "string".to_string(),
            display_name: None,
            description: None,
            required: false,
            default_value: Some(json!("GET")),
            example: None,
        };

        assert!(!field.required);
        assert_eq!(field.default_value, Some(json!("GET")));
    }

    // ========================================================================
    // InstanceInfo tests (with metrics)
    // ========================================================================

    #[test]
    fn test_instance_info_with_metrics() {
        let info = InstanceInfo {
            instance_id: "inst-123".to_string(),
            image_id: "img-456".to_string(),
            image_name: "my-workflow:v1".to_string(),
            tenant_id: "tenant-1".to_string(),
            status: InstanceStatus::Completed,
            checkpoint_id: None,
            created_at: Utc::now(),
            started_at: Some(Utc::now()),
            finished_at: Some(Utc::now()),
            heartbeat_at: None,
            input: None,
            output: Some(json!({"result": "success"})),
            error: None,
            stderr: None,
            retry_count: 0,
            max_retries: 3,
            memory_peak_bytes: Some(536_870_912), // 512 MB
            cpu_usage_usec: Some(1_500_000),      // 1.5 seconds
        };

        assert_eq!(info.memory_peak_bytes, Some(536_870_912));
        assert_eq!(info.cpu_usage_usec, Some(1_500_000));
    }

    #[test]
    fn test_instance_info_without_metrics() {
        let info = InstanceInfo {
            instance_id: "inst-123".to_string(),
            image_id: "img-456".to_string(),
            image_name: "my-workflow:v1".to_string(),
            tenant_id: "tenant-1".to_string(),
            status: InstanceStatus::Running,
            checkpoint_id: None,
            created_at: Utc::now(),
            started_at: Some(Utc::now()),
            finished_at: None,
            heartbeat_at: Some(Utc::now()),
            input: Some(json!({"key": "value"})),
            output: None,
            error: None,
            stderr: None,
            retry_count: 0,
            max_retries: 3,
            memory_peak_bytes: None,
            cpu_usage_usec: None,
        };

        assert!(info.memory_peak_bytes.is_none());
        assert!(info.cpu_usage_usec.is_none());
    }

    #[test]
    fn test_instance_info_serde_with_metrics() {
        let info = InstanceInfo {
            instance_id: "inst-123".to_string(),
            image_id: "img-456".to_string(),
            image_name: "workflow".to_string(),
            tenant_id: "tenant-1".to_string(),
            status: InstanceStatus::Completed,
            checkpoint_id: None,
            created_at: Utc::now(),
            started_at: Some(Utc::now()),
            finished_at: Some(Utc::now()),
            heartbeat_at: None,
            input: None,
            output: Some(json!("done")),
            error: None,
            stderr: None,
            retry_count: 1,
            max_retries: 3,
            memory_peak_bytes: Some(1_073_741_824), // 1 GB
            cpu_usage_usec: Some(5_000_000),        // 5 seconds
        };

        let json_str = serde_json::to_string(&info).unwrap();
        let deserialized: InstanceInfo = serde_json::from_str(&json_str).unwrap();

        assert_eq!(deserialized.instance_id, "inst-123");
        assert_eq!(deserialized.memory_peak_bytes, Some(1_073_741_824));
        assert_eq!(deserialized.cpu_usage_usec, Some(5_000_000));
    }

    #[test]
    fn test_instance_info_with_stderr() {
        let info = InstanceInfo {
            instance_id: "inst-123".to_string(),
            image_id: "img-456".to_string(),
            image_name: "my-workflow:v1".to_string(),
            tenant_id: "tenant-1".to_string(),
            status: InstanceStatus::Failed,
            checkpoint_id: None,
            created_at: Utc::now(),
            started_at: Some(Utc::now()),
            finished_at: Some(Utc::now()),
            heartbeat_at: None,
            input: None,
            output: None,
            error: Some("Connection refused".to_string()),
            stderr: Some("thread 'main' panicked at 'assertion failed'".to_string()),
            retry_count: 3,
            max_retries: 3,
            memory_peak_bytes: None,
            cpu_usage_usec: None,
        };

        assert_eq!(info.error, Some("Connection refused".to_string()));
        assert_eq!(
            info.stderr,
            Some("thread 'main' panicked at 'assertion failed'".to_string())
        );
    }
}
