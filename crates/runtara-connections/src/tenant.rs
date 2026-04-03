use axum::extract::FromRequestParts;
use axum::http::StatusCode;
use axum::http::request::Parts;
use axum::response::{IntoResponse, Response};
use serde_json::json;

/// Tenant identifier extracted from request extensions.
///
/// The host application is responsible for inserting this into the request
/// extensions before the request reaches the connections router (typically
/// via an authentication middleware).
#[derive(Debug, Clone)]
pub struct TenantId(pub String);

impl<S: Send + Sync> FromRequestParts<S> for TenantId {
    type Rejection = TenantIdRejection;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        parts
            .extensions
            .get::<TenantId>()
            .cloned()
            .ok_or(TenantIdRejection)
    }
}

/// Rejection returned when `TenantId` is missing from request extensions.
pub struct TenantIdRejection;

impl IntoResponse for TenantIdRejection {
    fn into_response(self) -> Response {
        let body = axum::Json(json!({"error": "Missing tenant context"}));
        (StatusCode::UNAUTHORIZED, body).into_response()
    }
}
