// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Shared types used across agents

use base64::{Engine as _, engine::general_purpose};
use runtara_agent_macro::CapabilityOutput;
pub use runtara_dsl::{ErrorCategory, ErrorSeverity};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::fmt;

/// Represents a base64-encoded file payload that can flow through mappings
#[derive(Debug, Clone, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(
    display_name = "File Data",
    description = "Base64-encoded file with optional metadata"
)]
pub struct FileData {
    #[field(display_name = "Content", description = "Base64-encoded file content")]
    pub content: String,

    #[field(
        display_name = "Filename",
        description = "Original filename (optional)"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filename: Option<String>,

    #[field(
        display_name = "MIME Type",
        description = "MIME type (e.g., 'text/plain', 'text/csv', 'application/xml')"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "mimeType")]
    pub mime_type: Option<String>,
}

impl FileData {
    /// Decode the base64 content to raw bytes
    pub fn decode(&self) -> Result<Vec<u8>, AgentError> {
        general_purpose::STANDARD
            .decode(&self.content)
            .map_err(|e| {
                AgentError::permanent(
                    "FILE_BASE64_DECODE_ERROR",
                    format!("Failed to decode base64 file content: {}", e),
                )
                .with_attr("decode_error", e.to_string())
            })
    }

    /// Create FileData from raw bytes
    pub fn from_bytes(data: Vec<u8>, filename: Option<String>, mime_type: Option<String>) -> Self {
        FileData {
            content: general_purpose::STANDARD.encode(&data),
            filename,
            mime_type,
        }
    }

    /// Try to parse a Value as FileData
    pub fn from_value(value: &Value) -> Result<Self, AgentError> {
        match value {
            Value::String(s) => Ok(FileData {
                content: s.clone(),
                filename: None,
                mime_type: None,
            }),
            Value::Object(_) => serde_json::from_value(value.clone()).map_err(|e| {
                AgentError::permanent(
                    "FILE_INVALID_STRUCTURE",
                    format!("Invalid file data structure: {}", e),
                )
                .with_attr("parse_error", e.to_string())
            }),
            Value::Array(arr) => {
                let mut bytes = Vec::with_capacity(arr.len());
                for (idx, v) in arr.iter().enumerate() {
                    let num = v.as_u64().ok_or_else(|| {
                        AgentError::permanent(
                            "FILE_INVALID_BYTE_ARRAY",
                            "Byte array must contain only numbers",
                        )
                        .with_attr("index", idx.to_string())
                    })?;
                    if num > 255 {
                        return Err(AgentError::permanent(
                            "FILE_BYTE_OUT_OF_RANGE",
                            format!(
                                "Byte value {} at index {} must be in the range 0-255",
                                num, idx
                            ),
                        )
                        .with_attr("index", idx.to_string())
                        .with_attr("value", num.to_string()));
                    }
                    bytes.push(num as u8);
                }
                Ok(FileData::from_bytes(bytes, None, None))
            }
            other => {
                let type_name = match other {
                    Value::Null => "null",
                    Value::Bool(_) => "boolean",
                    Value::Number(_) => "number",
                    _ => "unknown",
                };
                Err(AgentError::permanent(
                    "FILE_INVALID_INPUT_TYPE",
                    "File data must be a string (base64), byte array, or object with content field",
                )
                .with_attr("received_type", type_name))
            }
        }
    }
}

/// Token usage statistics for LLM capabilities
#[derive(Debug, Clone, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(
    display_name = "LLM Usage",
    description = "Token count statistics from LLM API calls"
)]
#[serde(rename_all = "camelCase")]
pub struct LlmUsage {
    #[field(
        display_name = "Prompt Tokens",
        description = "Token count for input prompt",
        example = "150"
    )]
    pub prompt_tokens: i32,

    #[field(
        display_name = "Completion Tokens",
        description = "Token count for generated response",
        example = "50"
    )]
    pub completion_tokens: i32,

    #[field(
        display_name = "Total Tokens",
        description = "Combined token count",
        example = "200"
    )]
    pub total_tokens: i32,
}

/// Structured error for agent capabilities.
///
/// Provides error classification for proper handling:
/// - **Transient**: Temporary failures (network, timeout, rate limit) - `#[resilient]` retries
/// - **Permanent**: Non-recoverable failures (404, validation, business rules) - human intervention may help
///
/// To distinguish technical vs business errors within Permanent category, use:
/// - `code`: e.g., `VALIDATION_*` for technical, `BUSINESS_*` or domain-specific codes for business
/// - `severity`: `Error` for technical failures, `Warning` for expected business outcomes
///
/// Example business error:
/// ```rust,ignore
/// AgentError::permanent("CREDIT_LIMIT_EXCEEDED", "Order exceeds credit limit")
///     .with_severity(ErrorSeverity::Warning)
///     .with_attr("order_amount", "5000")
///     .with_attr("credit_limit", "3000")
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentError {
    /// Machine-readable error code (e.g., "HTTP_TIMEOUT", "VALIDATION_ERROR")
    pub code: String,

    /// Human-readable error message
    pub message: String,

    /// Error category for retry/routing decisions
    pub category: ErrorCategory,

    /// Error severity for logging/alerting
    pub severity: ErrorSeverity,

    /// Optional retry delay hint in milliseconds (for rate limits, etc.)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_after_ms: Option<u64>,

    /// Additional context attributes. Values may be any JSON type so HTTP
    /// errors can echo rich payloads (GraphQL `errors[]`, response bodies,
    /// nested diagnostics) without stringifying.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub attributes: HashMap<String, Value>,
}

impl AgentError {
    /// Create a transient error (retry likely to succeed).
    ///
    /// Use for: network failures, timeouts, rate limits, temporary unavailability.
    pub fn transient(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            category: ErrorCategory::Transient,
            severity: ErrorSeverity::Warning,
            retry_after_ms: None,
            attributes: HashMap::new(),
        }
    }

    /// Create a permanent error (don't auto-retry, human fix may help).
    ///
    /// Use for: 404 not found, validation errors, authentication failures, business rule violations.
    ///
    /// For business errors (expected outcomes like "credit limit exceeded"), use
    /// `.with_severity(ErrorSeverity::Warning)` to distinguish from technical failures.
    pub fn permanent(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            category: ErrorCategory::Permanent,
            severity: ErrorSeverity::Error,
            retry_after_ms: None,
            attributes: HashMap::new(),
        }
    }

    /// Set the error severity.
    pub fn with_severity(mut self, severity: ErrorSeverity) -> Self {
        self.severity = severity;
        self
    }

    /// Set a retry delay hint (for rate limits).
    pub fn with_retry_after(mut self, ms: u64) -> Self {
        self.retry_after_ms = Some(ms);
        self
    }

    /// Add a string-valued context attribute. Wraps the value as
    /// `Value::String` for wire-format continuity with the pre-widening
    /// format. Use [`with_attr_value`](Self::with_attr_value) when the
    /// attribute needs a richer shape (number, array, object, etc.).
    pub fn with_attr(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.attributes
            .insert(key.into(), Value::String(value.into()));
        self
    }

    /// Add a context attribute with an arbitrary JSON value. Use this when
    /// echoing structured payloads (GraphQL `errors[]`, HubSpot `paging`,
    /// nested diagnostics) that would lose fidelity if stringified.
    pub fn with_attr_value(mut self, key: impl Into<String>, value: impl Into<Value>) -> Self {
        self.attributes.insert(key.into(), value.into());
        self
    }

    /// Merge a `serde_json::Value::Object` into `attributes`. Non-object
    /// values are ignored. Used to migrate callers that built attribute
    /// maps as `json!({...})` literals without rewriting each key/value
    /// into a `.with_attr` chain.
    pub fn with_attrs(mut self, attributes: Value) -> Self {
        if let Some(obj) = attributes.as_object() {
            for (k, v) in obj {
                self.attributes.insert(k.clone(), v.clone());
            }
        }
        self
    }

    /// Should the `#[resilient]` macro retry this error?
    pub fn should_retry(&self) -> bool {
        self.category == ErrorCategory::Transient
    }
}

impl fmt::Display for AgentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}] {}", self.code, self.message)
    }
}

impl std::error::Error for AgentError {}

/// Convert AgentError to String for compatibility with the `#[capability]` macro.
/// The macro-generated executor wraps function results in `Result<Value, String>`,
/// so we need this conversion to use `?` with `Result<T, AgentError>` functions.
impl From<AgentError> for String {
    fn from(err: AgentError) -> Self {
        // Log structured error details before converting to string
        // Extract common attributes for OTEL filtering
        let status_code = err.attributes.get("status_code").and_then(|v| v.as_str());
        let url = err.attributes.get("url").and_then(|v| v.as_str());

        tracing::error!(
            error.code = %err.code,
            error.message = %err.message,
            error.category = ?err.category,
            error.severity = ?err.severity,
            http.status_code = status_code,
            http.url = url,
            "Agent error"
        );

        // Serialize the full AgentError to JSON for rich error information
        serde_json::to_string(&err).unwrap_or_else(|_| format!("[{}] {}", err.code, err.message))
    }
}

/// Classify an HTTP status code into an error category.
///
/// Classification logic:
/// - 408 Request Timeout → Transient (retry)
/// - 429 Too Many Requests → Transient (retry with backoff)
/// - 5xx Server Errors → Transient (retry)
/// - 4xx Client Errors → Permanent (don't auto-retry)
pub fn classify_http_status(status: u16) -> ErrorCategory {
    match status {
        // Explicitly transient 4xx codes
        408 => ErrorCategory::Transient, // Request Timeout
        429 => ErrorCategory::Transient, // Too Many Requests (rate limit)

        // All 5xx are transient (server issues)
        500..=599 => ErrorCategory::Transient,

        // Other 4xx are permanent (client errors)
        400..=499 => ErrorCategory::Permanent,

        // Anything else unexpected is permanent
        _ => ErrorCategory::Permanent,
    }
}

/// Parse the `Retry-After` response header into milliseconds.
///
/// Checks `retry-after-ms` first (precise, set by our proxy), then falls back
/// to the standard `Retry-After` header in seconds.
/// Returns `None` if both are absent or unparseable.
#[allow(clippy::collapsible_if)]
pub fn parse_retry_after_header(headers: &HashMap<String, String>) -> Option<u64> {
    // Prefer precise millisecond header from our proxy
    if let Some(ms_str) = headers
        .get("retry-after-ms")
        .or_else(|| headers.get("Retry-After-Ms"))
    {
        if let Ok(ms) = ms_str.trim().parse::<u64>() {
            return Some(ms);
        }
    }

    // Fall back to standard Retry-After header (seconds)
    let value = headers
        .get("retry-after")
        .or_else(|| headers.get("Retry-After"))?;
    // Try integer seconds (covers >99% of real-world APIs)
    if let Ok(seconds) = value.trim().parse::<u64>() {
        return Some(seconds.saturating_mul(1000));
    }
    // Try float seconds (some APIs like Stripe use e.g. "1.5")
    if let Ok(seconds) = value.trim().parse::<f64>() {
        if seconds > 0.0 && seconds.is_finite() {
            return Some((seconds * 1000.0) as u64);
        }
    }
    None
}

/// Create an AgentError from an HTTP response status.
///
/// Extracts retry-after header if present for rate limiting.
pub fn http_error(status: u16, body: impl Into<String>) -> AgentError {
    http_error_with_headers(status, body, None)
}

/// Create an AgentError from an HTTP response status, with optional response headers.
///
/// When headers are provided and the status is 429, extracts the `Retry-After` header
/// to populate `retry_after_ms` so the `#[resilient]` retry loop can honor server-specified delays.
pub fn http_error_with_headers(
    status: u16,
    body: impl Into<String>,
    headers: Option<&HashMap<String, String>>,
) -> AgentError {
    let category = classify_http_status(status);
    let body_text = body.into();

    let code = match status {
        400 => "HTTP_BAD_REQUEST",
        401 => "HTTP_UNAUTHORIZED",
        403 => "HTTP_FORBIDDEN",
        404 => "HTTP_NOT_FOUND",
        408 => "HTTP_TIMEOUT",
        429 => "HTTP_RATE_LIMITED",
        500 => "HTTP_INTERNAL_ERROR",
        502 => "HTTP_BAD_GATEWAY",
        503 => "HTTP_SERVICE_UNAVAILABLE",
        504 => "HTTP_GATEWAY_TIMEOUT",
        _ => "HTTP_ERROR",
    };

    let message = if body_text.is_empty() {
        format!("HTTP request failed with status {}", status)
    } else {
        format!("HTTP {} error: {}", status, body_text)
    };

    // Extract Retry-After for rate-limited responses
    let retry_after_ms = if status == 429 {
        headers.and_then(parse_retry_after_header)
    } else {
        None
    };

    AgentError {
        code: code.to_string(),
        message,
        category,
        severity: if category == ErrorCategory::Transient {
            ErrorSeverity::Warning
        } else {
            ErrorSeverity::Error
        },
        retry_after_ms,
        attributes: {
            let mut attrs = HashMap::new();
            // Keep status_code and retry_after_ms as string-typed Values for
            // wire-format continuity with the pre-widening format. New
            // attribute keys are free to use richer Value shapes.
            attrs.insert("status_code".to_string(), status.to_string().into());
            if let Some(ms) = retry_after_ms {
                attrs.insert("retry_after_ms".to_string(), ms.to_string().into());
            }
            attrs
        },
    }
}

/// Create an AgentError from a network/connection failure.
pub fn network_error(message: impl Into<String>) -> AgentError {
    AgentError::transient("NETWORK_ERROR", message).with_severity(ErrorSeverity::Warning)
}

/// Create an AgentError from a timeout.
pub fn timeout_error(message: impl Into<String>) -> AgentError {
    AgentError::transient("TIMEOUT", message).with_severity(ErrorSeverity::Warning)
}

/// Well-known attribute keys used by the platform. Centralized so every
/// producer spells them the same way and every consumer (UI, observability,
/// retry loop) can look them up by constant rather than string literal.
pub mod attrs {
    /// HTTP response status code.
    pub const STATUS_CODE: &str = "status_code";
    /// Echoed HTTP response body (may be truncated for very large payloads).
    pub const BODY: &str = "body";
    /// Request URL.
    pub const URL: &str = "url";
    /// Request path (relative to the integration base URL).
    pub const PATH: &str = "path";
    /// HTTP method.
    pub const METHOD: &str = "method";
    /// Integration prefix (e.g. `"SHOPIFY"`, `"STRIPE"`).
    pub const INTEGRATION: &str = "integration";
    /// Retry-after hint duplicated into attributes for legacy consumers.
    /// Prefer the typed [`AgentError::retry_after_ms`] field — which is
    /// what the `#[resilient]` retry loop actually reads.
    pub const RETRY_AFTER_MS: &str = "retry_after_ms";
    /// Nested validation details (typically an object or array).
    pub const DETAILS: &str = "details";
    /// Payload echo for validation errors originating from capability
    /// inputs.
    pub const PAYLOAD: &str = "payload";
    /// Name of the missing / invalid field.
    pub const FIELD: &str = "field";
}

/// Integration-namespaced HTTP error constructors.
///
/// These produce [`AgentError`] values with codes of the form
/// `{PREFIX}_{KIND}` (e.g. `SHOPIFY_UNAUTHORIZED`) so integrations can
/// distinguish their failure modes from each other while sharing the
/// same classification logic. Prefer these over the generic
/// [`http_error`] helper when the caller knows which integration the
/// failure came from.
///
/// Every constructor populates [`attrs::STATUS_CODE`] (where applicable)
/// and [`attrs::BODY`] consistently. Rate-limited errors additionally
/// set the typed [`AgentError::retry_after_ms`] field so the
/// `#[resilient]` retry loop can honor server-specified delays.
pub mod http {
    use super::{
        AgentError, ErrorCategory, ErrorSeverity, Value, attrs, classify_http_status,
        parse_retry_after_header,
    };
    use std::collections::HashMap;

    fn make_code(prefix: &str, kind: &str) -> String {
        format!("{}_{}", prefix, kind)
    }

    fn default_message(prefix: &str, status: u16, body: &str) -> String {
        if body.is_empty() {
            format!("{} request failed with status {}", prefix, status)
        } else {
            format!("{} {} error: {}", prefix, status, body)
        }
    }

    /// Build an [`AgentError`] from any non-success HTTP response.
    ///
    /// Dispatches to the variant-specific constructors below based on
    /// `status`. On 429 the `headers` map is consulted for
    /// `Retry-After` / `Retry-After-Ms`, which feeds the typed
    /// [`AgentError::retry_after_ms`] field.
    pub fn classify_response(
        prefix: &str,
        status: u16,
        body: impl Into<String>,
        headers: &HashMap<String, String>,
    ) -> AgentError {
        let body = body.into();
        match status {
            401 => unauthorized(prefix, body),
            403 => forbidden(prefix, body),
            404 => not_found(prefix, body),
            429 => rate_limited(prefix, parse_retry_after_header(headers), body),
            408 | 500..=599 => upstream(prefix, status, body),
            _ => other(prefix, status, body),
        }
    }

    /// HTTP 401 Unauthorized — permanent, typically requires credential refresh.
    pub fn unauthorized(prefix: &str, body: impl Into<String>) -> AgentError {
        let body = body.into();
        let message = default_message(prefix, 401, &body);
        AgentError::permanent(make_code(prefix, "UNAUTHORIZED"), message)
            .with_attr(attrs::STATUS_CODE, "401")
            .with_attr(attrs::BODY, body)
            .with_attr(attrs::INTEGRATION, prefix.to_string())
    }

    /// HTTP 403 Forbidden — permanent, typically a scope / permission issue.
    pub fn forbidden(prefix: &str, body: impl Into<String>) -> AgentError {
        let body = body.into();
        let message = default_message(prefix, 403, &body);
        AgentError::permanent(make_code(prefix, "FORBIDDEN"), message)
            .with_attr(attrs::STATUS_CODE, "403")
            .with_attr(attrs::BODY, body)
            .with_attr(attrs::INTEGRATION, prefix.to_string())
    }

    /// HTTP 404 Not Found — permanent.
    pub fn not_found(prefix: &str, body: impl Into<String>) -> AgentError {
        let body = body.into();
        let message = default_message(prefix, 404, &body);
        AgentError::permanent(make_code(prefix, "NOT_FOUND"), message)
            .with_attr(attrs::STATUS_CODE, "404")
            .with_attr(attrs::BODY, body)
            .with_attr(attrs::INTEGRATION, prefix.to_string())
    }

    /// HTTP 429 Too Many Requests — transient. Sets the typed
    /// [`AgentError::retry_after_ms`] field so the durable retry loop
    /// honors server-specified delays; also duplicates the value into
    /// [`attrs::RETRY_AFTER_MS`] for legacy consumers.
    pub fn rate_limited(
        prefix: &str,
        retry_after_ms: Option<u64>,
        body: impl Into<String>,
    ) -> AgentError {
        let body = body.into();
        let message = default_message(prefix, 429, &body);
        let mut err = AgentError::transient(make_code(prefix, "RATE_LIMITED"), message)
            .with_attr(attrs::STATUS_CODE, "429")
            .with_attr(attrs::BODY, body)
            .with_attr(attrs::INTEGRATION, prefix.to_string());
        if let Some(ms) = retry_after_ms {
            err = err
                .with_retry_after(ms)
                .with_attr(attrs::RETRY_AFTER_MS, ms.to_string());
        }
        err
    }

    /// HTTP 408 / 5xx — transient upstream failure.
    pub fn upstream(prefix: &str, status: u16, body: impl Into<String>) -> AgentError {
        let body = body.into();
        let message = default_message(prefix, status, &body);
        AgentError::transient(make_code(prefix, "UPSTREAM_ERROR"), message)
            .with_severity(ErrorSeverity::Warning)
            .with_attr(attrs::STATUS_CODE, status.to_string())
            .with_attr(attrs::BODY, body)
            .with_attr(attrs::INTEGRATION, prefix.to_string())
    }

    /// Any other non-success status (unusual 4xx codes, 3xx treated as
    /// errors by the caller, etc.). Classified by
    /// [`classify_http_status`].
    pub fn other(prefix: &str, status: u16, body: impl Into<String>) -> AgentError {
        let body = body.into();
        let message = default_message(prefix, status, &body);
        let category = classify_http_status(status);
        let (code_suffix, severity) = match category {
            ErrorCategory::Transient => ("UPSTREAM_ERROR", ErrorSeverity::Warning),
            ErrorCategory::Permanent => ("CLIENT_ERROR", ErrorSeverity::Error),
        };
        let err = if category == ErrorCategory::Transient {
            AgentError::transient(make_code(prefix, code_suffix), message)
        } else {
            AgentError::permanent(make_code(prefix, code_suffix), message)
        };
        err.with_severity(severity)
            .with_attr(attrs::STATUS_CODE, status.to_string())
            .with_attr(attrs::BODY, body)
            .with_attr(attrs::INTEGRATION, prefix.to_string())
    }

    /// Transport / DNS / connection failure — no HTTP status was received.
    pub fn network(prefix: &str, message: impl Into<String>) -> AgentError {
        AgentError::transient(
            make_code(prefix, "NETWORK_ERROR"),
            format!("{} network failure: {}", prefix, message.into()),
        )
        .with_severity(ErrorSeverity::Warning)
        .with_attr(attrs::INTEGRATION, prefix.to_string())
    }

    /// Response was received but could not be parsed as expected.
    pub fn deserialization(prefix: &str, message: impl Into<String>) -> AgentError {
        AgentError::permanent(
            make_code(prefix, "INVALID_RESPONSE"),
            format!("{} invalid response: {}", prefix, message.into()),
        )
        .with_attr(attrs::INTEGRATION, prefix.to_string())
    }

    /// Integration-specific domain error that doesn't map to an HTTP
    /// status (e.g. GraphQL `errors[]`, `userErrors[]`, vendor error
    /// body codes). Attach structured context via
    /// [`AgentError::with_attr_value`] on the returned value.
    pub fn domain(
        prefix: &str,
        code_suffix: &str,
        category: ErrorCategory,
        message: impl Into<String>,
    ) -> AgentError {
        let code = make_code(prefix, code_suffix);
        let err = match category {
            ErrorCategory::Transient => AgentError::transient(code, message),
            _ => AgentError::permanent(code, message),
        };
        err.with_attr(attrs::INTEGRATION, prefix.to_string())
    }

    /// Ensure a string value fits in an attribute without bloating the
    /// error. Used by response-body echo: long bodies get truncated
    /// with an ellipsis so downstream log pipelines aren't swamped.
    pub fn truncate_body(body: String, max: usize) -> Value {
        if body.len() <= max {
            Value::String(body)
        } else {
            let mut trimmed = body;
            trimmed.truncate(max);
            trimmed.push_str("…[truncated]");
            Value::String(trimmed)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ========================================================================
    // HTTP Status Classification Tests
    // ========================================================================

    #[test]
    fn test_classify_http_status_transient_408() {
        assert_eq!(classify_http_status(408), ErrorCategory::Transient);
    }

    #[test]
    fn test_classify_http_status_transient_429() {
        assert_eq!(classify_http_status(429), ErrorCategory::Transient);
    }

    #[test]
    fn test_classify_http_status_transient_5xx() {
        assert_eq!(classify_http_status(500), ErrorCategory::Transient);
        assert_eq!(classify_http_status(502), ErrorCategory::Transient);
        assert_eq!(classify_http_status(503), ErrorCategory::Transient);
        assert_eq!(classify_http_status(504), ErrorCategory::Transient);
        assert_eq!(classify_http_status(599), ErrorCategory::Transient);
    }

    #[test]
    fn test_classify_http_status_permanent_4xx() {
        assert_eq!(classify_http_status(400), ErrorCategory::Permanent);
        assert_eq!(classify_http_status(401), ErrorCategory::Permanent);
        assert_eq!(classify_http_status(403), ErrorCategory::Permanent);
        assert_eq!(classify_http_status(404), ErrorCategory::Permanent);
        assert_eq!(classify_http_status(422), ErrorCategory::Permanent);
    }

    #[test]
    fn test_classify_http_status_unknown() {
        // Unexpected status codes default to permanent
        assert_eq!(classify_http_status(100), ErrorCategory::Permanent);
        assert_eq!(classify_http_status(200), ErrorCategory::Permanent);
        assert_eq!(classify_http_status(301), ErrorCategory::Permanent);
    }

    // ========================================================================
    // AgentError Construction Tests
    // ========================================================================

    #[test]
    fn test_agent_error_transient() {
        let err = AgentError::transient("NETWORK_ERROR", "Connection refused");
        assert_eq!(err.code, "NETWORK_ERROR");
        assert_eq!(err.message, "Connection refused");
        assert_eq!(err.category, ErrorCategory::Transient);
        assert_eq!(err.severity, ErrorSeverity::Warning);
        assert!(err.should_retry());
    }

    #[test]
    fn test_agent_error_permanent() {
        let err = AgentError::permanent("NOT_FOUND", "Resource not found");
        assert_eq!(err.code, "NOT_FOUND");
        assert_eq!(err.category, ErrorCategory::Permanent);
        assert_eq!(err.severity, ErrorSeverity::Error);
        assert!(!err.should_retry());
    }

    #[test]
    fn test_agent_error_business_pattern() {
        // Business errors are permanent errors with Warning severity
        // This pattern allows distinguishing technical failures from expected business outcomes
        let err = AgentError::permanent("CREDIT_LIMIT_EXCEEDED", "Credit limit exceeded")
            .with_severity(ErrorSeverity::Warning)
            .with_attr("order_amount", "5000")
            .with_attr("credit_limit", "3000");

        assert_eq!(err.code, "CREDIT_LIMIT_EXCEEDED");
        assert_eq!(err.category, ErrorCategory::Permanent);
        assert_eq!(err.severity, ErrorSeverity::Warning); // Warning = expected business outcome
        assert!(!err.should_retry());
        assert_eq!(
            err.attributes.get("order_amount").and_then(|v| v.as_str()),
            Some("5000")
        );
    }

    #[test]
    fn test_agent_error_with_attrs() {
        let err = AgentError::transient("TEST", "test")
            .with_attr("url", "https://example.com")
            .with_attr("method", "GET");

        assert_eq!(
            err.attributes.get("url").and_then(|v| v.as_str()),
            Some("https://example.com")
        );
        assert_eq!(
            err.attributes.get("method").and_then(|v| v.as_str()),
            Some("GET")
        );
    }

    #[test]
    fn test_agent_error_with_retry_after() {
        let err = AgentError::transient("RATE_LIMITED", "Too many requests").with_retry_after(5000);

        assert_eq!(err.retry_after_ms, Some(5000));
    }

    #[test]
    fn test_agent_error_with_severity() {
        let err =
            AgentError::transient("ERROR", "Something bad").with_severity(ErrorSeverity::Critical);

        assert_eq!(err.severity, ErrorSeverity::Critical);
    }

    // ========================================================================
    // HTTP Error Helper Tests
    // ========================================================================

    #[test]
    fn test_http_error_404() {
        let err = http_error(404, "Not found");
        assert_eq!(err.code, "HTTP_NOT_FOUND");
        assert_eq!(err.category, ErrorCategory::Permanent);
        assert_eq!(err.severity, ErrorSeverity::Error);
        assert_eq!(
            err.attributes.get("status_code").and_then(|v| v.as_str()),
            Some("404")
        );
    }

    #[test]
    fn test_http_error_429() {
        let err = http_error(429, "Rate limited");
        assert_eq!(err.code, "HTTP_RATE_LIMITED");
        assert_eq!(err.category, ErrorCategory::Transient);
        assert_eq!(err.severity, ErrorSeverity::Warning);
    }

    #[test]
    fn test_http_error_500() {
        let err = http_error(500, "Internal server error");
        assert_eq!(err.code, "HTTP_INTERNAL_ERROR");
        assert_eq!(err.category, ErrorCategory::Transient);
        assert_eq!(err.severity, ErrorSeverity::Warning);
    }

    #[test]
    fn test_http_error_empty_body() {
        let err = http_error(503, "");
        assert_eq!(err.message, "HTTP request failed with status 503");
    }

    #[test]
    fn test_http_error_with_body() {
        let err = http_error(400, "Invalid JSON");
        assert_eq!(err.message, "HTTP 400 error: Invalid JSON");
    }

    // ========================================================================
    // Helper Function Tests
    // ========================================================================

    #[test]
    fn test_network_error() {
        let err = network_error("Connection refused");
        assert_eq!(err.code, "NETWORK_ERROR");
        assert_eq!(err.category, ErrorCategory::Transient);
        assert!(err.should_retry());
    }

    #[test]
    fn test_timeout_error() {
        let err = timeout_error("Request timed out after 30s");
        assert_eq!(err.code, "TIMEOUT");
        assert_eq!(err.category, ErrorCategory::Transient);
        assert!(err.should_retry());
    }

    // ========================================================================
    // Serialization Tests
    // ========================================================================

    #[test]
    fn test_agent_error_serialization() {
        let err = AgentError::transient("TEST", "Test error").with_attr("key", "value");

        let json = serde_json::to_string(&err).unwrap();
        let parsed: AgentError = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.code, err.code);
        assert_eq!(parsed.message, err.message);
        assert_eq!(parsed.category, err.category);
        assert_eq!(
            parsed.attributes.get("key").and_then(|v| v.as_str()),
            Some("value")
        );
    }

    #[test]
    fn test_agent_error_display() {
        let err = AgentError::permanent("NOT_FOUND", "Resource not found");
        assert_eq!(format!("{}", err), "[NOT_FOUND] Resource not found");
    }

    // ========================================================================
    // Namespaced http::* helper tests
    // ========================================================================

    #[test]
    fn http_unauthorized_uses_prefix_and_carries_body() {
        let err = http::unauthorized("SHOPIFY", "Invalid token".to_string());
        assert_eq!(err.code, "SHOPIFY_UNAUTHORIZED");
        assert_eq!(err.category, ErrorCategory::Permanent);
        assert_eq!(
            err.attributes
                .get(attrs::STATUS_CODE)
                .and_then(|v| v.as_str()),
            Some("401")
        );
        assert_eq!(
            err.attributes.get(attrs::BODY).and_then(|v| v.as_str()),
            Some("Invalid token")
        );
        assert_eq!(
            err.attributes
                .get(attrs::INTEGRATION)
                .and_then(|v| v.as_str()),
            Some("SHOPIFY")
        );
    }

    #[test]
    fn http_rate_limited_populates_typed_retry_after_ms_field() {
        // Critical: the #[resilient] retry loop reads top-level `retryAfterMs`
        // (camelCase) from the serialized error JSON. This test locks in
        // that wire-format contract — regressing it silently drops server
        // rate-limit hints.
        let err = http::rate_limited("STRIPE", Some(1500), "Too many requests".to_string());
        assert_eq!(err.code, "STRIPE_RATE_LIMITED");
        assert_eq!(err.category, ErrorCategory::Transient);
        assert_eq!(err.retry_after_ms, Some(1500));

        let json = serde_json::to_string(&err).unwrap();
        let parsed: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["retryAfterMs"], 1500);
        assert_eq!(parsed["category"], "transient");
        assert_eq!(parsed["code"], "STRIPE_RATE_LIMITED");
    }

    #[test]
    fn http_rate_limited_without_retry_after_omits_typed_field() {
        let err = http::rate_limited("STRIPE", None, "".to_string());
        assert_eq!(err.retry_after_ms, None);
        let json = serde_json::to_string(&err).unwrap();
        let parsed: Value = serde_json::from_str(&json).unwrap();
        assert!(parsed.get("retryAfterMs").is_none());
    }

    #[test]
    fn http_classify_response_dispatches_by_status() {
        let mut headers = std::collections::HashMap::new();
        headers.insert("retry-after-ms".to_string(), "2500".to_string());

        let err = http::classify_response("HUBSPOT", 429, "rate limit".to_string(), &headers);
        assert_eq!(err.code, "HUBSPOT_RATE_LIMITED");
        assert_eq!(err.retry_after_ms, Some(2500));

        let err = http::classify_response("HUBSPOT", 404, "".to_string(), &headers);
        assert_eq!(err.code, "HUBSPOT_NOT_FOUND");

        let err = http::classify_response("HUBSPOT", 503, "down".to_string(), &headers);
        assert_eq!(err.code, "HUBSPOT_UPSTREAM_ERROR");
        assert_eq!(err.category, ErrorCategory::Transient);
    }

    #[test]
    fn http_domain_supports_non_http_classification() {
        // Shopify GraphQL errors arrive on a 200 response body — `domain`
        // handles that case while still producing a consistent AgentError.
        let err = http::domain(
            "SHOPIFY",
            "GRAPHQL_ERROR",
            ErrorCategory::Permanent,
            "Field 'foo' not found",
        )
        .with_attr_value(
            attrs::DETAILS,
            serde_json::json!([{"message": "Field 'foo' not found"}]),
        );
        assert_eq!(err.code, "SHOPIFY_GRAPHQL_ERROR");
        assert_eq!(err.category, ErrorCategory::Permanent);
        assert!(err.attributes.contains_key(attrs::DETAILS));
    }

    #[test]
    fn http_network_is_transient() {
        let err = http::network("HUBSPOT", "DNS failure");
        assert_eq!(err.code, "HUBSPOT_NETWORK_ERROR");
        assert_eq!(err.category, ErrorCategory::Transient);
        assert!(err.should_retry());
    }

    #[test]
    fn http_deserialization_is_permanent() {
        let err = http::deserialization("OPENAI", "expected field 'choices'");
        assert_eq!(err.code, "OPENAI_INVALID_RESPONSE");
        assert_eq!(err.category, ErrorCategory::Permanent);
    }

    #[test]
    fn attrs_constants_match_expected_wire_keys() {
        // Downstream consumers (UI, observability) read these keys by
        // string. Lock them in so renames are intentional.
        assert_eq!(attrs::STATUS_CODE, "status_code");
        assert_eq!(attrs::BODY, "body");
        assert_eq!(attrs::RETRY_AFTER_MS, "retry_after_ms");
        assert_eq!(attrs::INTEGRATION, "integration");
    }
}
