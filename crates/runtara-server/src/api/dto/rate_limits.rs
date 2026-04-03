//! Rate Limit Analytics DTOs
//!
//! Data transfer objects for rate limit status endpoints

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

// ============================================================================
// Configuration DTO (from PostgreSQL)
// ============================================================================

/// Rate limit configuration stored in PostgreSQL
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RateLimitConfigDto {
    /// Requests allowed per second (refill rate)
    pub requests_per_second: u32,
    /// Maximum token capacity (burst size)
    pub burst_size: u32,
    /// Whether to automatically retry when rate limited
    pub retry_on_limit: bool,
    /// Maximum retry attempts
    pub max_retries: u32,
    /// Maximum cumulative wait time in milliseconds
    pub max_wait_ms: u64,
}

// ============================================================================
// Real-time State DTO (from Redis)
// ============================================================================

/// Real-time rate limit state from Redis
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[derive(Default)]
pub struct RateLimitStateDto {
    /// Whether Redis state is available
    pub available: bool,
    /// Current token count in the bucket
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_tokens: Option<f64>,
    /// Last refill timestamp in milliseconds
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_refill_ms: Option<i64>,
    /// Learned rate limit from API response headers
    #[serde(skip_serializing_if = "Option::is_none")]
    pub learned_limit: Option<u32>,
    /// Number of calls made in the current window (since last refill)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub calls_in_window: Option<u32>,
    /// Total lifetime calls made through this connection
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_calls: Option<u64>,
    /// Timestamp when the current window started (milliseconds)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub window_start_ms: Option<i64>,
}

// ============================================================================
// Computed Metrics DTO
// ============================================================================

/// Computed rate limit metrics
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[derive(Default)]
pub struct RateLimitMetricsDto {
    /// Current capacity as percentage (tokens / burst_size * 100)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capacity_percent: Option<f64>,
    /// Current utilization as percentage (100 - capacity_percent)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub utilization_percent: Option<f64>,
    /// Whether the connection is currently rate limited (tokens < 1)
    pub is_rate_limited: bool,
    /// Milliseconds until next token is available (if rate limited)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_after_ms: Option<u64>,
}

// ============================================================================
// Combined Status DTO
// ============================================================================

/// Complete rate limit status for a connection
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RateLimitStatusDto {
    /// Connection ID
    pub connection_id: String,
    /// Connection title
    pub connection_title: String,
    /// Integration ID (connection type)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub integration_id: Option<String>,
    /// Rate limit configuration (from PostgreSQL)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config: Option<RateLimitConfigDto>,
    /// Real-time state (from Redis)
    pub state: RateLimitStateDto,
    /// Computed metrics
    pub metrics: RateLimitMetricsDto,
    /// Aggregated stats for the requested time period
    #[serde(skip_serializing_if = "Option::is_none")]
    pub period_stats: Option<PeriodStatsDto>,
}

// ============================================================================
// Response Types
// ============================================================================

/// Response for single connection rate limit status
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct GetRateLimitStatusResponse {
    pub success: bool,
    pub data: RateLimitStatusDto,
}

/// Response for listing all connections' rate limit status
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ListRateLimitsResponse {
    pub success: bool,
    pub data: Vec<RateLimitStatusDto>,
    pub count: usize,
}

// ============================================================================
// Query Parameters
// ============================================================================

/// Query parameters for listing rate limits
#[derive(Debug, Clone, Deserialize, ToSchema, Default)]
#[serde(rename_all = "camelCase")]
pub struct ListRateLimitsQuery {
    /// Time interval for aggregated stats: 1h, 24h, 7d, 30d (default: 24h)
    #[serde(default = "default_list_interval")]
    pub interval: String,
}

fn default_list_interval() -> String {
    "24h".to_string()
}

// ============================================================================
// Period Stats DTO
// ============================================================================

/// Aggregated rate limit stats for a time period
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PeriodStatsDto {
    /// The interval used for aggregation
    pub interval: String,
    /// Total requests in the period
    pub total_requests: i64,
    /// Number of rate-limited events
    pub rate_limited_count: i64,
    /// Number of retry events
    pub retry_count: i64,
    /// Percentage of requests that were rate-limited
    pub rate_limited_percent: f64,
}

// ============================================================================
// Timeline Event Types
// ============================================================================

/// Rate limit event types
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RateLimitEventType {
    /// A request was made (credential fetch)
    Request,
    /// Request was blocked due to rate limiting
    RateLimited,
    /// A retry attempt was made
    Retry,
}

impl std::fmt::Display for RateLimitEventType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RateLimitEventType::Request => write!(f, "request"),
            RateLimitEventType::RateLimited => write!(f, "rate_limited"),
            RateLimitEventType::Retry => write!(f, "retry"),
        }
    }
}

impl std::str::FromStr for RateLimitEventType {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "request" => Ok(RateLimitEventType::Request),
            "rate_limited" => Ok(RateLimitEventType::RateLimited),
            "retry" => Ok(RateLimitEventType::Retry),
            _ => Err(format!("Unknown event type: {}", s)),
        }
    }
}

/// A single rate limit event in the timeline
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RateLimitEventDto {
    /// Event ID
    pub id: i64,
    /// Connection ID
    pub connection_id: String,
    /// Type of event
    pub event_type: String,
    /// When the event occurred
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// Additional event metadata
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
}

/// Query parameters for rate limit history
#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RateLimitHistoryQuery {
    /// Maximum number of events to return (default: 100, max: 1000)
    #[serde(default = "default_limit")]
    pub limit: i64,
    /// Number of events to skip (for pagination)
    #[serde(default)]
    pub offset: i64,
    /// Filter by event type
    pub event_type: Option<String>,
    /// Filter events after this timestamp
    pub from: Option<chrono::DateTime<chrono::Utc>>,
    /// Filter events before this timestamp
    pub to: Option<chrono::DateTime<chrono::Utc>>,
}

fn default_limit() -> i64 {
    100
}

/// Response for rate limit history endpoint
#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RateLimitHistoryResponse {
    pub success: bool,
    pub data: Vec<RateLimitEventDto>,
    pub total_count: i64,
    pub limit: i64,
    pub offset: i64,
}

// ============================================================================
// Timeline (Time-Bucketed) Types
// ============================================================================

/// Query parameters for rate limit timeline (time-bucketed aggregation)
#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RateLimitTimelineQuery {
    /// Start time (ISO 8601), defaults to 1 hour ago
    pub start_time: Option<chrono::DateTime<chrono::Utc>>,
    /// End time (ISO 8601), defaults to now
    pub end_time: Option<chrono::DateTime<chrono::Utc>>,
    /// Time granularity: minute, hourly, daily (default: minute)
    #[serde(default = "default_timeline_granularity")]
    pub granularity: String,
    /// Optional tag filter (e.g. agent name like "shopify_graphql")
    pub tag: Option<String>,
}

fn default_timeline_granularity() -> String {
    "minute".to_string()
}

/// A single time bucket in the timeline
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RateLimitTimelineBucket {
    /// Start of the time bucket
    pub bucket: chrono::DateTime<chrono::Utc>,
    /// Number of request events in this bucket
    pub request_count: i64,
    /// Number of rate_limited events in this bucket
    pub rate_limited_count: i64,
    /// Number of retry events in this bucket
    pub retry_count: i64,
}

/// Response data for the timeline endpoint
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RateLimitTimelineData {
    pub connection_id: String,
    pub start_time: chrono::DateTime<chrono::Utc>,
    pub end_time: chrono::DateTime<chrono::Utc>,
    pub granularity: String,
    pub buckets: Vec<RateLimitTimelineBucket>,
}

/// Response for the timeline endpoint
#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RateLimitTimelineResponse {
    pub success: bool,
    pub data: RateLimitTimelineData,
    pub bucket_count: usize,
}
