//! Shared HTTP helpers for handlers.
//!
//! Centralizes the mapping from `ExecutionError` to an HTTP response so that
//! handlers can remain thin wrappers over `ExecutionEngine`. The engine
//! exposes `ExecutionError::http_status()` as the recommended status mapping;
//! these helpers pair that with the canonical `{success, message, data}`
//! body shape used across scenario handlers.

use axum::http::StatusCode;
use axum::response::Json;
use serde_json::{Value, json};

use crate::workers::execution_engine::ExecutionError;

/// Standard error response for an `ExecutionError`.
///
/// Status comes from `ExecutionError::http_status()`; body follows the
/// canonical `{success: false, message: <display>, data: null}` shape.
pub fn execution_error_response(err: &ExecutionError) -> (StatusCode, Json<Value>) {
    (
        err.http_status(),
        Json(json!({
            "success": false,
            "message": format!("{}", err),
            "data": Value::Null,
        })),
    )
}

/// Like `execution_error_response` but merges extra top-level fields
/// (e.g. `instanceId`) into the body. Used by instance-scoped handlers
/// (stop / pause / resume) that echo the target id in error responses.
pub fn execution_error_response_with(
    err: &ExecutionError,
    extra: Value,
) -> (StatusCode, Json<Value>) {
    let mut body = json!({
        "success": false,
        "message": format!("{}", err),
        "data": Value::Null,
    });
    if let (Some(extra_obj), Some(body_obj)) = (extra.as_object(), body.as_object_mut()) {
        for (k, v) in extra_obj {
            body_obj.insert(k.clone(), v.clone());
        }
    }
    (err.http_status(), Json(body))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn execution_error_response_uses_http_status() {
        let cases = [
            (
                ExecutionError::ValidationError("bad".into()),
                StatusCode::BAD_REQUEST,
            ),
            (
                ExecutionError::NotFound("missing".into()),
                StatusCode::NOT_FOUND,
            ),
            (
                ExecutionError::ScenarioNotFound("gone".into()),
                StatusCode::NOT_FOUND,
            ),
            (
                ExecutionError::CompilationTimeout("slow".into()),
                StatusCode::GATEWAY_TIMEOUT,
            ),
            (
                ExecutionError::NotConnected("no conn".into()),
                StatusCode::SERVICE_UNAVAILABLE,
            ),
            (
                ExecutionError::DatabaseError("boom".into()),
                StatusCode::INTERNAL_SERVER_ERROR,
            ),
        ];

        for (err, expected_status) in &cases {
            let (status, Json(body)) = execution_error_response(err);
            assert_eq!(status, *expected_status, "status mismatch for {:?}", err);
            assert_eq!(body["success"], false);
            assert_eq!(body["data"], Value::Null);
            assert_eq!(body["message"], format!("{}", err));
        }
    }

    #[test]
    fn execution_error_response_with_merges_extra_fields() {
        let err = ExecutionError::NotFound("no instance".into());
        let (status, Json(body)) =
            execution_error_response_with(&err, json!({"instanceId": "abc-123"}));

        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body["success"], false);
        assert_eq!(body["instanceId"], "abc-123");
        assert_eq!(body["message"], "Not found: no instance");
    }

    #[test]
    fn execution_error_response_with_ignores_non_object_extra() {
        let err = ExecutionError::ValidationError("x".into());
        let (_, Json(body)) = execution_error_response_with(&err, json!("not an object"));
        assert_eq!(body["success"], false);
        assert_eq!(body["data"], Value::Null);
        assert!(body.get("instanceId").is_none());
    }
}
