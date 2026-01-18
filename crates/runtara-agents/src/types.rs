// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Shared types used across agents

use base64::{Engine as _, engine::general_purpose};
use runtara_agent_macro::CapabilityOutput;
use runtara_dsl::{ErrorCategory, ErrorSeverity};
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
/// - **Transient**: Temporary failures (network, timeout, rate limit) - `#[durable]` retries
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

    /// Additional context attributes
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub attributes: HashMap<String, String>,
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

    /// Add a context attribute.
    pub fn with_attr(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.attributes.insert(key.into(), value.into());
        self
    }

    /// Should the `#[durable]` macro retry this error?
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

/// Create an AgentError from an HTTP response status.
///
/// Extracts retry-after header if present for rate limiting.
pub fn http_error(status: u16, body: impl Into<String>) -> AgentError {
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

    AgentError {
        code: code.to_string(),
        message,
        category,
        severity: if category == ErrorCategory::Transient {
            ErrorSeverity::Warning
        } else {
            ErrorSeverity::Error
        },
        retry_after_ms: None,
        attributes: {
            let mut attrs = HashMap::new();
            attrs.insert("status_code".to_string(), status.to_string());
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
            err.attributes.get("order_amount"),
            Some(&"5000".to_string())
        );
    }

    #[test]
    fn test_agent_error_with_attrs() {
        let err = AgentError::transient("TEST", "test")
            .with_attr("url", "https://example.com")
            .with_attr("method", "GET");

        assert_eq!(
            err.attributes.get("url"),
            Some(&"https://example.com".to_string())
        );
        assert_eq!(err.attributes.get("method"), Some(&"GET".to_string()));
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
        assert_eq!(err.attributes.get("status_code"), Some(&"404".to_string()));
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
        assert_eq!(parsed.attributes.get("key"), Some(&"value".to_string()));
    }

    #[test]
    fn test_agent_error_display() {
        let err = AgentError::permanent("NOT_FOUND", "Resource not found");
        assert_eq!(format!("{}", err), "[NOT_FOUND] Resource not found");
    }
}
