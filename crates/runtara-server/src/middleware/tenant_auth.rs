use axum::{
    extract::FromRequestParts,
    http::{StatusCode, request::Parts},
    response::Json,
};
use serde_json::{Value, json};

use crate::auth::AuthContext;

/// Middleware that bridges server auth context to `runtara_connections::TenantId`.
///
/// Reads `AuthContext` from request extensions (inserted by the auth middleware)
/// and inserts `runtara_connections::TenantId` so crate handlers can extract it.
pub async fn inject_connections_tenant_id(
    mut req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let tenant_id = req
        .extensions()
        .get::<AuthContext>()
        .map(|ctx| ctx.org_id.clone());
    if let Some(org_id) = tenant_id {
        req.extensions_mut()
            .insert(runtara_connections::TenantId(org_id));
    }
    next.run(req).await
}

/// Axum extractor that pulls the validated org_id from request extensions.
///
/// The `authenticate` middleware must run before this extractor is used.
/// It inserts `AuthContext` into extensions after JWT/API key validation.
///
/// Usage in handlers:
/// ```ignore
/// pub async fn handler(OrgId(tenant_id): OrgId) -> ... { ... }
/// ```
pub struct OrgId(pub String);

impl<S: Send + Sync> FromRequestParts<S> for OrgId {
    type Rejection = (StatusCode, Json<Value>);

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        parts
            .extensions
            .get::<AuthContext>()
            .map(|ctx| OrgId(ctx.org_id.clone()))
            .ok_or_else(|| {
                (
                    StatusCode::UNAUTHORIZED,
                    Json(json!({
                        "error": "Unauthorized",
                        "message": "Authentication required"
                    })),
                )
            })
    }
}
