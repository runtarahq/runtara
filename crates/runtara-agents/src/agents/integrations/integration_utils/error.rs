// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Typed integration errors with exact wire-format fidelity.
//!
//! Capability functions must return `Result<_, String>`, where the string is
//! a JSON-encoded structured error (see `docs/structured-errors.md`). Today
//! each integration builds those strings by calling `errors::structured_error`
//! / `errors::http_status_error` inline, which hides intent and invites drift.
//!
//! `IntegrationError` captures the same shape as a typed enum. Calling
//! `into_structured()` (or relying on the `From<IntegrationError> for String`
//! impl) produces the exact same JSON wire format the integrations produce
//! today — with one intentional enhancement: on 429 responses we now embed
//! the parsed `Retry-After` value as `attributes.retry_after_ms` so the
//! `#[durable]` retry loop can honor it.

use std::fmt;

use serde_json::{Value, json};

use super::super::errors::{http_status_error, permanent_error, structured_error, transient_error};

/// Typed integration error. Convertible back into the JSON-as-string wire
/// format via [`IntegrationError::into_structured`].
#[derive(Debug, Clone)]
pub enum IntegrationError {
    /// HTTP 401 Unauthorized.
    Unauthorized {
        prefix: &'static str,
        status: u16,
        body: String,
    },

    /// HTTP 403 Forbidden.
    Forbidden {
        prefix: &'static str,
        status: u16,
        body: String,
    },

    /// HTTP 404 Not Found.
    NotFound {
        prefix: &'static str,
        status: u16,
        body: String,
    },

    /// HTTP 429 Too Many Requests. `retry_after_ms` preserves the
    /// `Retry-After` / `Retry-After-Ms` signal (parsed by
    /// `crate::types::parse_retry_after_header`).
    RateLimited {
        prefix: &'static str,
        status: u16,
        body: String,
        retry_after_ms: Option<u64>,
    },

    /// A structured validation / business-logic error, not tied to a raw HTTP status.
    Validation {
        prefix: &'static str,
        message: String,
        details: Value,
    },

    /// HTTP upstream error (5xx, 408). Classified transient.
    Upstream {
        prefix: &'static str,
        status: u16,
        body: String,
    },

    /// Network / transport failure (connection refused, DNS, etc).
    Network {
        prefix: &'static str,
        message: String,
    },

    /// Failed to deserialize or interpret a successful response.
    Deserialization {
        prefix: &'static str,
        message: String,
    },

    /// The capability was invoked without a required connection.
    NoConnection { prefix: &'static str },

    /// A required field was missing from the input payload.
    MissingField {
        prefix: &'static str,
        field: &'static str,
        payload: Value,
    },

    /// Escape hatch for integration-specific error codes (e.g. Slack's
    /// `channel_not_found`). Produces the same JSON shape as
    /// `errors::permanent_error` / `errors::transient_error`.
    Unknown {
        prefix: &'static str,
        code: String,
        message: String,
        category: ErrorCategory,
        attributes: Value,
    },
}

impl fmt::Display for IntegrationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            IntegrationError::Unauthorized { prefix, status, .. } => {
                write!(f, "{} unauthorized (status {})", prefix, status)
            }
            IntegrationError::Forbidden { prefix, status, .. } => {
                write!(f, "{} forbidden (status {})", prefix, status)
            }
            IntegrationError::NotFound { prefix, status, .. } => {
                write!(f, "{} not found (status {})", prefix, status)
            }
            IntegrationError::RateLimited { prefix, status, .. } => {
                write!(f, "{} rate limited (status {})", prefix, status)
            }
            IntegrationError::Validation {
                prefix, message, ..
            } => {
                write!(f, "{} validation error: {}", prefix, message)
            }
            IntegrationError::Upstream { prefix, status, .. } => {
                write!(f, "{} upstream error (status {})", prefix, status)
            }
            IntegrationError::Network { prefix, message } => {
                write!(f, "{} network error: {}", prefix, message)
            }
            IntegrationError::Deserialization { prefix, message } => {
                write!(f, "{} deserialization error: {}", prefix, message)
            }
            IntegrationError::NoConnection { prefix } => {
                write!(f, "{} no connection configured", prefix)
            }
            IntegrationError::MissingField { prefix, field, .. } => {
                write!(f, "{} missing field: {}", prefix, field)
            }
            IntegrationError::Unknown {
                prefix, message, ..
            } => {
                write!(f, "{} error: {}", prefix, message)
            }
        }
    }
}

impl std::error::Error for IntegrationError {}

/// Error category parallel to `errors::structured_error`'s string field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCategory {
    Transient,
    Permanent,
}

impl ErrorCategory {
    fn as_str(&self) -> &'static str {
        match self {
            ErrorCategory::Transient => "transient",
            ErrorCategory::Permanent => "permanent",
        }
    }
}

impl IntegrationError {
    /// Serialize this error into the JSON-as-string wire format the
    /// capability layer currently emits.
    pub fn into_structured(self) -> String {
        match self {
            IntegrationError::Unauthorized {
                prefix,
                status,
                body,
            } => http_status_error(
                prefix,
                status,
                &format!("{} API error: {}", prefix, body),
                json!({ "status_code": status, "body": body }),
            ),
            IntegrationError::Forbidden {
                prefix,
                status,
                body,
            } => http_status_error(
                prefix,
                status,
                &format!("{} API error: {}", prefix, body),
                json!({ "status_code": status, "body": body }),
            ),
            IntegrationError::NotFound {
                prefix,
                status,
                body,
            } => http_status_error(
                prefix,
                status,
                &format!("{} API error: {}", prefix, body),
                json!({ "status_code": status, "body": body }),
            ),
            IntegrationError::RateLimited {
                prefix,
                status,
                body,
                retry_after_ms,
            } => {
                let mut attrs = json!({ "status_code": status, "body": body });
                if let Some(ms) = retry_after_ms {
                    // Safe: attrs was just built as a JSON object.
                    attrs
                        .as_object_mut()
                        .unwrap()
                        .insert("retry_after_ms".to_string(), json!(ms));
                }
                http_status_error(
                    prefix,
                    status,
                    &format!("{} API error: {}", prefix, body),
                    attrs,
                )
            }
            IntegrationError::Upstream {
                prefix,
                status,
                body,
            } => http_status_error(
                prefix,
                status,
                &format!("{} API error: {}", prefix, body),
                json!({ "status_code": status, "body": body }),
            ),
            IntegrationError::Validation {
                prefix,
                message,
                details,
            } => permanent_error(&format!("{}_VALIDATION_ERROR", prefix), &message, details),
            IntegrationError::Network { prefix, message } => transient_error(
                &format!("{}_NETWORK_ERROR", prefix),
                &message,
                json!({ "message": message }),
            ),
            IntegrationError::Deserialization { prefix, message } => permanent_error(
                &format!("{}_INVALID_RESPONSE", prefix),
                &message,
                json!({ "message": message }),
            ),
            IntegrationError::NoConnection { prefix } => permanent_error(
                &format!("{}_NO_CONNECTION", prefix),
                &format!("Connection is required for {} operations", prefix),
                json!({}),
            ),
            IntegrationError::MissingField {
                prefix,
                field,
                payload,
            } => permanent_error(
                &format!("{}_MISSING_FIELD", prefix),
                &format!("Missing required field '{}'", field),
                json!({ "field": field, "payload": payload }),
            ),
            IntegrationError::Unknown {
                prefix: _,
                code,
                message,
                category,
                attributes,
            } => structured_error(&code, &message, category.as_str(), "error", attributes),
        }
    }
}

impl From<IntegrationError> for String {
    fn from(e: IntegrationError) -> Self {
        e.into_structured()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(s: &str) -> Value {
        serde_json::from_str(s).unwrap_or_else(|e| panic!("not JSON: {} ({})", s, e))
    }

    #[test]
    fn unauthorized_round_trip() {
        let err = IntegrationError::Unauthorized {
            prefix: "HUBSPOT",
            status: 401,
            body: "invalid token".into(),
        };
        let v = parse(&err.into_structured());
        assert_eq!(v["code"], "HUBSPOT_UNAUTHORIZED");
        assert_eq!(v["category"], "permanent");
        assert_eq!(v["severity"], "error");
        assert_eq!(v["attributes"]["status_code"], 401);
        assert_eq!(v["attributes"]["body"], "invalid token");
    }

    #[test]
    fn forbidden_round_trip() {
        let err = IntegrationError::Forbidden {
            prefix: "SHOPIFY",
            status: 403,
            body: "no scope".into(),
        };
        let v = parse(&err.into_structured());
        assert_eq!(v["code"], "SHOPIFY_FORBIDDEN");
        assert_eq!(v["category"], "permanent");
        assert_eq!(v["attributes"]["status_code"], 403);
    }

    #[test]
    fn not_found_round_trip() {
        let err = IntegrationError::NotFound {
            prefix: "STRIPE",
            status: 404,
            body: "{}".into(),
        };
        let v = parse(&err.into_structured());
        assert_eq!(v["code"], "STRIPE_NOT_FOUND");
        assert_eq!(v["category"], "permanent");
    }

    #[test]
    fn rate_limited_round_trip_with_retry_after() {
        let err = IntegrationError::RateLimited {
            prefix: "OPENAI",
            status: 429,
            body: "slow down".into(),
            retry_after_ms: Some(1500),
        };
        let v = parse(&err.into_structured());
        assert_eq!(v["code"], "OPENAI_RATE_LIMITED");
        assert_eq!(v["category"], "transient");
        assert_eq!(v["attributes"]["status_code"], 429);
        assert_eq!(v["attributes"]["retry_after_ms"], 1500);
    }

    #[test]
    fn rate_limited_round_trip_without_retry_after() {
        let err = IntegrationError::RateLimited {
            prefix: "OPENAI",
            status: 429,
            body: "slow down".into(),
            retry_after_ms: None,
        };
        let v = parse(&err.into_structured());
        assert_eq!(v["code"], "OPENAI_RATE_LIMITED");
        assert_eq!(v["category"], "transient");
        assert!(v["attributes"].get("retry_after_ms").is_none());
    }

    #[test]
    fn upstream_round_trip() {
        let err = IntegrationError::Upstream {
            prefix: "BEDROCK",
            status: 503,
            body: "try later".into(),
        };
        let v = parse(&err.into_structured());
        assert_eq!(v["code"], "BEDROCK_SERVER_ERROR");
        assert_eq!(v["category"], "transient");
    }

    #[test]
    fn upstream_408_is_transient_timeout() {
        let err = IntegrationError::Upstream {
            prefix: "BEDROCK",
            status: 408,
            body: "".into(),
        };
        let v = parse(&err.into_structured());
        assert_eq!(v["code"], "BEDROCK_TIMEOUT");
        assert_eq!(v["category"], "transient");
    }

    #[test]
    fn validation_round_trip() {
        let err = IntegrationError::Validation {
            prefix: "SHOPIFY",
            message: "title required".into(),
            details: json!({ "field": "title" }),
        };
        let v = parse(&err.into_structured());
        assert_eq!(v["code"], "SHOPIFY_VALIDATION_ERROR");
        assert_eq!(v["category"], "permanent");
        assert_eq!(v["message"], "title required");
        assert_eq!(v["attributes"]["field"], "title");
    }

    #[test]
    fn network_round_trip_is_transient() {
        let err = IntegrationError::Network {
            prefix: "MAILGUN",
            message: "dns resolution failed".into(),
        };
        let v = parse(&err.into_structured());
        assert_eq!(v["code"], "MAILGUN_NETWORK_ERROR");
        assert_eq!(v["category"], "transient");
    }

    #[test]
    fn deserialization_round_trip_is_permanent() {
        let err = IntegrationError::Deserialization {
            prefix: "SLACK",
            message: "expected JSON object".into(),
        };
        let v = parse(&err.into_structured());
        assert_eq!(v["code"], "SLACK_INVALID_RESPONSE");
        assert_eq!(v["category"], "permanent");
    }

    #[test]
    fn no_connection_round_trip() {
        let err = IntegrationError::NoConnection { prefix: "STRIPE" };
        let v = parse(&err.into_structured());
        assert_eq!(v["code"], "STRIPE_NO_CONNECTION");
        assert_eq!(v["category"], "permanent");
        assert_eq!(v["message"], "Connection is required for STRIPE operations");
    }

    #[test]
    fn missing_field_round_trip() {
        let err = IntegrationError::MissingField {
            prefix: "MAILGUN",
            field: "domain",
            payload: json!({"some": "value"}),
        };
        let v = parse(&err.into_structured());
        assert_eq!(v["code"], "MAILGUN_MISSING_FIELD");
        assert_eq!(v["category"], "permanent");
        assert_eq!(v["attributes"]["field"], "domain");
    }

    #[test]
    fn unknown_round_trip_preserves_category_and_attributes() {
        let err = IntegrationError::Unknown {
            prefix: "SLACK",
            code: "SLACK_CHANNEL_NOT_FOUND".into(),
            message: "channel_not_found".into(),
            category: ErrorCategory::Permanent,
            attributes: json!({ "error": "channel_not_found" }),
        };
        let v = parse(&err.into_structured());
        assert_eq!(v["code"], "SLACK_CHANNEL_NOT_FOUND");
        assert_eq!(v["category"], "permanent");
        assert_eq!(v["attributes"]["error"], "channel_not_found");
    }

    #[test]
    fn unknown_round_trip_transient_variant() {
        let err = IntegrationError::Unknown {
            prefix: "SLACK",
            code: "SLACK_RATE_LIMITED".into(),
            message: "ratelimited".into(),
            category: ErrorCategory::Transient,
            attributes: json!({}),
        };
        let v = parse(&err.into_structured());
        assert_eq!(v["category"], "transient");
    }

    #[test]
    fn from_impl_produces_same_string() {
        let err = IntegrationError::NoConnection { prefix: "STRIPE" };
        let via_fn = err.clone().into_structured();
        let via_from: String = err.into();
        assert_eq!(via_fn, via_from);
    }
}
