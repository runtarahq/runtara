use axum::{
    extract::{Request, State},
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Json, Response},
};
use serde_json::json;
use tracing::Instrument;

use crate::auth::{AuthContext, AuthMethod, AuthState, MembershipPolicy};
use crate::authz::Role;
use crate::valkey::auth::{AuthzValkeyError, get_member_role, token_is_revoked};

/// Authentication middleware. Defers to the configured `AuthProvider` for everything
/// except the in-process bypass and the RUNTARA-issued API-key fast path.
///
/// For every request:
/// 1. If an `AuthContext` is already in extensions, pass through (trusted in-process
///    call, e.g. from MCP tools via `Router::oneshot`).
/// 2. If `Authorization: Bearer rt_*|smo_*`, validate via the local API-key table.
///    API keys work regardless of `AUTH_PROVIDER` — they are an operator escape hatch.
/// 3. Otherwise, call `auth_state.provider.authenticate(headers)` and use its result.
///
/// The tenant-mismatch check that used to live here now sits inside `OidcProvider`;
/// `LocalProvider` and `TrustProxyProvider` set `org_id` from the configured tenant by
/// construction, so there is nothing to mismatch.
pub async fn authenticate(
    State(auth_state): State<AuthState>,
    mut request: Request,
    next: Next,
) -> Response {
    if let Some(ctx) = request.extensions().get::<AuthContext>().cloned() {
        return next.run(request).instrument(auth_span(&ctx)).await;
    }

    // Fast path: RUNTARA-issued API key. Works in every provider mode.
    if let Some(token) = api_key_token(request.headers()) {
        let auth_context = match validate_api_key(token, &auth_state).await {
            Ok(ctx) => ctx,
            Err(response) => return response,
        };
        let span = auth_span(&auth_context);
        request.extensions_mut().insert(auth_context);
        return next.run(request).instrument(span).await;
    }

    // Delegate everything else to the configured provider.
    let mut auth_context = match auth_state.provider.authenticate(request.headers()).await {
        Ok(ctx) => ctx,
        Err(e) => return e.into_http_response(),
    };

    // SYN-437: resolve per-tenant membership + token revocation from Valkey, attaching the
    // caller's role. Governed by `membership_policy`; fails the request closed under
    // `Required`.
    if let Err(response) = enforce_membership(&auth_state, &mut auth_context).await {
        return response;
    }

    let span = auth_span(&auth_context);
    request.extensions_mut().insert(auth_context);
    next.run(request).instrument(span).await
}

/// Span carrying the resolved auth identity, used to wrap the rest of the
/// request future so any downstream `tracing` event inherits `user_id` and
/// `auth_method` fields. Subscribers that flatten parent-span fields onto
/// each emitted event (JSON formatter, OTLP exporter) surface these
/// alongside entitlement-denial warns, satisfying the Phase 6 audit
/// requirement that denial logs identify the caller without per-line
/// plumbing through every gate.
fn auth_span(ctx: &AuthContext) -> tracing::Span {
    tracing::info_span!(
        "request_auth",
        user_id = %ctx.user_id,
        auth_method = ?ctx.auth_method,
        role = ?ctx.role,
    )
}

/// Extract a Bearer token from the `Authorization` header if and only if it looks like
/// a RUNTARA-issued API key. Returns `None` for JWTs, missing headers, or empty tokens
/// so the provider path handles them.
fn api_key_token(headers: &axum::http::HeaderMap) -> Option<&str> {
    let value = headers.get("Authorization")?.to_str().ok()?;
    let token = value.strip_prefix("Bearer ").unwrap_or(value);
    if token.is_empty() {
        return None;
    }
    if token.starts_with("rt_") || token.starts_with("smo_") {
        Some(token)
    } else {
        None
    }
}

/// Validate an API key token via the local database.
async fn validate_api_key(token: &str, auth_state: &AuthState) -> Result<AuthContext, Response> {
    use sha2::Digest;
    let key_hash = hex::encode(sha2::Sha256::digest(token.as_bytes()));

    let api_key =
        crate::api::handlers::api_keys::validate_api_key_by_hash(&auth_state.pool, &key_hash)
            .await
            .map_err(|e| {
                tracing::warn!(error = %e, "API key validation failed");
                unauthorized("Invalid or expired API key")
            })?;

    Ok(AuthContext::new(
        api_key.org_id,
        "api-key".to_string(),
        AuthMethod::ApiKey,
    ))
}

fn unauthorized(message: &str) -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(json!({
            "error": "Unauthorized",
            "message": message
        })),
    )
        .into_response()
}

/// Why a membership/revocation check rejected (or would reject) a request. Maps to the
/// failure-stance table in `docs/security/user-management-contracts.md` §7.
#[derive(Debug)]
enum MembershipDenial {
    /// JWT carried no `sub` — there is no subject to look up.
    MissingSubject,
    /// JWT carried no `jti` — the token cannot be checked against the revocation denylist.
    MissingJti,
    /// `token:revoked:{jti}` is present.
    TokenRevoked,
    /// No `member:{sub}` entry — the user is not a member of this tenant.
    NotAMember,
    /// `member:{sub}` existed but did not match the contract (bad JSON / unknown role).
    MalformedRecord(String),
    /// Valkey was unreachable or not configured — fail closed, loud alert.
    ValkeyUnavailable(String),
}

impl MembershipDenial {
    /// Stable error code surfaced to the client and logs.
    fn code(&self) -> &'static str {
        match self {
            MembershipDenial::MissingSubject => "MISSING_SUBJECT",
            MembershipDenial::MissingJti => "MISSING_JTI",
            MembershipDenial::TokenRevoked => "TOKEN_REVOKED",
            MembershipDenial::NotAMember => "NOT_A_MEMBER",
            MembershipDenial::MalformedRecord(_) => "MALFORMED_MEMBER_RECORD",
            MembershipDenial::ValkeyUnavailable(_) => "AUTH_MEMBERSHIP_UNAVAILABLE",
        }
    }

    fn status(&self) -> StatusCode {
        match self {
            // Token-shape / revocation problems are a 401 (re-authenticate).
            MembershipDenial::MissingSubject
            | MembershipDenial::MissingJti
            | MembershipDenial::TokenRevoked => StatusCode::UNAUTHORIZED,
            // Authenticated but not authorized for this tenant.
            MembershipDenial::NotAMember | MembershipDenial::MalformedRecord(_) => {
                StatusCode::FORBIDDEN
            }
            // Infrastructure failure — distinct from bad credentials.
            MembershipDenial::ValkeyUnavailable(_) => StatusCode::SERVICE_UNAVAILABLE,
        }
    }

    fn message(&self) -> &'static str {
        match self {
            MembershipDenial::MissingSubject => "Token has no subject claim",
            MembershipDenial::MissingJti => "Token has no jti claim",
            MembershipDenial::TokenRevoked => "Token has been revoked",
            MembershipDenial::NotAMember => "Not a member of this tenant",
            MembershipDenial::MalformedRecord(_) => "Membership record is malformed",
            MembershipDenial::ValkeyUnavailable(_) => "Membership store unavailable",
        }
    }

    fn into_response(self) -> Response {
        let status = self.status();
        let code = self.code();
        let message = self.message();
        (status, Json(json!({ "error": code, "message": message }))).into_response()
    }

    /// Emit a structured log. Valkey-unavailability is a loud `error!` (operator alert); the
    /// rest are `warn!`. `enforced` distinguishes a real block from a `Logging`-mode shadow.
    fn log(&self, ctx: &AuthContext, enforced: bool) {
        match self {
            MembershipDenial::ValkeyUnavailable(detail) => tracing::error!(
                user_id = %ctx.user_id,
                code = self.code(),
                detail = %detail,
                enforced,
                "membership lookup failed: Valkey unavailable"
            ),
            MembershipDenial::MalformedRecord(detail) => tracing::warn!(
                user_id = %ctx.user_id,
                code = self.code(),
                detail = %detail,
                enforced,
                "membership check denied"
            ),
            _ => tracing::warn!(
                user_id = %ctx.user_id,
                code = self.code(),
                enforced,
                "membership check denied"
            ),
        }
    }
}

/// Apply the per-tenant membership/revocation policy to a freshly authenticated request.
///
/// - `Disabled`: no Valkey lookup; role stays unset.
/// - non-JWT auth (local / trust_proxy / api-key): skipped here. API-key role inheritance is
///   a later phase.
/// - `Required`: any failure rejects the request closed.
/// - `Logging`: failures are logged (what `Required` *would* do) but never block; a
///   successfully resolved role is still attached.
async fn enforce_membership(auth_state: &AuthState, ctx: &mut AuthContext) -> Result<(), Response> {
    if auth_state.membership_policy == MembershipPolicy::Disabled {
        return Ok(());
    }
    if ctx.auth_method != AuthMethod::Jwt {
        return Ok(());
    }

    match resolve_membership(auth_state, ctx).await {
        Ok(role) => {
            ctx.role = role;
            Ok(())
        }
        Err(denial) => {
            let enforced = auth_state.membership_policy == MembershipPolicy::Required;
            denial.log(ctx, enforced);
            if enforced {
                Err(denial.into_response())
            } else {
                Ok(())
            }
        }
    }
}

/// Run the contract sequence: validate token shape, check the revocation denylist, then read
/// the membership role. Token-shape checks come first so a misconfigured Valkey doesn't mask
/// a plainly invalid token.
async fn resolve_membership(
    auth_state: &AuthState,
    ctx: &AuthContext,
) -> Result<Option<Role>, MembershipDenial> {
    if ctx.user_id.is_empty() {
        return Err(MembershipDenial::MissingSubject);
    }
    let Some(jti) = ctx.jti.as_deref() else {
        return Err(MembershipDenial::MissingJti);
    };
    let Some(manager) = auth_state.valkey.as_ref() else {
        return Err(MembershipDenial::ValkeyUnavailable(
            "Valkey not configured".to_string(),
        ));
    };

    match token_is_revoked(manager, jti).await {
        Ok(true) => return Err(MembershipDenial::TokenRevoked),
        Ok(false) => {}
        Err(e) => return Err(MembershipDenial::ValkeyUnavailable(e.to_string())),
    }

    match get_member_role(manager, &ctx.user_id).await {
        Ok(Some(role)) => Ok(Some(role)),
        Ok(None) => Err(MembershipDenial::NotAMember),
        Err(AuthzValkeyError::Parse(detail)) => Err(MembershipDenial::MalformedRecord(detail)),
        Err(AuthzValkeyError::Redis(e)) => Err(MembershipDenial::ValkeyUnavailable(e.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use redis::AsyncCommands;

    use super::*;
    use crate::auth::providers::LocalProvider;
    use crate::auth::{AuthProvider, MembershipPolicy};

    fn state(policy: MembershipPolicy, valkey: Option<redis::aio::ConnectionManager>) -> AuthState {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://localhost/runtara_test_unused")
            .expect("lazy pool never connects in these tests");
        let provider = Arc::new(LocalProvider::new("tenant".to_string())) as Arc<dyn AuthProvider>;
        AuthState {
            provider,
            pool,
            valkey,
            membership_policy: policy,
        }
    }

    fn jwt_ctx(user_id: &str, jti: Option<&str>) -> AuthContext {
        let mut ctx = AuthContext::new("org".to_string(), user_id.to_string(), AuthMethod::Jwt);
        ctx.jti = jti.map(|s| s.to_string());
        ctx
    }

    #[test]
    fn denial_mapping_matches_contract() {
        let cases: &[(MembershipDenial, StatusCode, &str)] = &[
            (
                MembershipDenial::MissingSubject,
                StatusCode::UNAUTHORIZED,
                "MISSING_SUBJECT",
            ),
            (
                MembershipDenial::MissingJti,
                StatusCode::UNAUTHORIZED,
                "MISSING_JTI",
            ),
            (
                MembershipDenial::TokenRevoked,
                StatusCode::UNAUTHORIZED,
                "TOKEN_REVOKED",
            ),
            (
                MembershipDenial::NotAMember,
                StatusCode::FORBIDDEN,
                "NOT_A_MEMBER",
            ),
            (
                MembershipDenial::MalformedRecord("x".into()),
                StatusCode::FORBIDDEN,
                "MALFORMED_MEMBER_RECORD",
            ),
            (
                MembershipDenial::ValkeyUnavailable("x".into()),
                StatusCode::SERVICE_UNAVAILABLE,
                "AUTH_MEMBERSHIP_UNAVAILABLE",
            ),
        ];
        for (denial, status, code) in cases {
            assert_eq!(denial.status(), *status, "status for {code}");
            assert_eq!(denial.code(), *code);
        }
    }

    #[tokio::test]
    async fn disabled_policy_skips_lookup() {
        let mut ctx = jwt_ctx("auth0|u", None);
        let st = state(MembershipPolicy::Disabled, None);
        assert!(enforce_membership(&st, &mut ctx).await.is_ok());
        assert_eq!(ctx.role, None);
    }

    #[tokio::test]
    async fn non_jwt_requests_are_skipped() {
        let mut ctx = AuthContext::new("org".into(), "local".into(), AuthMethod::Unauthenticated);
        let st = state(MembershipPolicy::Required, None);
        assert!(enforce_membership(&st, &mut ctx).await.is_ok());
    }

    #[tokio::test]
    async fn logging_policy_never_blocks() {
        // Valkey is None -> would be ValkeyUnavailable under Required, but Logging allows it.
        let mut ctx = jwt_ctx("auth0|u", Some("jti-1"));
        let st = state(MembershipPolicy::Logging, None);
        assert!(enforce_membership(&st, &mut ctx).await.is_ok());
        assert_eq!(ctx.role, None);
    }

    #[tokio::test]
    async fn required_without_valkey_fails_closed_503() {
        let mut ctx = jwt_ctx("auth0|u", Some("jti-1"));
        let st = state(MembershipPolicy::Required, None);
        let resp = enforce_membership(&st, &mut ctx).await.unwrap_err();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn required_missing_jti_is_401() {
        let mut ctx = jwt_ctx("auth0|u", None);
        let st = state(MembershipPolicy::Required, None);
        let resp = enforce_membership(&st, &mut ctx).await.unwrap_err();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    // --- Live Valkey round-trips (skip cleanly without VALKEY_HOST) ---

    macro_rules! manager_or_skip {
        () => {
            match crate::valkey::ValkeyConfig::from_env() {
                Some(cfg) => crate::valkey::get_or_create_manager(&cfg.connection_url())
                    .await
                    .expect("connect valkey"),
                None => {
                    eprintln!("Skipping test: VALKEY_HOST not set");
                    return;
                }
            }
        };
    }

    fn unique(prefix: &str) -> String {
        format!("{}-{}", prefix, uuid::Uuid::new_v4())
    }

    #[tokio::test]
    async fn required_attaches_role_for_member() {
        let manager = manager_or_skip!();
        let uid = unique("auth0|member");
        let mut conn = manager.clone();
        let _: () = conn
            .set(
                format!("member:{uid}"),
                r#"{"role":"admin","updated_at":"2026-05-28T12:00:00Z"}"#,
            )
            .await
            .expect("seed member");

        let mut ctx = jwt_ctx(&uid, Some(&unique("jti")));
        let st = state(MembershipPolicy::Required, Some(manager));
        assert!(enforce_membership(&st, &mut ctx).await.is_ok());
        assert_eq!(ctx.role, Some(Role::Admin));

        let _: () = conn.del(format!("member:{uid}")).await.expect("cleanup");
    }

    #[tokio::test]
    async fn required_denies_non_member_403() {
        let manager = manager_or_skip!();
        let mut ctx = jwt_ctx(&unique("auth0|ghost"), Some(&unique("jti")));
        let st = state(MembershipPolicy::Required, Some(manager));
        let resp = enforce_membership(&st, &mut ctx).await.unwrap_err();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
        assert_eq!(ctx.role, None);
    }

    #[tokio::test]
    async fn required_denies_revoked_token_401() {
        let manager = manager_or_skip!();
        let uid = unique("auth0|member");
        let jti = unique("jti");
        let mut conn = manager.clone();
        let _: () = conn
            .set(format!("member:{uid}"), r#"{"role":"member"}"#)
            .await
            .expect("seed member");
        let _: () = conn
            .set(
                format!("token:revoked:{jti}"),
                r#"{"revoked_at":"2026-05-28T12:00:00Z"}"#,
            )
            .await
            .expect("seed revocation");

        let mut ctx = jwt_ctx(&uid, Some(&jti));
        let st = state(MembershipPolicy::Required, Some(manager));
        let resp = enforce_membership(&st, &mut ctx).await.unwrap_err();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

        let _: () = conn.del(format!("member:{uid}")).await.expect("cleanup");
        let _: () = conn
            .del(format!("token:revoked:{jti}"))
            .await
            .expect("cleanup");
    }
}
