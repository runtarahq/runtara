use axum::{
    extract::FromRequestParts,
    http::{StatusCode, request::Parts},
    response::Json,
};
use serde_json::{Value, json};

use crate::auth::{AuthContext, AuthMethod};
use crate::authz::Role;
use crate::product_events::EventSource;

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

/// Axum extractor resolving the **surface** an authenticated request entered through, for
/// product-analytics `source`. Prefers an explicit [`EventSource`] stamped in request
/// extensions — the MCP in-process bridge (`mcp::tools::internal_api::build_request`) sets
/// `EventSource::Mcp` there. Absent that (a real external HTTP request), it falls back to
/// the caller's auth method as a proxy: an API key implies the programmatic API surface, a
/// JWT (or non-OIDC mode) implies the web UI.
///
/// Infallible: a surface label must never fail a request, so it always resolves to *some*
/// value (defaulting to the UI when no auth context is present at all).
pub struct Source(pub EventSource);

impl<S: Send + Sync> FromRequestParts<S> for Source {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        // An explicit surface marker (e.g. MCP) always wins.
        if let Some(source) = parts.extensions.get::<EventSource>().copied() {
            return Ok(Source(source));
        }
        // Otherwise infer the surface from how the caller authenticated.
        let source = match parts
            .extensions
            .get::<AuthContext>()
            .map(|ctx| ctx.auth_method)
        {
            Some(AuthMethod::ApiKey) => EventSource::Api,
            _ => EventSource::Ui,
        };
        Ok(Source(source))
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

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::Request;

    /// Build request `Parts` with the given extensions populated.
    fn parts_with(f: impl FnOnce(&mut axum::http::Extensions)) -> Parts {
        let mut req = Request::builder().body(()).unwrap();
        f(req.extensions_mut());
        req.into_parts().0
    }

    fn ctx(method: AuthMethod) -> AuthContext {
        AuthContext::new("org".to_string(), "user".to_string(), method)
    }

    #[tokio::test]
    async fn source_prefers_explicit_marker() {
        let mut parts = parts_with(|ext| {
            ext.insert(EventSource::Mcp);
        });
        let Source(s) = Source::from_request_parts(&mut parts, &()).await.unwrap();
        assert_eq!(s, EventSource::Mcp);
    }

    #[tokio::test]
    async fn source_marker_wins_over_auth_method() {
        // An explicit surface marker beats the auth-method fallback.
        let mut parts = parts_with(|ext| {
            ext.insert(ctx(AuthMethod::ApiKey));
            ext.insert(EventSource::Mcp);
        });
        let Source(s) = Source::from_request_parts(&mut parts, &()).await.unwrap();
        assert_eq!(s, EventSource::Mcp);
    }

    #[tokio::test]
    async fn source_api_key_falls_back_to_api() {
        let mut parts = parts_with(|ext| {
            ext.insert(ctx(AuthMethod::ApiKey));
        });
        let Source(s) = Source::from_request_parts(&mut parts, &()).await.unwrap();
        assert_eq!(s, EventSource::Api);
    }

    #[tokio::test]
    async fn source_jwt_falls_back_to_ui() {
        let mut parts = parts_with(|ext| {
            ext.insert(ctx(AuthMethod::Jwt));
        });
        let Source(s) = Source::from_request_parts(&mut parts, &()).await.unwrap();
        assert_eq!(s, EventSource::Ui);
    }

    #[tokio::test]
    async fn source_defaults_to_ui_without_any_context() {
        let mut parts = parts_with(|_| {});
        let Source(s) = Source::from_request_parts(&mut parts, &()).await.unwrap();
        assert_eq!(s, EventSource::Ui);
    }
}
