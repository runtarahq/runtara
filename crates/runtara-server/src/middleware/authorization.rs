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
    extract::{MatchedPath, Request},
    http::{Method, StatusCode},
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
///   resolve a role never reaches enforcement (membership fails closed first), and an API key
///   always resolves its issuing user's role; the only role-less callers that get here are
///   trusted in-process calls.
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

/// The permission a request requires, keyed on its HTTP method and the matched route template
/// (e.g. `/api/runtime/workflows/{id}/delete`, with `{…}` placeholders, not a concrete path).
///
/// This is the single place the route → permission map lives. `None` means the route is
/// intentionally ungated: read-only metadata (specs, step types, agent listings/metadata, the
/// entitlements snapshot) and health. A `None` here lets the request through — only routes with
/// a permission can be denied. The ungated set is pinned by a coverage test, so a new mutating
/// route that forgets a mapping fails CI rather than silently opening.
///
/// Mappings worth calling out, because they are choices rather than mechanical:
///
/// - **Instance control** (stop/pause/resume/replay, signal/action submission, session event
///   submit) → `workflow:execute`: driving a run is an execution capability, and Member may
///   execute any workflow, so it stays consistent with "Member can run, Viewer cannot".
/// - **Compile** → `workflow:execute` (not `update`): it produces a runnable image, and a
///   Member who can execute any workflow must be able to compile any workflow to run it.
/// - **Clone** → `workflow:create`: it produces a new workflow.
/// - **Graph/mapping validation, preview, render, CSV/SQL read queries** → the resource's
///   `read`: they touch no state.
/// - **`object-model/sql/execute`** → `database:delete`: it runs arbitrary SQL, so it is gated
///   at the most destructive write.
/// - **Report-driven workflow-action submit** → `report:read`: it is a report-consumption
///   interaction; the report surface, not the workflow surface, gates it.
/// - **OAuth authorize** (a `GET`) → `connection:update`: it begins a credential change, so it
///   must be closed to read-only Viewers despite the verb.
/// - **Agent execute / test** → `workflow:execute`: host-mediated capability I/O (possibly with
///   a connection's credentials) is an execution capability; Viewers are denied.
/// - **API keys** (legacy `rt_*`) are deliberately absent: they are personal credentials gated by
///   ownership in the handler (every key has an `issuing_user_id`; a caller manages only its own),
///   not by role, so no permission is assigned and the route stays ungated here.
///
/// Connection routes are matched on both their full (`/api/runtime/connections/{id}`) and
/// nest-relative (`/connections/{id}`) templates, so the gate is correct regardless of which
/// form the nested router surfaces as the matched path.
pub fn permission_for(method: &Method, path: &str) -> Option<Permission> {
    use Permission::{
        AnalyticsRead, ConnectionCreate, ConnectionDelete, ConnectionRead, ConnectionUpdate,
        DatabaseCreate, DatabaseDelete, DatabaseRead, DatabaseUpdate, InvocationHistoryRead,
        ReportCreate, ReportDelete, ReportRead, ReportUpdate, TriggerCreate, TriggerDelete,
        TriggerRead, TriggerUpdate, WorkflowCreate, WorkflowDelete, WorkflowExecute,
        WorkflowFolderRename, WorkflowRead, WorkflowUpdate,
    };

    let m = method.as_str();
    Some(match (m, path) {
        // ── Workflows: read ──────────────────────────────────────────────
        ("GET", "/api/runtime/workflows") => WorkflowRead,
        ("GET", "/api/runtime/workflows/{id}") => WorkflowRead,
        ("GET", "/api/runtime/workflows/{id}/versions") => WorkflowRead,
        ("GET", "/api/runtime/workflows/{id}/versions/{version}/compilation-progress") => {
            WorkflowRead
        }
        ("GET", "/api/runtime/workflows/{id}/versions/{version}/schemas") => WorkflowRead,
        ("GET", "/api/runtime/workflows/{id}/dependencies") => WorkflowRead,
        ("GET", "/api/runtime/workflows/{id}/dependents") => WorkflowRead,
        ("GET", "/api/runtime/workflows/folders") => WorkflowRead,
        ("POST", "/api/runtime/workflows/graph/validate") => WorkflowRead,
        ("POST", "/api/runtime/workflows/{workflowId}/validate-mappings") => WorkflowRead,
        ("GET", "/api/runtime/steps") => WorkflowRead,
        // ── Workflows: create ────────────────────────────────────────────
        ("POST", "/api/runtime/workflows/create") => WorkflowCreate,
        ("POST", "/api/runtime/workflows/{id}/clone") => WorkflowCreate,
        // ── Workflows: update (authoring) ────────────────────────────────
        ("POST", "/api/runtime/workflows/{id}/update") => WorkflowUpdate,
        ("PUT", "/api/runtime/workflows/{id}/versions/{version}/graph") => WorkflowUpdate,
        ("PUT", "/api/runtime/workflows/{id}/versions/{version}/track-events") => WorkflowUpdate,
        ("POST", "/api/runtime/workflows/{id}/versions/{versionNumber}/set-current") => {
            WorkflowUpdate
        }
        // Folder rename is a tenant-wide bulk op (rewrites the path prefix of every workflow
        // under the folder, other members' included) → Owner/Admin only, not Member-`update`.
        ("PUT", "/api/runtime/workflows/folders/rename") => WorkflowFolderRename,
        // Move is single-workflow authoring → gated like update (tenant-wide Allow for Member).
        ("PUT", "/api/runtime/workflows/{id}/move") => WorkflowUpdate,
        // ── Workflows: delete ────────────────────────────────────────────
        ("POST", "/api/runtime/workflows/{id}/delete") => WorkflowDelete,
        // ── Workflows: execute / run control ─────────────────────────────
        ("POST", "/api/runtime/workflows/{id}/versions/{version}/compile") => WorkflowExecute,
        ("POST", "/api/runtime/workflows/{id}/execute") => WorkflowExecute,
        ("POST", "/api/runtime/workflows/{id}/chat") => WorkflowExecute,
        ("POST", "/api/runtime/workflows/{id}/chat/start") => WorkflowExecute,
        ("POST", "/api/runtime/workflows/{id}/sessions") => WorkflowExecute,
        ("POST", "/api/runtime/sessions/{sessionId}/events") => WorkflowExecute,
        ("POST", "/api/runtime/workflows/{id}/schedule") => WorkflowExecute,
        ("POST", "/api/runtime/workflows/instances/{instanceId}/stop") => WorkflowExecute,
        ("POST", "/api/runtime/workflows/instances/{instanceId}/pause") => WorkflowExecute,
        ("POST", "/api/runtime/workflows/instances/{instanceId}/resume") => WorkflowExecute,
        ("POST", "/api/runtime/workflows/instances/{instanceId}/replay") => WorkflowExecute,
        (
            "POST",
            "/api/runtime/workflows/{workflowId}/instances/{instanceId}/actions/{actionId}/submit",
        ) => WorkflowExecute,
        ("POST", "/api/runtime/signals/{instanceId}") => WorkflowExecute,
        // Host-mediated agent capability invocation (execute / test) runs real I/O, possibly
        // with a connection's stored credentials — gate it like running a workflow. (The
        // runtime's own internal calls use a separate no-auth router, so they are unaffected.)
        ("POST", "/api/runtime/agents/{name}/capabilities/{capability_id}/execute") => {
            WorkflowExecute
        }
        ("POST", "/api/runtime/agents/{name}/capabilities/{capability_id}/test") => WorkflowExecute,
        // ── Invocation history (runs, steps, events) ─────────────────────
        ("GET", "/api/runtime/executions") => InvocationHistoryRead,
        ("GET", "/api/runtime/sessions/{sessionId}/events") => InvocationHistoryRead,
        ("GET", "/api/runtime/sessions/{sessionId}/pending-input") => InvocationHistoryRead,
        ("GET", "/api/runtime/workflows/{id}/instances") => InvocationHistoryRead,
        ("GET", "/api/runtime/workflows/{id}/instances/{instanceId}") => InvocationHistoryRead,
        ("GET", "/api/runtime/workflows/{id}/instances/{instanceId}/checkpoints") => {
            InvocationHistoryRead
        }
        ("GET", "/api/runtime/workflows/instances/{instanceId}") => InvocationHistoryRead,
        ("GET", "/api/runtime/workflows/instances/{instanceId}/steps/{stepId}/subinstances") => {
            InvocationHistoryRead
        }
        ("GET", "/api/runtime/workflows/{workflowId}/instances/{instanceId}/step-events") => {
            InvocationHistoryRead
        }
        ("GET", "/api/runtime/workflows/{workflowId}/instances/{instanceId}/steps") => {
            InvocationHistoryRead
        }
        (
            "GET",
            "/api/runtime/workflows/{workflowId}/instances/{instanceId}/scopes/{scopeId}/ancestors",
        ) => InvocationHistoryRead,
        ("GET", "/api/runtime/workflows/{workflowId}/instances/{instanceId}/pending-input") => {
            InvocationHistoryRead
        }
        ("GET", "/api/runtime/workflows/{workflowId}/actions") => InvocationHistoryRead,
        ("GET", "/api/runtime/workflows/{workflowId}/instances/{instanceId}/actions") => {
            InvocationHistoryRead
        }
        // ── Analytics / metrics ──────────────────────────────────────────
        ("GET", "/api/runtime/metrics/workflows/{workflow_id}") => AnalyticsRead,
        ("GET", "/api/runtime/metrics/workflows/{workflow_id}/stats") => AnalyticsRead,
        ("GET", "/api/runtime/metrics/tenant") => AnalyticsRead,
        ("GET", "/api/runtime/analytics/system") => AnalyticsRead,
        // ── Triggers ─────────────────────────────────────────────────────
        ("POST", "/api/runtime/triggers") => TriggerCreate,
        ("GET", "/api/runtime/triggers") => TriggerRead,
        ("GET", "/api/runtime/triggers/{id}") => TriggerRead,
        ("PUT", "/api/runtime/triggers/{id}") => TriggerUpdate,
        ("DELETE", "/api/runtime/triggers/{id}") => TriggerDelete,
        // ── Reports ──────────────────────────────────────────────────────
        ("GET", "/api/runtime/reports") => ReportRead,
        ("POST", "/api/runtime/reports") => ReportCreate,
        ("POST", "/api/runtime/reports/validate") => ReportRead,
        ("POST", "/api/runtime/reports/preview") => ReportRead,
        ("GET", "/api/runtime/reports/schema") => ReportRead,
        ("GET", "/api/runtime/reports/{report_id}") => ReportRead,
        ("PUT", "/api/runtime/reports/{report_id}") => ReportUpdate,
        ("DELETE", "/api/runtime/reports/{report_id}") => ReportDelete,
        ("POST", "/api/runtime/reports/{report_id}/render") => ReportRead,
        ("POST", "/api/runtime/reports/{report_id}/blocks/{block_id}/data") => ReportRead,
        (
            "POST",
            "/api/runtime/reports/{report_id}/blocks/{block_id}/actions/{action_id}/submit",
        ) => ReportRead,
        ("POST", "/api/runtime/reports/{report_id}/filters/{filter_id}/options") => ReportRead,
        (
            "POST",
            "/api/runtime/reports/{report_id}/blocks/{block_id}/fields/{field}/lookup-options",
        ) => ReportRead,
        ("POST", "/api/runtime/reports/{report_id}/datasets/{dataset_id}/query") => ReportRead,
        ("POST", "/api/runtime/reports/{report_id}/edit") => ReportUpdate,
        // ── Object model (database) ──────────────────────────────────────
        ("POST", "/api/runtime/object-model/schemas") => DatabaseCreate,
        ("GET", "/api/runtime/object-model/schemas") => DatabaseRead,
        ("GET", "/api/runtime/object-model/schemas/{id}") => DatabaseRead,
        ("GET", "/api/runtime/object-model/schemas/name/{name}") => DatabaseRead,
        ("PUT", "/api/runtime/object-model/schemas/{id}") => DatabaseUpdate,
        ("DELETE", "/api/runtime/object-model/schemas/{id}") => DatabaseDelete,
        ("GET", "/api/runtime/object-model/instances/schema/{schema_id}") => DatabaseRead,
        ("GET", "/api/runtime/object-model/instances/schema/name/{schema_name}") => DatabaseRead,
        ("POST", "/api/runtime/object-model/instances") => DatabaseCreate,
        ("POST", "/api/runtime/object-model/instances/schema/{name}/filter") => DatabaseRead,
        ("POST", "/api/runtime/object-model/instances/schema/{name}/aggregate") => DatabaseRead,
        ("POST", "/api/runtime/object-model/sql/query") => DatabaseRead,
        ("POST", "/api/runtime/object-model/sql/query-one") => DatabaseRead,
        ("POST", "/api/runtime/object-model/sql/query-raw") => DatabaseRead,
        ("POST", "/api/runtime/object-model/sql/execute") => DatabaseDelete,
        ("GET", "/api/runtime/object-model/instances/{schema_id}/{instance_id}") => DatabaseRead,
        ("PUT", "/api/runtime/object-model/instances/{schema_id}/{instance_id}") => DatabaseUpdate,
        ("DELETE", "/api/runtime/object-model/instances/{schema_id}/{instance_id}") => {
            DatabaseDelete
        }
        ("DELETE", "/api/runtime/object-model/instances/{schema_id}/bulk") => DatabaseDelete,
        ("POST", "/api/runtime/object-model/instances/{schema_id}/bulk") => DatabaseCreate,
        ("PATCH", "/api/runtime/object-model/instances/{schema_id}/bulk") => DatabaseUpdate,
        ("POST", "/api/runtime/object-model/instances/schema/{name}/export-csv") => DatabaseRead,
        ("POST", "/api/runtime/object-model/instances/schema/{name}/import-csv/preview") => {
            DatabaseRead
        }
        ("POST", "/api/runtime/object-model/instances/schema/{name}/import-csv") => DatabaseCreate,
        // ── Connections (matched on full and nest-relative templates) ────
        ("POST", "/api/runtime/connections") | ("POST", "/connections") => ConnectionCreate,
        ("GET", "/api/runtime/connections") | ("GET", "/connections") => ConnectionRead,
        ("GET", "/api/runtime/connections/{id}") | ("GET", "/connections/{id}") => ConnectionRead,
        ("PUT", "/api/runtime/connections/{id}") | ("PUT", "/connections/{id}") => ConnectionUpdate,
        ("DELETE", "/api/runtime/connections/{id}") | ("DELETE", "/connections/{id}") => {
            ConnectionDelete
        }
        ("GET", "/api/runtime/connections/operator/{operatorName}")
        | ("GET", "/connections/operator/{operatorName}") => ConnectionRead,
        ("GET", "/api/runtime/connections/categories") | ("GET", "/connections/categories") => {
            ConnectionRead
        }
        ("GET", "/api/runtime/connections/auth-types") | ("GET", "/connections/auth-types") => {
            ConnectionRead
        }
        ("GET", "/api/runtime/connections/types") | ("GET", "/connections/types") => ConnectionRead,
        ("GET", "/api/runtime/connections/types/{integration_id}")
        | ("GET", "/connections/types/{integration_id}") => ConnectionRead,
        ("GET", "/api/runtime/connections/{id}/oauth/authorize")
        | ("GET", "/connections/{id}/oauth/authorize") => ConnectionUpdate,
        ("GET", "/api/runtime/rate-limits") | ("GET", "/rate-limits") => ConnectionRead,
        ("GET", "/api/runtime/connections/{id}/rate-limit-status")
        | ("GET", "/connections/{id}/rate-limit-status") => ConnectionRead,
        ("GET", "/api/runtime/connections/{id}/rate-limit-history")
        | ("GET", "/connections/{id}/rate-limit-history") => ConnectionRead,
        ("GET", "/api/runtime/connections/{id}/rate-limit-timeline")
        | ("GET", "/connections/{id}/rate-limit-timeline") => ConnectionRead,
        // API-key routes (legacy rt_* keys) are intentionally NOT role-gated here. An API key is
        // a personal credential scoped to its issuing user: any role may manage its own keys, and
        // the handlers enforce that ownership directly (list/revoke filter on `issuing_user_id`).
        // Role plays no part, so there is no permission to gate on — see `api/handlers/api_keys`.
        //
        // Everything else (specs, metadata, agent listings, the entitlements snapshot, health)
        // is intentionally ungated.
        _ => return None,
    })
}

/// Build a `route_layer`-compatible middleware that gates every request by the permission
/// [`permission_for`] assigns to its method + matched route. `policy` is captured at wiring
/// time (process-global, `Copy`).
///
/// Reads the matched route template ([`MatchedPath`], populated by routing) and the caller's
/// role ([`AuthContext`], populated by `authenticate`), both of which are present by the time a
/// `route_layer` runs. A request whose route has no permission, or whose role
/// [`require_permission`] permits, passes through; otherwise it short-circuits with
/// `403 PERMISSION_DENIED`.
pub fn authorize(
    policy: MembershipPolicy,
) -> impl Clone + Send + Sync + 'static + Fn(Request, Next) -> BoxFuture<'static, Response> {
    move |req: Request, next: Next| {
        // Snapshot the caller identity so denial logs are self-contained rather than relying on
        // parent-span field flattening. `role` also drives the gate decision.
        let (tenant_id, user_id, auth_method, role) = req
            .extensions()
            .get::<AuthContext>()
            .map(|c| {
                (
                    Some(c.org_id.clone()),
                    Some(c.user_id.clone()),
                    Some(c.auth_method.as_str()),
                    c.role,
                )
            })
            .unwrap_or((None, None, None, None));
        let matched = req
            .extensions()
            .get::<MatchedPath>()
            .map(|m| m.as_str().to_owned());
        let method = req.method().clone();
        async move {
            if let Some(path) = matched.as_deref()
                && let Some(permission) = permission_for(&method, path)
                && let Err(denial) = require_permission(policy, role, permission)
            {
                tracing::warn!(
                    tenant_id = tenant_id.as_deref(),
                    user_id = user_id.as_deref(),
                    auth_method,
                    role = role.map(|r| r.as_str()),
                    permission = permission.as_str(),
                    code = AuthzDenial::CODE,
                    method = method.as_str(),
                    matched_path = path,
                    "authorization denied"
                );
                crate::observability::record_permission_denial(permission.as_str());
                return denial.into_response();
            }
            next.run(req).await
        }
        .boxed()
    }
}

/// The resource-level ownership decision for an `Own`-scoped permission. This is the second
/// half of enforcement: the route gate ([`authorize`]) lets `Own` through, then the handler
/// loads the resource's `created_by` and calls this to decide whether the caller may act on
/// *this specific* resource.
///
/// - Non-`Required` policy or `role == None` → `Ok` (enforcement dormant; mirrors
///   [`require_permission`]).
/// - `Access::Allow` → `Ok` (Owner/Admin, who bypass ownership entirely).
/// - `Access::Own` → `Ok` only when `resource_owner == Some(caller_id)`. A different owner or a
///   `None` owner (unowned legacy row, or a resource that does not exist) denies — such rows are
///   manageable only by Owner/Admin, by design.
/// - `Access::Deny` → denies (defense in depth; the route gate already blocks these, but a
///   handler calling this directly must still fail closed).
pub fn require_ownership(
    policy: MembershipPolicy,
    role: Option<Role>,
    permission: Permission,
    resource_owner: Option<&str>,
    caller_id: &str,
) -> Result<(), AuthzDenial> {
    if policy != MembershipPolicy::Required {
        return Ok(());
    }
    let Some(role) = role else {
        return Ok(());
    };
    match access_for(role, permission) {
        Access::Allow => Ok(()),
        Access::Own if resource_owner == Some(caller_id) => Ok(()),
        Access::Own | Access::Deny => Err(AuthzDenial::forbidden(permission)),
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
        // Trusted in-process calls reach the gate with role == None and must not be blocked.
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
        // Create / execute / update / delete / folder-rename all denied for a read-only role.
        for permission in [
            Permission::WorkflowCreate,
            Permission::WorkflowUpdate,
            Permission::WorkflowDelete,
            Permission::WorkflowExecute,
            Permission::WorkflowFolderRename,
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
        // workflow:delete is Own and connection update/delete are Allow for Member. At the route
        // gate Own is indistinguishable from Allow — the created_by comparison is a handler-level
        // check, not this gate — so all of these clear it.
        let p = MembershipPolicy::Required;
        for permission in [
            Permission::WorkflowDelete,
            Permission::ConnectionUpdate,
            Permission::ConnectionDelete,
        ] {
            assert!(
                require_permission(p, Some(Role::Member), permission).is_ok(),
                "Member write {permission} must clear the route gate"
            );
        }
        // A Member CAN create/execute, and — collaboratively — update any workflow (all Allow).
        assert!(require_permission(p, Some(Role::Member), Permission::WorkflowCreate).is_ok());
        assert!(require_permission(p, Some(Role::Member), Permission::WorkflowExecute).is_ok());
        assert!(require_permission(p, Some(Role::Member), Permission::WorkflowUpdate).is_ok());
        // ...but folder rename is Owner/Admin-only — a Member is denied at the gate.
        let err = require_permission(p, Some(Role::Member), Permission::WorkflowFolderRename)
            .expect_err("Member must be denied folder rename");
        assert_eq!(err.permission(), Permission::WorkflowFolderRename);
    }

    #[test]
    fn required_folder_rename_is_owner_admin_only() {
        // The collaboration change opens workflow:update to Member, but folder rename — a
        // tenant-wide bulk path-prefix rewrite — must stay Owner/Admin-only. Pin the whole
        // column so a future grant-list edit can't quietly hand it to Member or Viewer.
        let p = MembershipPolicy::Required;
        for role in [Role::Owner, Role::Admin] {
            assert!(
                require_permission(p, Some(role), Permission::WorkflowFolderRename).is_ok(),
                "{role:?} must be allowed folder rename"
            );
        }
        for role in [Role::Member, Role::Viewer] {
            let err =
                require_permission(p, Some(role), Permission::WorkflowFolderRename).unwrap_err();
            assert_eq!(
                err.permission(),
                Permission::WorkflowFolderRename,
                "{role:?} must be denied folder rename"
            );
        }
    }

    #[test]
    fn ownership_lets_member_update_any_workflow_but_only_delete_own() {
        // The crux of the collaboration change, at the per-resource layer: with workflow:update
        // now Allow, a Member may update a workflow created by someone else (move rides the same
        // permission, so it follows). Delete stays Own, so a Member may delete only their own.
        let p = MembershipPolicy::Required;
        let other = Some("member-a");

        assert!(
            require_ownership(
                p,
                Some(Role::Member),
                Permission::WorkflowUpdate,
                other,
                "member-b"
            )
            .is_ok(),
            "Member must be able to update a workflow they don't own"
        );
        assert!(
            require_ownership(
                p,
                Some(Role::Member),
                Permission::WorkflowDelete,
                other,
                "member-b"
            )
            .is_err(),
            "Member must not be able to delete a workflow they don't own"
        );
        // Owner/Admin bypass ownership entirely for delete.
        for role in [Role::Owner, Role::Admin] {
            assert!(
                require_ownership(p, Some(role), Permission::WorkflowDelete, other, "member-b")
                    .is_ok(),
                "{role:?} must bypass the delete ownership check"
            );
        }
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

    // ── permission_for: the route → permission table ────────────────────────

    #[test]
    fn permission_for_maps_representative_routes() {
        let cases: &[(Method, &str, Permission)] = &[
            (
                Method::GET,
                "/api/runtime/workflows",
                Permission::WorkflowRead,
            ),
            (
                Method::POST,
                "/api/runtime/workflows/create",
                Permission::WorkflowCreate,
            ),
            (
                Method::POST,
                "/api/runtime/workflows/{id}/update",
                Permission::WorkflowUpdate,
            ),
            (
                Method::POST,
                "/api/runtime/workflows/{id}/delete",
                Permission::WorkflowDelete,
            ),
            (
                Method::PUT,
                "/api/runtime/workflows/{id}/move",
                Permission::WorkflowUpdate,
            ),
            (
                Method::PUT,
                "/api/runtime/workflows/folders/rename",
                Permission::WorkflowFolderRename,
            ),
            (
                Method::POST,
                "/api/runtime/workflows/{id}/execute",
                Permission::WorkflowExecute,
            ),
            (
                Method::POST,
                "/api/runtime/workflows/{id}/clone",
                Permission::WorkflowCreate,
            ),
            (
                Method::POST,
                "/api/runtime/workflows/{id}/versions/{version}/compile",
                Permission::WorkflowExecute,
            ),
            (
                Method::GET,
                "/api/runtime/executions",
                Permission::InvocationHistoryRead,
            ),
            (
                Method::GET,
                "/api/runtime/workflows/{id}/instances",
                Permission::InvocationHistoryRead,
            ),
            (
                Method::GET,
                "/api/runtime/metrics/tenant",
                Permission::AnalyticsRead,
            ),
            (
                Method::POST,
                "/api/runtime/triggers",
                Permission::TriggerCreate,
            ),
            (
                Method::DELETE,
                "/api/runtime/triggers/{id}",
                Permission::TriggerDelete,
            ),
            (
                Method::POST,
                "/api/runtime/reports",
                Permission::ReportCreate,
            ),
            (
                Method::DELETE,
                "/api/runtime/reports/{report_id}",
                Permission::ReportDelete,
            ),
            (
                Method::POST,
                "/api/runtime/object-model/schemas",
                Permission::DatabaseCreate,
            ),
            (
                Method::DELETE,
                "/api/runtime/object-model/schemas/{id}",
                Permission::DatabaseDelete,
            ),
            (
                Method::POST,
                "/api/runtime/object-model/sql/execute",
                Permission::DatabaseDelete,
            ),
            (
                Method::POST,
                "/api/runtime/connections",
                Permission::ConnectionCreate,
            ),
            (
                Method::DELETE,
                "/api/runtime/connections/{id}",
                Permission::ConnectionDelete,
            ),
        ];
        for (method, path, want) in cases {
            assert_eq!(permission_for(method, path), Some(*want), "{method} {path}");
        }
    }

    #[test]
    fn permission_for_is_method_sensitive_on_shared_paths() {
        // Same path, different verb → different permission. This is the property that lets one
        // table gate combined-method routes that a per-route layer could not.
        assert_eq!(
            permission_for(&Method::GET, "/api/runtime/reports"),
            Some(Permission::ReportRead)
        );
        assert_eq!(
            permission_for(&Method::POST, "/api/runtime/reports"),
            Some(Permission::ReportCreate)
        );
        let schema = "/api/runtime/object-model/schemas/{id}";
        assert_eq!(
            permission_for(&Method::GET, schema),
            Some(Permission::DatabaseRead)
        );
        assert_eq!(
            permission_for(&Method::PUT, schema),
            Some(Permission::DatabaseUpdate)
        );
        assert_eq!(
            permission_for(&Method::DELETE, schema),
            Some(Permission::DatabaseDelete)
        );
    }

    #[test]
    fn permission_for_matches_connections_on_both_path_forms() {
        // The nested connections router may surface either the full or the nest-relative
        // template as the matched path; both must resolve identically.
        for path in ["/api/runtime/connections/{id}", "/connections/{id}"] {
            assert_eq!(
                permission_for(&Method::GET, path),
                Some(Permission::ConnectionRead)
            );
            assert_eq!(
                permission_for(&Method::DELETE, path),
                Some(Permission::ConnectionDelete)
            );
        }
        // OAuth authorize is a GET but mutates credentials → not a read.
        assert_eq!(
            permission_for(&Method::GET, "/connections/{id}/oauth/authorize"),
            Some(Permission::ConnectionUpdate)
        );
    }

    #[test]
    fn permission_for_returns_none_for_ungated_routes() {
        // Metadata, agent listings/metadata, the entitlements snapshot, health: deliberately
        // ungated (read-only, or not authz'd by runtara). API-key routes are ungated too — they
        // are personal credentials gated by ownership in the handler, not by role.
        for (method, path) in [
            (Method::GET, "/api/runtime/agents"),
            (Method::GET, "/api/runtime/agents/{name}"),
            (Method::GET, "/api/runtime/specs/dsl"),
            (Method::GET, "/api/runtime/entitlements"),
            (Method::GET, "/health"),
            (Method::GET, "/api/runtime/nonexistent"),
            (Method::GET, "/api/runtime/api-keys"),
            (Method::POST, "/api/runtime/api-keys"),
            (Method::DELETE, "/api/runtime/api-keys/{id}"),
        ] {
            assert_eq!(permission_for(&method, path), None, "{method} {path}");
        }
    }

    #[test]
    fn permission_for_gates_agent_execution() {
        // Host-mediated agent capability I/O is gated like running a workflow.
        let cases: &[(Method, &str, Permission)] = &[
            (
                Method::POST,
                "/api/runtime/agents/{name}/capabilities/{capability_id}/execute",
                Permission::WorkflowExecute,
            ),
            (
                Method::POST,
                "/api/runtime/agents/{name}/capabilities/{capability_id}/test",
                Permission::WorkflowExecute,
            ),
        ];
        for (method, path, want) in cases {
            assert_eq!(permission_for(method, path), Some(*want), "{method} {path}");
        }
    }

    // ── authorize layer: end-to-end with a real matched path ────────────────

    fn inject(
        role: Option<Role>,
    ) -> impl Clone + Send + Sync + 'static + Fn(Request, Next) -> BoxFuture<'static, Response>
    {
        move |mut req: Request, next: Next| {
            let mut ctx = AuthContext::new("tenant".into(), "auth0|u".into(), AuthMethod::Jwt);
            ctx.role = role;
            async move {
                req.extensions_mut().insert(ctx);
                next.run(req).await
            }
            .boxed()
        }
    }

    /// Build an app with two real route templates gated by the `authorize` layer, with an
    /// injected role standing in for `authenticate`.
    fn matched_path_app(role: Option<Role>, policy: MembershipPolicy) -> Router {
        Router::new()
            .route("/api/runtime/workflows", get(|| async { "ok" }))
            .route(
                "/api/runtime/workflows/{id}/delete",
                post(|| async { "ok" }),
            )
            .route_layer(from_fn(authorize(policy)))
            .route_layer(from_fn(inject(role)))
    }

    #[tokio::test]
    async fn authorize_denies_viewer_on_matched_write_route() {
        let app = matched_path_app(Some(Role::Viewer), MembershipPolicy::Required);
        let resp = app
            .oneshot(
                HttpRequest::post("/api/runtime/workflows/wf-1/delete")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
        let bytes = axum::body::to_bytes(resp.into_body(), 4096).await.unwrap();
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["permission"], "workflow:delete");
    }

    #[tokio::test]
    async fn authorize_allows_viewer_on_matched_read_route() {
        let app = matched_path_app(Some(Role::Viewer), MembershipPolicy::Required);
        let resp = app
            .oneshot(
                HttpRequest::get("/api/runtime/workflows")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn authorize_allows_member_write_route_at_route_level() {
        // workflow:delete is Own for Member → clears the route gate (ownership is checked in
        // the handler, not here).
        let app = matched_path_app(Some(Role::Member), MembershipPolicy::Required);
        let resp = app
            .oneshot(
                HttpRequest::post("/api/runtime/workflows/wf-1/delete")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    /// Build an app exposing the real folder-rename route under the `authorize` layer.
    fn folder_rename_app(role: Option<Role>) -> Router {
        Router::new()
            .route(
                "/api/runtime/workflows/folders/rename",
                axum::routing::put(|| async { "ok" }),
            )
            .route_layer(from_fn(authorize(MembershipPolicy::Required)))
            .route_layer(from_fn(inject(role)))
    }

    #[tokio::test]
    async fn authorize_denies_member_on_folder_rename_route() {
        // The collaboration change makes workflow:update tenant-wide for Member, but folder
        // rename is gated by the Owner/Admin-only workflow:folder_rename — a Member is rejected
        // at the route gate, before any handler runs.
        let app = folder_rename_app(Some(Role::Member));
        let resp = app
            .oneshot(
                HttpRequest::put("/api/runtime/workflows/folders/rename")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
        let bytes = axum::body::to_bytes(resp.into_body(), 4096).await.unwrap();
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["permission"], "workflow:folder_rename");
    }

    #[tokio::test]
    async fn authorize_allows_admin_on_folder_rename_route() {
        let app = folder_rename_app(Some(Role::Admin));
        let resp = app
            .oneshot(
                HttpRequest::put("/api/runtime/workflows/folders/rename")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn authorize_is_dormant_under_logging() {
        let app = matched_path_app(Some(Role::Viewer), MembershipPolicy::Logging);
        let resp = app
            .oneshot(
                HttpRequest::post("/api/runtime/workflows/wf-1/delete")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    // ── require_ownership: resource-level Own check ──────────────────────────

    const REQ: MembershipPolicy = MembershipPolicy::Required;

    #[test]
    fn ownership_member_allowed_only_on_own_resource() {
        // Member's workflow:delete is Own → may delete a workflow they created, not another's.
        assert!(
            require_ownership(
                REQ,
                Some(Role::Member),
                Permission::WorkflowDelete,
                Some("u1"),
                "u1"
            )
            .is_ok()
        );
        let err = require_ownership(
            REQ,
            Some(Role::Member),
            Permission::WorkflowDelete,
            Some("u2"),
            "u1",
        )
        .expect_err("Member cannot delete another user's workflow");
        assert_eq!(err.permission(), Permission::WorkflowDelete);
    }

    #[test]
    fn ownership_member_denied_on_unowned_or_missing_resource() {
        // NULL owner (legacy row) or a resource that doesn't exist → Member denied for an Own
        // permission; only Owner/Admin (who get Allow) can manage these. Uses workflow:delete:
        // workflow:update is now flat Allow for Member, so it would not exercise the Own path.
        assert!(
            require_ownership(
                REQ,
                Some(Role::Member),
                Permission::WorkflowDelete,
                None,
                "u1"
            )
            .is_err()
        );
    }

    #[test]
    fn ownership_owner_and_admin_bypass_regardless_of_creator() {
        // Owner/Admin have Allow on update/delete → ownership never consulted.
        for role in [Role::Owner, Role::Admin] {
            assert!(
                require_ownership(
                    REQ,
                    Some(role),
                    Permission::WorkflowDelete,
                    Some("someone-else"),
                    "me"
                )
                .is_ok()
            );
            // even an unowned row:
            assert!(
                require_ownership(REQ, Some(role), Permission::WorkflowDelete, None, "me").is_ok()
            );
        }
    }

    #[test]
    fn ownership_allow_scoped_permission_skips_owner_check() {
        // database:delete is flat Allow for Member (no per-row owner) → ownership not consulted,
        // even against a resource owned by someone else.
        assert!(
            require_ownership(
                REQ,
                Some(Role::Member),
                Permission::DatabaseDelete,
                Some("someone-else"),
                "me"
            )
            .is_ok()
        );
    }

    #[test]
    fn ownership_dormant_unless_required() {
        // Under Logging/Disabled the check never blocks, even a non-owner Member.
        for policy in [MembershipPolicy::Disabled, MembershipPolicy::Logging] {
            assert!(
                require_ownership(
                    policy,
                    Some(Role::Member),
                    Permission::WorkflowDelete,
                    Some("u2"),
                    "u1"
                )
                .is_ok()
            );
        }
    }

    #[test]
    fn ownership_no_role_passes() {
        // Trusted internal callers (role None) are not ownership-checked.
        assert!(require_ownership(REQ, None, Permission::WorkflowDelete, Some("u2"), "u1").is_ok());
    }

    // ── coverage guard: the known mutating routes stay gated ─────────────────

    /// Regression guard for the fail-open class the external review found: every mutating
    /// `/api/runtime/*` route we know about must resolve to a permission, not `None`. New
    /// routes still need a `permission_for` arm, but this fails loudly if an existing mapping
    /// is dropped. (A full router-introspection check is a heavier follow-up.)
    #[test]
    fn known_mutating_routes_are_all_gated() {
        let mutating: &[(Method, &str)] = &[
            (Method::POST, "/api/runtime/workflows/create"),
            (Method::POST, "/api/runtime/workflows/{id}/update"),
            (
                Method::PUT,
                "/api/runtime/workflows/{id}/versions/{version}/graph",
            ),
            (Method::POST, "/api/runtime/workflows/{id}/delete"),
            (Method::PUT, "/api/runtime/workflows/{id}/move"),
            (Method::PUT, "/api/runtime/workflows/folders/rename"),
            (Method::POST, "/api/runtime/workflows/{id}/execute"),
            (Method::POST, "/api/runtime/workflows/{id}/clone"),
            (
                Method::POST,
                "/api/runtime/agents/{name}/capabilities/{capability_id}/execute",
            ),
            (
                Method::POST,
                "/api/runtime/agents/{name}/capabilities/{capability_id}/test",
            ),
            (Method::POST, "/api/runtime/triggers"),
            (Method::PUT, "/api/runtime/triggers/{id}"),
            (Method::DELETE, "/api/runtime/triggers/{id}"),
            (Method::POST, "/api/runtime/reports"),
            (Method::PUT, "/api/runtime/reports/{report_id}"),
            (Method::DELETE, "/api/runtime/reports/{report_id}"),
            (Method::POST, "/api/runtime/object-model/schemas"),
            (Method::DELETE, "/api/runtime/object-model/schemas/{id}"),
            (Method::POST, "/api/runtime/object-model/sql/execute"),
            (Method::POST, "/api/runtime/connections"),
            (Method::DELETE, "/api/runtime/connections/{id}"),
            // API-key routes are deliberately NOT in this list: they are personal credentials
            // gated by ownership in the handler (filter on `issuing_user_id`), not by role.
        ];
        for (method, path) in mutating {
            assert!(
                permission_for(method, path).is_some(),
                "mutating route is ungated (fail-open): {method} {path}"
            );
        }
    }
}
