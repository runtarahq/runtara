//! Shared HTTP helpers for handlers.
//!
//! Centralizes the mapping from `ExecutionError` to an HTTP response so that
//! handlers can remain thin wrappers over `ExecutionEngine`. The engine
//! exposes `ExecutionError::http_status()` as the recommended status mapping;
//! these helpers pair that with the canonical `{success, message, data}`
//! body shape used across workflow handlers.

use axum::http::StatusCode;
use axum::response::Json;
use serde_json::{Value, json};

use crate::workers::execution_engine::ExecutionError;

/// Standard error response for an `ExecutionError`.
///
/// Status comes from `ExecutionError::http_status()`; body follows the
/// canonical `{success: false, message: <display>, data: null}` shape
/// **except** for `EntitlementDenied`, which carries the documented
/// entitlement body (`{error, code, limit, maximum, message}`) so callers
/// can switch on the stable `code` field (see
/// `docs/entitlements.md` § Error Model).
pub fn execution_error_response(err: &ExecutionError) -> (StatusCode, Json<Value>) {
    if let ExecutionError::EntitlementDenied(denial) = err {
        return (err.http_status(), Json(denial.json_body()));
    }
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
///
/// Same `EntitlementDenied` special-case as
/// [`execution_error_response`]: the entitlement body is used as the base
/// (so `code` / `limit` / `maximum` survive) and `extra` fields are still
/// merged on top.
pub fn execution_error_response_with(
    err: &ExecutionError,
    extra: Value,
) -> (StatusCode, Json<Value>) {
    let mut body = if let ExecutionError::EntitlementDenied(denial) = err {
        denial.json_body()
    } else {
        json!({
            "success": false,
            "message": format!("{}", err),
            "data": Value::Null,
        })
    };
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
                ExecutionError::WorkflowNotFound("gone".into()),
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

    #[test]
    fn execution_error_response_surfaces_entitlement_denial_body() {
        // SYN-433 Finding 1: an EntitlementDenied execution error must
        // produce the documented body shape (code / limit / maximum) so
        // callers can switch on `code` — not the generic
        // `{success, message, data}` envelope.
        use crate::entitlement_error::{EntitlementDenial, codes};

        let denial = EntitlementDenial::LimitExceeded {
            limit: "maxConcurrentExecutions",
            maximum: 2,
        };
        let err = ExecutionError::EntitlementDenied(denial);
        let (status, Json(body)) = execution_error_response(&err);

        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(body["code"], codes::ENTITLEMENT_LIMIT_EXCEEDED);
        assert_eq!(body["limit"], "maxConcurrentExecutions");
        assert_eq!(body["maximum"], 2);
        assert!(
            body.get("data").is_none(),
            "entitlement bodies don't carry the generic `data` field"
        );
    }

    #[test]
    fn execution_error_response_with_merges_extra_into_entitlement_body() {
        // The instance-scoped variant must use the entitlement body as the
        // base AND still merge extra fields like instanceId on top.
        use crate::entitlement_error::{EntitlementDenial, codes};

        let denial = EntitlementDenial::LimitExceeded {
            limit: "maxConcurrentExecutions",
            maximum: 1,
        };
        let err = ExecutionError::EntitlementDenied(denial);
        let (status, Json(body)) =
            execution_error_response_with(&err, json!({"instanceId": "abc-123"}));

        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(body["code"], codes::ENTITLEMENT_LIMIT_EXCEEDED);
        assert_eq!(body["instanceId"], "abc-123");
        assert_eq!(body["maximum"], 1);
    }
}
