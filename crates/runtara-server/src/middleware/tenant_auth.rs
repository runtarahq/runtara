use axum::{
    extract::FromRequestParts,
    http::{StatusCode, request::Parts},
    response::Json,
};
use serde_json::{Value, json};

use crate::auth::AuthContext;
use crate::authz::Role;

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

/// Axum extractor for the authenticated caller's user id (Auth0 `sub`, or the synthetic id
/// for non-JWT modes). Like [`OrgId`], it reads `AuthContext` from request extensions and
/// requires the `authenticate` middleware to have run.
pub struct CallerId(pub String);

impl<S: Send + Sync> FromRequestParts<S> for CallerId {
    type Rejection = (StatusCode, Json<Value>);

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        parts
            .extensions
            .get::<AuthContext>()
            .map(|ctx| CallerId(ctx.user_id.clone()))
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

/// Axum extractor yielding both the caller's user id and resolved [`Role`] — what handler-level
/// `Own` ownership checks need (compare `created_by` against `user_id`, gated by `role`). Like
/// [`OrgId`]/[`CallerId`] it reads `AuthContext` from request extensions, so the `authenticate`
/// middleware must have run. `role` is `None` outside SaaS enforcement (local/trust_proxy, or
/// before the membership lookup populates it).
pub struct Caller {
    pub user_id: String,
    pub role: Option<Role>,
}

impl<S: Send + Sync> FromRequestParts<S> for Caller {
    type Rejection = (StatusCode, Json<Value>);

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        parts
            .extensions
            .get::<AuthContext>()
            .map(|ctx| Caller {
                user_id: ctx.user_id.clone(),
                role: ctx.role,
            })
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
