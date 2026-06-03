//! Role-based authorization gate.
//!
//! The auth middleware resolves the caller's [`Role`] from the per-tenant Valkey `member:{sub}`
//! entry and parks it on [`AuthContext::role`]; the static role → permission map
//! ([`crate::authz::access_for`]) is the single source of truth for "which role can do what".
//! This module turns that map into enforcement: a pure route-level decision
//! ([`require_permission`]) plus a `route_layer`-compatible constructor ([`require`]) that
//! short-circuits a request with `403 PERMISSION_DENIED` before the handler runs.
//!
//! Two deliberate scoping choices:
//!
//! - **Enforcement follows the membership posture.** Authorization only bites under
//!   [`MembershipPolicy::Required`] — the same switch that makes the Valkey membership lookup
//!   mandatory. Under `Disabled`/`Logging` the gate is a no-op, mirroring
//!   [`crate::middleware::auth::enforce_membership`] so the rollout stays a single knob
//!   (`disabled → logging → required`) rather than two parallel ones. Route-level role checks
//!   only begin once membership enforcement is `Required`.
//! - **`Access::Own` passes the route gate.** A `member:update`/`delete`-style permission that
//!   the map scopes to `Own` means "allowed, *on resources you created*". The route gate
//!   answers only the coarse question "may this role touch this permission at all?"; the
//!   per-resource `created_by` comparison is enforced separately in the handler. So both
//!   `Allow` and `Own` clear the gate here, and only `Deny` is rejected.

use axum::{
    extract::Request,
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Json, Response},
};
use futures::{FutureExt, future::BoxFuture};
use serde_json::{Value, json};

use crate::auth::{AuthContext, MembershipPolicy};
use crate::authz::{Access, Permission, Role, access_for};

/// A route-level authorization denial: the caller's role does not grant `permission`.
///
/// Renders as `403 Forbidden` with a stable `code` of `PERMISSION_DENIED` and the offending
/// permission in colon-style wire form, matching the sibling gate shapes in
/// [`crate::middleware::entitlement`] (`code` + a context field).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuthzDenial {
    permission: Permission,
}

impl AuthzDenial {
    /// Stable error code surfaced to clients and logs. One value — the *which permission*
    /// detail rides in the body, not the code, so callers switch on a single constant.
    pub const CODE: &'static str = "PERMISSION_DENIED";

    pub fn forbidden(permission: Permission) -> Self {
        Self { permission }
    }

    pub fn permission(&self) -> Permission {
        self.permission
    }

    pub fn code(&self) -> &'static str {
        Self::CODE
    }

    /// The 403 JSON body. `permission` is the colon-style identifier (`workflow:delete`) so
    /// the wire shape matches the contracts doc and the admin UI vocabulary.
    pub fn json_body(&self) -> Value {
        json!({
            "code": Self::CODE,
            "permission": self.permission.as_str(),
            "message": format!(
                "Your role does not permit {}",
                self.permission.as_str()
            ),
        })
    }
}

impl IntoResponse for AuthzDenial {
    fn into_response(self) -> Response {
        (StatusCode::FORBIDDEN, Json(self.json_body())).into_response()
    }
}

/// The pure route-level authorization decision. `Ok(())` lets the request proceed; `Err`
/// carries the `403` to surface.
///
/// Semantics (see the module docs for the rationale):
///
/// - Any policy other than [`MembershipPolicy::Required`] → `Ok` (gate disabled).
/// - `role == None` under `Required` → `Ok`. This is not a hole: a JWT request that failed to
///   resolve a role never reaches enforcement (membership fails closed first); the only
///   role-less callers that get here are legacy API keys with no `issuing_user_id` (which keep
///   their pre-contract unrestricted behavior until rotated) and trusted in-process calls.
/// - `Some(role)` → consult [`access_for`]: [`Access::Allow`] and [`Access::Own`] pass (the
///   `Own` ownership comparison is a handler-level check), [`Access::Deny`] rejects.
pub fn require_permission(
    policy: MembershipPolicy,
    role: Option<Role>,
    permission: Permission,
) -> Result<(), AuthzDenial> {
    if policy != MembershipPolicy::Required {
        return Ok(());
    }
    let Some(role) = role else {
        return Ok(());
    };
    match access_for(role, permission) {
        Access::Allow | Access::Own => Ok(()),
        Access::Deny => Err(AuthzDenial::forbidden(permission)),
    }
}

/// Build a `route_layer`-compatible middleware that enforces `permission` on every route it
/// wraps. `policy` is captured at wiring time (it is process-global and `Copy`), so the layer
/// needs no state handle.
///
/// Usage (route groups are wired to permissions separately):
///
/// ```ignore
/// let workflows = Router::new()
///     .route("/workflows/{id}/delete", post(delete_handler))
///     .route_layer(from_fn(require(Permission::WorkflowDelete, membership_policy)));
/// ```
///
/// The layer reads the caller's role from the [`AuthContext`] the `authenticate` middleware
/// inserted upstream; a missing `AuthContext` is treated as "no role" and passes, because the
/// authentication layer — not this gate — owns rejecting unauthenticated requests.
pub fn require(
    permission: Permission,
    policy: MembershipPolicy,
) -> impl Clone + Send + Sync + 'static + Fn(Request, Next) -> BoxFuture<'static, Response> {
    move |req: Request, next: Next| {
        async move {
            let role = req.extensions().get::<AuthContext>().and_then(|c| c.role);
            match require_permission(policy, role, permission) {
                Ok(()) => next.run(req).await,
                Err(denial) => denial.into_response(),
            }
        }
        .boxed()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use axum::Router;
    use axum::body::Body;
    use axum::http::Request as HttpRequest;
    use axum::middleware::from_fn;
    use axum::routing::{get, post};
    use tower::ServiceExt;

    use crate::auth::AuthMethod;

    // ── pure decision: enforcement only under Required ──────────────────────

    #[test]
    fn disabled_and_logging_never_enforce() {
        // Even a Viewer hitting a Deny cell (workflow:delete) passes when the policy isn't
        // Required — the gate is dormant until membership enforcement is on.
        for policy in [MembershipPolicy::Disabled, MembershipPolicy::Logging] {
            assert!(
                require_permission(policy, Some(Role::Viewer), Permission::WorkflowDelete).is_ok(),
                "policy {policy:?} must not enforce"
            );
        }
    }

    #[test]
    fn required_with_no_role_passes() {
        // Legacy API keys (no issuing_user_id) and trusted in-process calls reach the gate
        // with role == None and must not be blocked.
        assert!(
            require_permission(MembershipPolicy::Required, None, Permission::WorkflowDelete)
                .is_ok()
        );
    }

    // ── pure decision: the role × permission matrix under Required ──────────

    #[test]
    fn required_viewer_reads_pass_writes_deny() {
        let p = MembershipPolicy::Required;
        // Reads allowed.
        assert!(require_permission(p, Some(Role::Viewer), Permission::WorkflowRead).is_ok());
        assert!(require_permission(p, Some(Role::Viewer), Permission::ConnectionRead).is_ok());
        // Create / execute / update / delete all denied for a read-only role.
        for permission in [
            Permission::WorkflowCreate,
            Permission::WorkflowUpdate,
            Permission::WorkflowDelete,
            Permission::WorkflowExecute,
            Permission::ConnectionCreate,
        ] {
            let err = require_permission(p, Some(Role::Viewer), permission)
                .expect_err("viewer must be denied a write");
            assert_eq!(err.code(), AuthzDenial::CODE);
            assert_eq!(err.permission(), permission);
        }
    }

    #[test]
    fn required_member_own_scoped_permissions_pass_the_route_gate() {
        // The map scopes workflow:update/delete to Own for Member. At the route gate that is
        // indistinguishable from Allow — the created_by comparison is a handler-level
        // check, not this gate.
        let p = MembershipPolicy::Required;
        for permission in [
            Permission::WorkflowUpdate,
            Permission::WorkflowDelete,
            Permission::ConnectionUpdate,
            Permission::ConnectionDelete,
        ] {
            assert!(
                require_permission(p, Some(Role::Member), permission).is_ok(),
                "Own-scoped {permission} must clear the route gate for Member"
            );
        }
        // ...but a Member still cannot do an Owner/Admin-only action — there are none in the
        // P0 runtara vocabulary, so the strongest negative we can assert is Viewer-style:
        // a Member CAN create/execute (Allow).
        assert!(require_permission(p, Some(Role::Member), Permission::WorkflowCreate).is_ok());
        assert!(require_permission(p, Some(Role::Member), Permission::WorkflowExecute).is_ok());
    }

    #[test]
    fn required_owner_and_admin_pass_every_permission() {
        let p = MembershipPolicy::Required;
        for role in [Role::Owner, Role::Admin] {
            for permission in Permission::ALL {
                assert!(
                    require_permission(p, Some(role), permission).is_ok(),
                    "{role:?} must be allowed {permission}"
                );
            }
        }
    }

    #[test]
    fn required_decision_matches_access_for_for_every_cell() {
        // Cross-check the gate against the map directly: Allow/Own → Ok, Deny → Err, for the
        // whole Role × Permission grid under Required.
        let p = MembershipPolicy::Required;
        for role in Role::ALL {
            for permission in Permission::ALL {
                let got = require_permission(p, Some(role), permission);
                match access_for(role, permission) {
                    Access::Allow | Access::Own => assert!(
                        got.is_ok(),
                        "access_for({role:?}, {permission}) allows but gate denied"
                    ),
                    Access::Deny => {
                        let err = got.expect_err("Deny cell must reject");
                        assert_eq!(err.permission(), permission);
                    }
                }
            }
        }
    }

    // ── denial response shape ───────────────────────────────────────────────

    #[tokio::test]
    async fn denial_renders_as_403_with_stable_code_and_permission() {
        let denial = AuthzDenial::forbidden(Permission::WorkflowDelete);
        let response = denial.into_response();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let bytes = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .expect("body bytes");
        let body: Value = serde_json::from_slice(&bytes).expect("json body");
        assert_eq!(body["code"], AuthzDenial::CODE);
        assert_eq!(body["permission"], "workflow:delete");
    }

    // ── HTTP composition: the `require` layer in front of a real route ───────

    fn ctx_with_role(role: Option<Role>) -> AuthContext {
        let mut ctx = AuthContext::new("tenant".into(), "auth0|u".into(), AuthMethod::Jwt);
        ctx.role = role;
        ctx
    }

    /// Wrap a route with an injected `AuthContext` (standing in for the `authenticate`
    /// middleware) and the `require` gate, then drive one request through it.
    async fn run(
        method_role: Option<Role>,
        policy: MembershipPolicy,
        permission: Permission,
        verb_post: bool,
    ) -> Response {
        let ctx = ctx_with_role(method_role);
        let inject = move |mut req: Request, next: Next| {
            let ctx = ctx.clone();
            async move {
                req.extensions_mut().insert(ctx);
                next.run(req).await
            }
            .boxed()
        };
        let route = if verb_post {
            post(|| async { "ok" })
        } else {
            get(|| async { "ok" })
        };
        let app = Router::new()
            .route("/r", route)
            .route_layer(from_fn(require(permission, policy)))
            .route_layer(from_fn(inject));

        let builder = if verb_post {
            HttpRequest::post("/r")
        } else {
            HttpRequest::get("/r")
        };
        app.oneshot(builder.body(Body::empty()).unwrap())
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn layer_denies_viewer_write_under_required() {
        let resp = run(
            Some(Role::Viewer),
            MembershipPolicy::Required,
            Permission::WorkflowDelete,
            true,
        )
        .await;
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
        let bytes = axum::body::to_bytes(resp.into_body(), 4096).await.unwrap();
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["code"], AuthzDenial::CODE);
        assert_eq!(body["permission"], "workflow:delete");
    }

    #[tokio::test]
    async fn layer_allows_viewer_read_under_required() {
        let resp = run(
            Some(Role::Viewer),
            MembershipPolicy::Required,
            Permission::WorkflowRead,
            false,
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn layer_allows_member_own_scoped_write_under_required() {
        // Member + workflow:delete is Own — the route gate lets it through; the per-resource
        // ownership check happens later in the handler.
        let resp = run(
            Some(Role::Member),
            MembershipPolicy::Required,
            Permission::WorkflowDelete,
            true,
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn layer_is_dormant_under_logging() {
        // Same Viewer write that 403s under Required passes under Logging — the gate follows
        // the membership posture.
        let resp = run(
            Some(Role::Viewer),
            MembershipPolicy::Logging,
            Permission::WorkflowDelete,
            true,
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn layer_passes_when_no_auth_context_present() {
        // No injected AuthContext at all → the gate defers to the auth layer and passes.
        let app = Router::new()
            .route("/r", post(|| async { "ok" }))
            .route_layer(from_fn(require(
                Permission::WorkflowDelete,
                MembershipPolicy::Required,
            )));
        let resp = app
            .oneshot(HttpRequest::post("/r").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }
}
