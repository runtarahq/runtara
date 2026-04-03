// Allow dead code temporarily during refactoring - these will be used as modules are migrated
#![allow(dead_code)]

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
#[allow(dead_code)]
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ApiResponse<T> {
    pub success: bool,
    pub message: String,
    pub data: T,
}

#[allow(dead_code)]
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

/// Standard delete response
#[derive(Debug, Serialize, ToSchema)]
pub struct DeleteResponse {
    pub success: bool,
    pub message: String,
}

impl DeleteResponse {
    pub fn success(message: impl Into<String>) -> Self {
        Self {
            success: true,
            message: message.into(),
        }
    }
}

/// Standard pagination query parameters
#[derive(Debug, Deserialize, ToSchema)]
pub struct PaginationQuery {
    pub page: Option<i32>,
    pub size: Option<i32>,
}

impl PaginationQuery {
    pub fn page(&self) -> i32 {
        self.page.unwrap_or(1).max(1)
    }

    pub fn size(&self) -> i32 {
        self.size.unwrap_or(20).clamp(1, 100)
    }

    pub fn offset(&self) -> i64 {
        ((self.page() - 1) * self.size()) as i64
    }

    pub fn limit(&self) -> i64 {
        self.size() as i64
    }
}

impl Default for PaginationQuery {
    fn default() -> Self {
        Self {
            page: Some(1),
            size: Some(20),
        }
    }
}

/// Standard paginated response wrapper
#[derive(Debug, Serialize, ToSchema)]
pub struct PaginatedResponse<T> {
    pub items: Vec<T>,
    pub total_count: i64,
    pub page: i32,
    pub size: i32,
    pub total_pages: i32,
}

impl<T> PaginatedResponse<T> {
    pub fn new(items: Vec<T>, total_count: i64, page: i32, size: i32) -> Self {
        let total_pages = if total_count == 0 {
            0
        } else {
            ((total_count as f64) / (size as f64)).ceil() as i32
        };

        Self {
            items,
            total_count,
            page,
            size,
            total_pages,
        }
    }
}

/// Standard bulk delete request
#[derive(Debug, Deserialize, ToSchema)]
pub struct BulkDeleteRequest {
    pub ids: Vec<String>,
}

/// Standard bulk delete response
#[derive(Debug, Serialize, ToSchema)]
pub struct BulkDeleteResponse {
    pub success: bool,
    pub deleted_count: usize,
    pub message: String,
}

impl BulkDeleteResponse {
    pub fn success(deleted_count: usize) -> Self {
        Self {
            success: true,
            deleted_count,
            message: format!("Successfully deleted {} item(s)", deleted_count),
        }
    }
}

/// Not implemented response (for stub endpoints)
#[derive(Debug, Serialize, ToSchema)]
pub struct NotImplementedResponse {
    pub success: bool,
    pub message: String,
}

impl NotImplementedResponse {
    pub fn new() -> Self {
        Self {
            success: false,
            message: "This endpoint is not yet implemented".to_string(),
        }
    }
}

impl Default for NotImplementedResponse {
    fn default() -> Self {
        Self::new()
    }
}
