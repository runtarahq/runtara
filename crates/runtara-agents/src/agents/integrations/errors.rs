//! Structured error helpers for SMO agents.
//!
//! This module provides helper functions for constructing JSON-serialized structured errors
//! that follow the format documented in docs/structured-errors.md.
//!
//! # Error Format
//!
//! All errors are JSON-serialized with the following structure:
//! ```json
//! {
//!     "code": "ERROR_CODE",
//!     "message": "Human-readable description",
//!     "category": "transient" | "permanent",
//!     "severity": "error" | "warning",
//!     "attributes": { ... }
//! }
//! ```

use serde_json::{Value, json};

/// Create a structured error with full control over all fields.
///
/// # Arguments
/// * `code` - Unique error code (e.g., "OPENAI_RATE_LIMITED")
/// * `message` - Human-readable error description
/// * `category` - Either "transient" (retryable) or "permanent" (not retryable)
/// * `severity` - Either "error" or "warning"
/// * `attributes` - Additional context as JSON value
pub fn structured_error(
    code: &str,
    message: &str,
    category: &str,
    severity: &str,
    attributes: Value,
) -> String {
    json!({
        "code": code,
        "message": message,
        "category": category,
        "severity": severity,
        "attributes": attributes
    })
    .to_string()
}

/// Create a transient error (retryable).
///
/// Used for temporary failures like rate limiting, network issues, or server errors.
/// These errors may succeed if retried after some delay.
pub fn transient_error(code: &str, message: &str, attributes: Value) -> String {
    structured_error(code, message, "transient", "error", attributes)
}

/// Create a permanent error (not retryable).
///
/// Used for errors that won't be resolved by retrying, such as invalid credentials,
/// malformed requests, or resource not found.
pub fn permanent_error(code: &str, message: &str, attributes: Value) -> String {
    structured_error(code, message, "permanent", "error", attributes)
}

/// Classify an HTTP status code as transient or permanent.
///
/// Following the HTTP agent pattern:
/// - Transient: 408 (Request Timeout), 429 (Too Many Requests), 5xx (Server Errors)
/// - Permanent: 4xx (Client Errors)
pub fn is_transient_status(status_code: u16) -> bool {
    matches!(status_code, 408 | 429 | 500..=599)
}

/// Create an error based on HTTP status code, automatically categorizing as transient or permanent.
pub fn http_status_error(
    code_prefix: &str,
    status_code: u16,
    message: &str,
    attributes: Value,
) -> String {
    let (category, code_suffix) = match status_code {
        401 => ("permanent", "UNAUTHORIZED"),
        403 => ("permanent", "FORBIDDEN"),
        404 => ("permanent", "NOT_FOUND"),
        408 => ("transient", "TIMEOUT"),
        429 => ("transient", "RATE_LIMITED"),
        500..=599 => ("transient", "SERVER_ERROR"),
        _ => ("permanent", "CLIENT_ERROR"),
    };

    let code = format!("{}_{}", code_prefix, code_suffix);
    structured_error(&code, message, category, "error", attributes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_structured_error() {
        let err = structured_error(
            "TEST_CODE",
            "Test message",
            "transient",
            "error",
            json!({"key": "value"}),
        );
        let parsed: Value = serde_json::from_str(&err).unwrap();
        assert_eq!(parsed["code"], "TEST_CODE");
        assert_eq!(parsed["message"], "Test message");
        assert_eq!(parsed["category"], "transient");
        assert_eq!(parsed["severity"], "error");
        assert_eq!(parsed["attributes"]["key"], "value");
    }

    #[test]
    fn test_transient_error() {
        let err = transient_error(
            "RATE_LIMITED",
            "Too many requests",
            json!({"retry_after": 60}),
        );
        let parsed: Value = serde_json::from_str(&err).unwrap();
        assert_eq!(parsed["category"], "transient");
    }

    #[test]
    fn test_permanent_error() {
        let err = permanent_error("INVALID_KEY", "API key is invalid", json!({}));
        let parsed: Value = serde_json::from_str(&err).unwrap();
        assert_eq!(parsed["category"], "permanent");
    }

    #[test]
    fn test_http_status_error() {
        let err = http_status_error("OPENAI", 429, "Rate limited", json!({"status_code": 429}));
        let parsed: Value = serde_json::from_str(&err).unwrap();
        assert_eq!(parsed["code"], "OPENAI_RATE_LIMITED");
        assert_eq!(parsed["category"], "transient");

        let err = http_status_error("OPENAI", 401, "Unauthorized", json!({"status_code": 401}));
        let parsed: Value = serde_json::from_str(&err).unwrap();
        assert_eq!(parsed["code"], "OPENAI_UNAUTHORIZED");
        assert_eq!(parsed["category"], "permanent");
    }

    #[test]
    fn test_is_transient_status() {
        assert!(is_transient_status(408));
        assert!(is_transient_status(429));
        assert!(is_transient_status(500));
        assert!(is_transient_status(502));
        assert!(is_transient_status(503));
        assert!(is_transient_status(504));
        assert!(!is_transient_status(400));
        assert!(!is_transient_status(401));
        assert!(!is_transient_status(404));
    }
}
