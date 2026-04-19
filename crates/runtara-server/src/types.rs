// Type-safe enums for workflow execution states and configuration

use serde::{Deserialize, Serialize};
use sqlx::Type;
use std::fmt;
use std::ops::Deref;
use utoipa::ToSchema;

// Re-export CancellationHandle from workers module
pub use crate::workers::CancellationHandle;

/// Execution status representing the current state of a workflow execution
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type, ToSchema)]
#[sqlx(type_name = "text", rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]
pub enum ExecutionStatus {
    /// Waiting in queue to be executed
    Queued,
    /// Being compiled
    Compiling,
    /// Currently executing
    Running,
    /// Paused at a breakpoint or by user request
    Suspended,
    /// Successfully finished
    Completed,
    /// Execution error occurred
    Failed,
    /// Exceeded time limit
    Timeout,
    /// User or system cancelled
    Cancelled,
}

impl ExecutionStatus {
    /// Check if this is a terminal (final) state
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            ExecutionStatus::Completed
                | ExecutionStatus::Failed
                | ExecutionStatus::Cancelled
                | ExecutionStatus::Timeout
        )
    }

    /// Check if this is an active (non-terminal) state
    pub fn is_active(&self) -> bool {
        matches!(
            self,
            ExecutionStatus::Queued
                | ExecutionStatus::Compiling
                | ExecutionStatus::Running
                | ExecutionStatus::Suspended
        )
    }

    /// Check if execution was successful
    pub fn is_success(&self) -> bool {
        matches!(self, ExecutionStatus::Completed)
    }

    /// Check if execution failed (any failure state)
    pub fn is_failure(&self) -> bool {
        matches!(
            self,
            ExecutionStatus::Failed | ExecutionStatus::Timeout | ExecutionStatus::Cancelled
        )
    }

    /// Get string representation (matches database values)
    pub fn as_str(&self) -> &'static str {
        match self {
            ExecutionStatus::Queued => "queued",
            ExecutionStatus::Compiling => "compiling",
            ExecutionStatus::Running => "running",
            ExecutionStatus::Suspended => "suspended",
            ExecutionStatus::Completed => "completed",
            ExecutionStatus::Failed => "failed",
            ExecutionStatus::Timeout => "timeout",
            ExecutionStatus::Cancelled => "cancelled",
        }
    }
}

impl fmt::Display for ExecutionStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// Termination type providing context for why an execution terminated
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type, ToSchema)]
#[sqlx(type_name = "text", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum TerminationType {
    /// Execution completed successfully
    NormalCompletion,
    /// User clicked stop/cancel button
    UserInitiated,
    /// Stuck in queue for >24 hours
    QueueTimeout,
    /// Exceeded maximum runtime
    ExecutionTimeout,
    /// Internal error or crash during execution
    SystemError,
}

impl TerminationType {
    /// Get a human-readable description of the termination reason
    pub fn description(&self) -> &'static str {
        match self {
            TerminationType::NormalCompletion => "Execution completed successfully",
            TerminationType::UserInitiated => "Cancelled by user request",
            TerminationType::QueueTimeout => "Execution timed out in queue (24h limit)",
            TerminationType::ExecutionTimeout => "Execution exceeded maximum runtime",
            TerminationType::SystemError => "System error occurred during execution",
        }
    }
}

// ============================================================================
// MemoryTier - Wrapper around runtara_dsl::MemoryTier with sqlx support
// ============================================================================

/// Memory tier configuration for workflows
///
/// This is a newtype wrapper around runtara_dsl::MemoryTier that adds sqlx support.
/// Determines initial memory allocation and HTTP buffer sizes.
/// Each tier is optimized to support at least 10 parallel HTTP operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
#[derive(Default)]
pub struct MemoryTier(pub runtara_dsl::MemoryTier);

impl utoipa::ToSchema for MemoryTier {
    fn name() -> std::borrow::Cow<'static, str> {
        std::borrow::Cow::Borrowed("MemoryTier")
    }
}

impl utoipa::PartialSchema for MemoryTier {
    fn schema() -> utoipa::openapi::RefOr<utoipa::openapi::schema::Schema> {
        use utoipa::openapi::schema::{ObjectBuilder, SchemaType, Type};
        ObjectBuilder::new()
            .schema_type(SchemaType::Type(Type::String))
            .enum_values(Some(["S", "M", "L", "XL"]))
            .description(Some("Memory allocation tier for workflow execution"))
            .into()
    }
}

impl MemoryTier {
    pub const S: MemoryTier = MemoryTier(runtara_dsl::MemoryTier::S);
    pub const M: MemoryTier = MemoryTier(runtara_dsl::MemoryTier::M);
    pub const L: MemoryTier = MemoryTier(runtara_dsl::MemoryTier::L);
    pub const XL: MemoryTier = MemoryTier(runtara_dsl::MemoryTier::XL);

    /// Total memory allocation in bytes
    pub fn total_memory_bytes(&self) -> usize {
        self.0.total_memory_bytes()
    }

    /// Stack size in bytes
    pub fn stack_size_bytes(&self) -> usize {
        self.0.stack_size_bytes()
    }

    /// HTTP metadata buffer size per request (for headers)
    pub fn http_metadata_buffer_bytes(&self) -> usize {
        match self.0 {
            runtara_dsl::MemoryTier::S => 64 * 1024,    // 64KB
            runtara_dsl::MemoryTier::M => 256 * 1024,   // 256KB
            runtara_dsl::MemoryTier::L => 512 * 1024,   // 512KB
            runtara_dsl::MemoryTier::XL => 1024 * 1024, // 1MB
        }
    }

    /// HTTP body buffer size per request
    pub fn http_body_buffer_bytes(&self) -> usize {
        match self.0 {
            runtara_dsl::MemoryTier::S => 640 * 1024, // 640KB
            runtara_dsl::MemoryTier::M => 6029312,    // ~5.76MB
            runtara_dsl::MemoryTier::L => 12349030,   // ~11.78MB
            runtara_dsl::MemoryTier::XL => 24748851, // ~23.6MB (256MB - 8MB stack - 10 * 1MB metadata) / 10
        }
    }

    /// Total memory required for 10 parallel HTTP operations
    pub fn parallel_http_memory_bytes(&self) -> usize {
        10 * (self.http_metadata_buffer_bytes() + self.http_body_buffer_bytes())
    }

    /// Memory headroom (total - stack - 10 parallel HTTP operations)
    pub fn memory_headroom_bytes(&self) -> i64 {
        self.total_memory_bytes() as i64
            - self.stack_size_bytes() as i64
            - self.parallel_http_memory_bytes() as i64
    }

    /// Parse from string (case-insensitive)
    pub fn parse(s: &str) -> Option<Self> {
        runtara_dsl::MemoryTier::parse(s).map(MemoryTier)
    }

    /// Get as string
    pub fn as_str(&self) -> &'static str {
        self.0.as_str()
    }

    /// Convert to the underlying runtara_dsl::MemoryTier
    pub fn into_inner(self) -> runtara_dsl::MemoryTier {
        self.0
    }
}

impl fmt::Display for MemoryTier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<runtara_dsl::MemoryTier> for MemoryTier {
    fn from(tier: runtara_dsl::MemoryTier) -> Self {
        MemoryTier(tier)
    }
}

impl From<MemoryTier> for runtara_dsl::MemoryTier {
    fn from(tier: MemoryTier) -> Self {
        tier.0
    }
}

impl Deref for MemoryTier {
    type Target = runtara_dsl::MemoryTier;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

// For sqlx - decode from database VARCHAR
impl sqlx::Type<sqlx::Postgres> for MemoryTier {
    fn type_info() -> sqlx::postgres::PgTypeInfo {
        <String as sqlx::Type<sqlx::Postgres>>::type_info()
    }
}

impl<'r> sqlx::Decode<'r, sqlx::Postgres> for MemoryTier {
    fn decode(
        value: sqlx::postgres::PgValueRef<'r>,
    ) -> Result<Self, Box<dyn std::error::Error + 'static + Send + Sync>> {
        let s = <String as sqlx::Decode<sqlx::Postgres>>::decode(value)?;
        MemoryTier::parse(&s).ok_or_else(|| format!("Invalid memory tier: {}", s).into())
    }
}

impl<'q> sqlx::Encode<'q, sqlx::Postgres> for MemoryTier {
    fn encode_by_ref(
        &self,
        buf: &mut sqlx::postgres::PgArgumentBuffer,
    ) -> Result<sqlx::encode::IsNull, Box<dyn std::error::Error + Sync + Send>> {
        <&str as sqlx::Encode<sqlx::Postgres>>::encode_by_ref(&self.as_str(), buf)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_execution_status_helpers() {
        assert!(ExecutionStatus::Queued.is_active());
        assert!(ExecutionStatus::Running.is_active());
        assert!(!ExecutionStatus::Completed.is_active());

        assert!(ExecutionStatus::Completed.is_terminal());
        assert!(ExecutionStatus::Failed.is_terminal());
        assert!(!ExecutionStatus::Queued.is_terminal());

        assert!(ExecutionStatus::Completed.is_success());
        assert!(!ExecutionStatus::Failed.is_success());

        assert!(ExecutionStatus::Failed.is_failure());
        assert!(ExecutionStatus::Timeout.is_failure());
        assert!(!ExecutionStatus::Completed.is_failure());
    }

    #[test]
    fn test_serde_serialization() {
        // Test ExecutionStatus serialization
        let status = ExecutionStatus::Completed;
        let json = serde_json::to_string(&status).unwrap();
        assert_eq!(json, "\"completed\"");

        let deserialized: ExecutionStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, ExecutionStatus::Completed);

        // Test TerminationType serialization
        let termination = TerminationType::NormalCompletion;
        let json = serde_json::to_string(&termination).unwrap();
        assert_eq!(json, "\"normal_completion\"");

        let deserialized: TerminationType = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, TerminationType::NormalCompletion);
    }

    #[test]
    fn test_memory_tier_calculations() {
        // Test S tier
        assert_eq!(MemoryTier::S.total_memory_bytes(), 8 * 1024 * 1024);
        assert_eq!(MemoryTier::S.stack_size_bytes(), 1024 * 1024);
        assert_eq!(MemoryTier::S.http_metadata_buffer_bytes(), 64 * 1024);
        assert_eq!(MemoryTier::S.http_body_buffer_bytes(), 640 * 1024);

        // 10 parallel ops: 10 * (64KB + 640KB) = 7.04MB
        let parallel_memory = MemoryTier::S.parallel_http_memory_bytes();
        assert_eq!(parallel_memory, 10 * (64 * 1024 + 640 * 1024));

        // Headroom: 8MB - 1MB - 7.04MB = ~0MB (tight but workable)
        let headroom = MemoryTier::S.memory_headroom_bytes();
        assert!(headroom < 1024 * 1024); // Less than 1MB headroom

        // Test XL tier
        assert_eq!(MemoryTier::XL.total_memory_bytes(), 256 * 1024 * 1024);
        assert_eq!(MemoryTier::XL.stack_size_bytes(), 8 * 1024 * 1024);

        // Verify XL has positive headroom
        let xl_headroom = MemoryTier::XL.memory_headroom_bytes();
        assert!(xl_headroom > 0);
    }

    #[test]
    fn test_memory_tier_from_str() {
        assert_eq!(MemoryTier::parse("S"), Some(MemoryTier::S));
        assert_eq!(MemoryTier::parse("s"), Some(MemoryTier::S));
        assert_eq!(MemoryTier::parse("XL"), Some(MemoryTier::XL));
        assert_eq!(MemoryTier::parse("xl"), Some(MemoryTier::XL));
        assert_eq!(MemoryTier::parse("invalid"), None);
    }

    #[test]
    fn test_memory_tier_default() {
        assert_eq!(MemoryTier::default(), MemoryTier::XL);
    }

    #[test]
    fn test_memory_tier_display() {
        assert_eq!(MemoryTier::S.to_string(), "S");
        assert_eq!(MemoryTier::M.to_string(), "M");
        assert_eq!(MemoryTier::L.to_string(), "L");
        assert_eq!(MemoryTier::XL.to_string(), "XL");
    }

    #[test]
    fn test_memory_tier_conversion() {
        let dsl_tier = runtara_dsl::MemoryTier::XL;
        let runtime_tier: MemoryTier = dsl_tier.into();
        assert_eq!(runtime_tier, MemoryTier::XL);

        let back: runtara_dsl::MemoryTier = runtime_tier.into();
        assert!(matches!(back, runtara_dsl::MemoryTier::XL));
    }
}
