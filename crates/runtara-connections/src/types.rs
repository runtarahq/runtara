//! Types and DTOs for the runtara-connections crate.

use serde::{Deserialize, Serialize};
#[cfg(feature = "utoipa")]
use utoipa::ToSchema;

// ============================================================================
// Configuration DTO (from PostgreSQL)
// ============================================================================

/// Rate limit configuration stored in PostgreSQL
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
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
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
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
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
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
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
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
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct GetRateLimitStatusResponse {
    pub success: bool,
    pub data: RateLimitStatusDto,
}

/// Response for listing all connections' rate limit status
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct ListRateLimitsResponse {
    pub success: bool,
    pub data: Vec<RateLimitStatusDto>,
    pub count: usize,
}

// ============================================================================
// Query Parameters
// ============================================================================

/// Query parameters for listing rate limits
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
#[derive(Default)]
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
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
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
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
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
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
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
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
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
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
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
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
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
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
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
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct RateLimitTimelineData {
    pub connection_id: String,
    pub start_time: chrono::DateTime<chrono::Utc>,
    pub end_time: chrono::DateTime<chrono::Utc>,
    pub granularity: String,
    pub buckets: Vec<RateLimitTimelineBucket>,
}

/// Response for the timeline endpoint
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct RateLimitTimelineResponse {
    pub success: bool,
    pub data: RateLimitTimelineData,
    pub bucket_count: usize,
}

// ============================================================================
// Connection Status Enum
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ConnectionStatus {
    Unknown,
    Active,
    RequiresReconnection,
    InvalidCredentials,
}

impl ConnectionStatus {
    pub fn as_str(&self) -> &str {
        match self {
            ConnectionStatus::Unknown => "UNKNOWN",
            ConnectionStatus::Active => "ACTIVE",
            ConnectionStatus::RequiresReconnection => "REQUIRES_RECONNECTION",
            ConnectionStatus::InvalidCredentials => "INVALID_CREDENTIALS",
        }
    }

    pub fn parse(s: &str) -> Self {
        match s {
            "ACTIVE" => ConnectionStatus::Active,
            "REQUIRES_RECONNECTION" => ConnectionStatus::RequiresReconnection,
            "INVALID_CREDENTIALS" => ConnectionStatus::InvalidCredentials,
            _ => ConnectionStatus::Unknown,
        }
    }
}

// ============================================================================
// DTOs (Data Transfer Objects)
// ============================================================================

/// Connection DTO - Used for GET/LIST responses
/// SECURITY: Does NOT include connection_parameters field
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct ConnectionDto {
    pub id: String,
    #[serde(rename = "tenantId")]
    pub tenant_id: String,
    #[serde(rename = "createdAt")]
    pub created_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "validUntil")]
    pub valid_until: Option<String>,
    #[serde(rename = "updatedAt")]
    pub updated_at: String,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "connectionSubtype")]
    pub connection_subtype: Option<String>,
    /// Connection type identifier that maps to a connection schema (e.g., shopify_access_token, bearer, sftp)
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "integrationId")]
    pub integration_id: Option<String>,
    pub status: ConnectionStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "rateLimitConfig")]
    pub rate_limit_config: Option<RateLimitConfigDto>,
    /// Rate limit statistics for the requested time period (only included when requested)
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "rateLimitStats")]
    pub rate_limit_stats: Option<PeriodStatsDto>,
    /// When true, this connection is the default S3 storage for webhook attachments
    #[serde(rename = "isDefaultFileStorage")]
    pub is_default_file_storage: bool,
    // NOTE: connection_parameters is intentionally NOT included for security
}

/// Create connection request
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct CreateConnectionRequest {
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "connectionSubtype")]
    pub connection_subtype: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "connectionParameters")]
    pub connection_parameters: Option<serde_json::Value>,
    /// Connection type identifier that maps to a connection schema (e.g., shopify_access_token, bearer, sftp)
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "integrationId")]
    pub integration_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "rateLimitConfig")]
    pub rate_limit_config: Option<RateLimitConfigDto>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "validUntil")]
    pub valid_until: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<ConnectionStatus>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "isDefaultFileStorage")]
    pub is_default_file_storage: Option<bool>,
}

/// Update connection request - all fields optional
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct UpdateConnectionRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "connectionSubtype")]
    pub connection_subtype: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "connectionParameters")]
    pub connection_parameters: Option<serde_json::Value>,
    /// Connection type identifier that maps to a connection schema (e.g., shopify_access_token, bearer, sftp)
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "integrationId")]
    pub integration_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "rateLimitConfig")]
    pub rate_limit_config: Option<RateLimitConfigDto>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "validUntil")]
    pub valid_until: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<ConnectionStatus>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "isDefaultFileStorage")]
    pub is_default_file_storage: Option<bool>,
}

/// Response for listing connections
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct ListConnectionsResponse {
    pub success: bool,
    pub connections: Vec<ConnectionDto>,
    pub count: usize,
}

/// Response for single connection operations
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct ConnectionResponse {
    pub success: bool,
    pub connection: ConnectionDto,
}

/// Response for create operation
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct CreateConnectionResponse {
    pub success: bool,
    pub message: String,
    #[serde(rename = "connectionId")]
    pub connection_id: String,
}

/// Response for delete operation
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct DeleteConnectionResponse {
    pub success: bool,
    pub message: String,
}

// ============================================================================
// Query Parameters
// ============================================================================

#[derive(Debug, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::IntoParams))]
#[serde(rename_all = "camelCase")]
pub struct ListConnectionsQuery {
    /// Filter by integration_id (connection type identifier)
    #[serde(rename = "integrationId")]
    pub integration_id: Option<String>,
    pub status: Option<String>,
    /// Include rate limit statistics for each connection
    #[serde(default)]
    pub include_rate_limit_stats: bool,
    /// Time interval for rate limit stats: 1h, 24h, 7d, 30d (default: 24h)
    /// Only used when includeRateLimitStats is true
    #[serde(default = "default_interval")]
    pub interval: String,
}

fn default_interval() -> String {
    "24h".to_string()
}

// ============================================================================
// Connection Categories
// ============================================================================

/// Canonical list of connection categories.
///
/// Used for grouping connection types in the UI and API responses.
/// When adding a new integration, pick the most specific category that fits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum ConnectionCategory {
    Ecommerce,
    FileStorage,
    Llm,
    Crm,
    Erp,
    Database,
    Email,
    Messaging,
    Payment,
    Cloud,
    Api,
}

impl ConnectionCategory {
    /// All categories in preferred display order
    pub const ALL: &[ConnectionCategory] = &[
        ConnectionCategory::Ecommerce,
        ConnectionCategory::FileStorage,
        ConnectionCategory::Llm,
        ConnectionCategory::Crm,
        ConnectionCategory::Erp,
        ConnectionCategory::Database,
        ConnectionCategory::Email,
        ConnectionCategory::Messaging,
        ConnectionCategory::Payment,
        ConnectionCategory::Cloud,
        ConnectionCategory::Api,
    ];

    /// Snake_case identifier (matches serde serialization)
    pub fn id(&self) -> &'static str {
        match self {
            Self::Ecommerce => "ecommerce",
            Self::FileStorage => "file_storage",
            Self::Llm => "llm",
            Self::Crm => "crm",
            Self::Erp => "erp",
            Self::Database => "database",
            Self::Email => "email",
            Self::Messaging => "messaging",
            Self::Payment => "payment",
            Self::Cloud => "cloud",
            Self::Api => "api",
        }
    }

    /// Human-readable display name
    pub fn display_name(&self) -> &'static str {
        match self {
            Self::Ecommerce => "E-Commerce",
            Self::FileStorage => "File Storage",
            Self::Llm => "AI / LLM",
            Self::Crm => "CRM",
            Self::Erp => "ERP",
            Self::Database => "Database",
            Self::Email => "Email",
            Self::Messaging => "Messaging",
            Self::Payment => "Payment",
            Self::Cloud => "Cloud",
            Self::Api => "API",
        }
    }

    /// Short description of what this category covers
    pub fn description(&self) -> &'static str {
        match self {
            Self::Ecommerce => "Online store and marketplace platforms",
            Self::FileStorage => "File transfer and cloud storage services",
            Self::Llm => "Large language models and AI services",
            Self::Crm => "Customer relationship management systems",
            Self::Erp => "Enterprise resource planning systems",
            Self::Database => "Relational and document database connections",
            Self::Email => "Email delivery and transactional email services",
            Self::Messaging => "Chat and messaging platforms",
            Self::Payment => "Payment processing and billing platforms",
            Self::Cloud => "Cloud infrastructure providers",
            Self::Api => "Generic REST, GraphQL, or webhook endpoints",
        }
    }

    /// Parse from a string, accepting common legacy variants
    #[allow(dead_code)]
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_lowercase().replace('-', "_").as_str() {
            "ecommerce" | "e_commerce" => Some(Self::Ecommerce),
            "file_storage" | "storage" => Some(Self::FileStorage),
            "llm" | "ai" | "ai_llm" => Some(Self::Llm),
            "crm" => Some(Self::Crm),
            "erp" => Some(Self::Erp),
            "database" | "db" => Some(Self::Database),
            "email" | "smtp" => Some(Self::Email),
            "messaging" | "chat" => Some(Self::Messaging),
            "payment" => Some(Self::Payment),
            "cloud" => Some(Self::Cloud),
            "api" | "rest" | "graphql" | "webhook" => Some(Self::Api),
            _ => None,
        }
    }
}

impl std::fmt::Display for ConnectionCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.id())
    }
}

/// DTO for returning category metadata to the frontend
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct ConnectionCategoryDto {
    /// Category identifier (snake_case)
    pub id: String,
    /// Human-readable display name
    pub display_name: String,
    /// Short description of what this category covers
    pub description: String,
}

impl From<ConnectionCategory> for ConnectionCategoryDto {
    fn from(cat: ConnectionCategory) -> Self {
        Self {
            id: cat.id().to_string(),
            display_name: cat.display_name().to_string(),
            description: cat.description().to_string(),
        }
    }
}

/// Response for listing all connection categories
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct ListConnectionCategoriesResponse {
    pub success: bool,
    pub categories: Vec<ConnectionCategoryDto>,
    pub count: usize,
}

// ============================================================================
// Connection Auth Types
// ============================================================================

/// Canonical list of authentication / credential types for connections.
///
/// Describes **what credentials** are used to authenticate, not how they are
/// transported (e.g. bearer header is a delivery mechanism, not a credential type).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum ConnectionAuthType {
    /// Static secret key for API authentication
    ApiKey,
    /// User-interactive OAuth2 with redirect and consent
    Oauth2AuthorizationCode,
    /// Machine-to-machine OAuth2 token exchange
    Oauth2ClientCredentials,
    /// Credential pair authentication (login + password)
    UsernamePassword,
    /// Private key authentication (e.g. SSH, SFTP)
    SshKey,
    /// IAM-style key pair (key ID + secret key)
    AccessKey,
    /// Database DSN or connection URI
    ConnectionString,
    /// Integration-specific authentication that doesn't fit other types
    Custom,
}

impl ConnectionAuthType {
    /// All auth types in preferred display order
    pub const ALL: &[ConnectionAuthType] = &[
        ConnectionAuthType::ApiKey,
        ConnectionAuthType::Oauth2AuthorizationCode,
        ConnectionAuthType::Oauth2ClientCredentials,
        ConnectionAuthType::UsernamePassword,
        ConnectionAuthType::SshKey,
        ConnectionAuthType::AccessKey,
        ConnectionAuthType::ConnectionString,
        ConnectionAuthType::Custom,
    ];

    /// Snake_case identifier (matches serde serialization)
    pub fn id(&self) -> &'static str {
        match self {
            Self::ApiKey => "api_key",
            Self::Oauth2AuthorizationCode => "oauth2_authorization_code",
            Self::Oauth2ClientCredentials => "oauth2_client_credentials",
            Self::UsernamePassword => "username_password",
            Self::SshKey => "ssh_key",
            Self::AccessKey => "access_key",
            Self::ConnectionString => "connection_string",
            Self::Custom => "custom",
        }
    }

    /// Human-readable display name
    pub fn display_name(&self) -> &'static str {
        match self {
            Self::ApiKey => "API Key",
            Self::Oauth2AuthorizationCode => "OAuth2 (Authorization Code)",
            Self::Oauth2ClientCredentials => "OAuth2 (Client Credentials)",
            Self::UsernamePassword => "Username & Password",
            Self::SshKey => "SSH Key",
            Self::AccessKey => "Access Key & Secret",
            Self::ConnectionString => "Connection String",
            Self::Custom => "Custom",
        }
    }

    /// Short description of this authentication type
    pub fn description(&self) -> &'static str {
        match self {
            Self::ApiKey => "Static secret key for API authentication",
            Self::Oauth2AuthorizationCode => "User-interactive OAuth2 with redirect and consent",
            Self::Oauth2ClientCredentials => "Machine-to-machine OAuth2 token exchange",
            Self::UsernamePassword => "Credential pair authentication",
            Self::SshKey => "Private key authentication",
            Self::AccessKey => "IAM-style key pair (key ID + secret key)",
            Self::ConnectionString => "Database DSN or connection URI",
            Self::Custom => "Integration-specific authentication",
        }
    }

    /// Parse from a string, accepting legacy SCREAMING_SNAKE_CASE variants
    /// from common forms.
    #[allow(dead_code)]
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_lowercase().replace('-', "_").as_str() {
            "api_key" => Some(Self::ApiKey),
            "oauth2_authorization_code" | "oauth2" => Some(Self::Oauth2AuthorizationCode),
            "oauth2_client_credentials" => Some(Self::Oauth2ClientCredentials),
            "username_password" => Some(Self::UsernamePassword),
            "ssh_key" => Some(Self::SshKey),
            "access_key" => Some(Self::AccessKey),
            "connection_string" | "dsn" => Some(Self::ConnectionString),
            "custom" => Some(Self::Custom),
            _ => None,
        }
    }
}

impl std::fmt::Display for ConnectionAuthType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.id())
    }
}

/// DTO for returning auth type metadata to the frontend
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct ConnectionAuthTypeDto {
    /// Auth type identifier (snake_case)
    pub id: String,
    /// Human-readable display name
    pub display_name: String,
    /// Short description of this authentication type
    pub description: String,
}

impl From<ConnectionAuthType> for ConnectionAuthTypeDto {
    fn from(auth: ConnectionAuthType) -> Self {
        Self {
            id: auth.id().to_string(),
            display_name: auth.display_name().to_string(),
            description: auth.description().to_string(),
        }
    }
}

/// Response for listing all connection auth types
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct ListConnectionAuthTypesResponse {
    pub success: bool,
    pub auth_types: Vec<ConnectionAuthTypeDto>,
    pub count: usize,
}

// ============================================================================
// Connection Type Schema DTOs
// ============================================================================

/// A field in a connection type's parameter schema
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct ConnectionFieldDto {
    /// Field name (used in JSON)
    pub name: String,
    /// Type name (String, u16, bool, etc.)
    pub type_name: String,
    /// Whether this field is optional
    pub is_optional: bool,
    /// Display name for UI
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    /// Description of the field
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Placeholder text for the input
    #[serde(skip_serializing_if = "Option::is_none")]
    pub placeholder: Option<String>,
    /// Default value
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_value: Option<String>,
    /// Whether this is a secret field (password, API key, etc.)
    pub is_secret: bool,
}

/// OAuth2 configuration for a connection type (authorization code flow)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct OAuthConfigDto {
    /// Provider's authorization endpoint
    pub auth_url: String,
    /// Provider's token endpoint
    pub token_url: String,
    /// Space-separated default scopes
    pub default_scopes: String,
}

/// A connection type with its parameter schema
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct ConnectionTypeDto {
    /// Unique identifier for this connection type
    pub integration_id: String,
    /// Display name for UI
    pub display_name: String,
    /// Description of this connection type
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Category for grouping (e.g., "ecommerce", "file_storage", "llm")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    /// Fields required for this connection type
    pub fields: Vec<ConnectionFieldDto>,
    /// Default rate limit configuration for this connection type (if applicable)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_rate_limit_config: Option<RateLimitConfigDto>,
    /// OAuth2 configuration (only for auth_type = oauth2_authorization_code)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub oauth_config: Option<OAuthConfigDto>,
}

/// Response for listing all connection types
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct ListConnectionTypesResponse {
    pub success: bool,
    pub connection_types: Vec<ConnectionTypeDto>,
    pub count: usize,
}

/// Response for getting a single connection type
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct ConnectionTypeResponse {
    pub success: bool,
    pub connection_type: ConnectionTypeDto,
}

// ============================================================================
// Runtara Runtime Connection DTOs
// ============================================================================

/// Rate limit state for runtara-workflows runtime
/// This matches the format expected by runtara-workflow-stdlib
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub struct RuntimeRateLimitState {
    /// Whether the connection is currently rate limited
    pub is_limited: bool,
    /// Remaining requests in the current window (if known)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remaining: Option<u32>,
    /// Unix timestamp when the rate limit resets (if known)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reset_at: Option<i64>,
    /// Milliseconds to wait before retrying (if rate limited)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_after_ms: Option<u64>,
}

/// Connection response for runtara-workflows runtime
/// This is the format expected by runtara-workflow-stdlib when fetching connection credentials
///
/// SECURITY NOTE: This response INCLUDES connection_parameters (credentials).
/// This endpoint should only be called by runtara-workflows internally, not exposed to clients.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub struct RuntimeConnectionResponse {
    /// Connection credentials/configuration (decrypted)
    pub parameters: serde_json::Value,
    /// Connection type identifier (e.g., "sftp", "bearer", "api_key")
    pub integration_id: String,
    /// Optional subtype for connections with variants
    #[serde(skip_serializing_if = "Option::is_none")]
    pub connection_subtype: Option<String>,
    /// Current rate limit state (optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rate_limit: Option<RuntimeRateLimitState>,
}

/// Query parameters for the internal runtime connection endpoint
/// Used to pass context about which agent/step is requesting the connection
#[derive(Debug, Clone, Deserialize, Default)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct RuntimeConnectionQuery {
    /// Tag identifying the caller (e.g. agent name like "shopify_graphql")
    pub tag: Option<String>,
    /// Step ID that triggered this connection request
    pub step_id: Option<String>,
    /// Workflow ID that is executing
    pub workflow_id: Option<String>,
    /// Instance ID of the execution
    pub instance_id: Option<String>,
}
