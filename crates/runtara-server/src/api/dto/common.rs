//! DTOs shared across multiple handler modules.
//!
//! Most endpoint-specific request/response types live next to the handler
//! they serve. This module is for the small set of envelopes that
//! genuinely cross several modules — currently just `ErrorResponse` and
//! `ApiResponse<T>`. If a struct only has one caller, prefer keeping it
//! in that caller's module.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Standard error response used across all endpoints
#[derive(Debug, Serialize, ToSchema)]
pub struct ErrorResponse {
    pub success: bool,
    pub error: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

impl ErrorResponse {
    pub fn new(error: impl Into<String>) -> Self {
        Self {
            success: false,
            error: error.into(),
            message: None,
        }
    }

    pub fn with_message(error: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            success: false,
            error: error.into(),
            message: Some(message.into()),
        }
    }
}

/// Generic API response wrapper
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ApiResponse<T> {
    pub success: bool,
    pub message: String,
    pub data: T,
}

impl<T> ApiResponse<T> {
    pub fn success(data: T) -> Self {
        Self {
            success: true,
            message: "Success".to_string(),
            data,
        }
    }

    pub fn success_with_message(message: impl Into<String>, data: T) -> Self {
        Self {
            success: true,
            message: message.into(),
            data,
        }
    }

    pub fn error(message: impl Into<String>, data: T) -> Self {
        Self {
            success: false,
            message: message.into(),
            data,
        }
    }
}
