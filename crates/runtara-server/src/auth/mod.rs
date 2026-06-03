pub mod jwks;
pub mod jwt_validator;
pub mod provider;
pub mod providers;

use std::sync::Arc;

use redis::aio::ConnectionManager;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;

use crate::authz::Role;

pub use provider::{AuthError, AuthProvider, AuthProviderKind, AuthProviders};

/// JWT configuration consumed by `OidcProvider`. Parsed in the provider factory; other
/// modes ignore these fields entirely.
#[derive(Debug, Clone)]
pub struct JwtConfig {
    pub jwks_uri: String,
    pub issuer: String,
    pub audience: Option<String>,
    /// When true, a JWT without a `jti` claim is rejected. Off during rollout before the
    /// Auth0 Action emits `jti`, flipped on once every token carries it. Driven by
    /// `RUNTARA_AUTH_REQUIRE_JTI`. See `docs/security/user-management-contracts.md`.
    pub require_jti: bool,
}

/// Authentication context inserted into request extensions after successful auth.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthContext {
    pub org_id: String,
    pub user_id: String,
    pub auth_method: AuthMethod,
    /// Caller's role in this tenant. `None` outside SaaS enforcement — non-JWT modes,
    /// trusted internal calls, and the rollout transition before the Valkey membership
    /// lookup populates it. JWTs never carry the role; it is read from the
    /// per-tenant Valkey `member:{sub}` entry.
    pub role: Option<Role>,
    /// Token identity (`jti` claim / API-key token id). Key for the revocation denylist.
    pub jti: Option<String>,
    /// Identity claims passed through from the JWT, for logging and the `/me` response.
    pub email: Option<String>,
    pub name: Option<String>,
    /// Human-readable tenant identifier. Logging and `/me` only — `org_id` is the tenant key.
    pub tenant_slug: Option<String>,
}

impl AuthContext {
    /// Construct an `AuthContext` with no enriched identity (no role, jti, email, name, or
    /// tenant_slug). Used by non-JWT auth paths and trusted internal callers; the JWT path
    /// builds the struct directly so it can thread the token claims.
    pub fn new(org_id: String, user_id: String, auth_method: AuthMethod) -> Self {
        Self {
            org_id,
            user_id,
            auth_method,
            role: None,
            jti: None,
            email: None,
            name: None,
            tenant_slug: None,
        }
    }
}

/// How the request was authenticated.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum AuthMethod {
    Jwt,
    ApiKey,
    /// No per-request auth was performed inside RUNTARA — request was either trusted
    /// unconditionally (`local`) or trusted because it arrived through a reverse proxy
    /// that terminated auth upstream (`trust_proxy`).
    Unauthenticated,
}

/// How runtara treats the per-tenant Valkey membership/revocation lookup.
///
/// One env var (`RUNTARA_AUTH_MEMBERSHIP_POLICY=disabled|logging|required`) is the only
/// switch; the rollout moves it `Disabled` → `Logging` → `Required`. The auth middleware
/// consumes it; [`AuthState`] carries the policy and the Valkey handle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MembershipPolicy {
    /// No Valkey lookup at all — `local` mode, or dev with no Valkey configured.
    Disabled,
    /// Look up membership + token revocation and log what would be denied, but never block.
    /// The initial observe-only rollout posture.
    Logging,
    /// Fail closed on missing membership, a revoked token, or an unreachable Valkey.
    Required,
}

impl MembershipPolicy {
    /// Resolve the policy. An explicit `RUNTARA_AUTH_MEMBERSHIP_POLICY` always wins;
    /// otherwise the default is derived from the auth mode and whether Valkey is configured.
    ///
    /// Defaults: `oidc` with Valkey → `Logging` (non-blocking until an operator opts into
    /// `Required`); `oidc` without Valkey, `local`, and `trust_proxy` → `Disabled`.
    /// Panics on an unrecognized explicit value — a typo'd security policy must fail fast,
    /// not silently fall back.
    pub fn from_env(kind: AuthProviderKind, valkey_configured: bool) -> Self {
        match std::env::var("RUNTARA_AUTH_MEMBERSHIP_POLICY")
            .ok()
            .as_deref()
        {
            Some(explicit) => Self::parse(explicit).unwrap_or_else(|| {
                panic!(
                    "Unknown RUNTARA_AUTH_MEMBERSHIP_POLICY value: '{explicit}'. \
                     Must be one of: disabled, logging, required"
                )
            }),
            None => Self::default_for(kind, valkey_configured),
        }
    }

    /// Parse an explicit policy string. `None` for an unrecognized value.
    fn parse(s: &str) -> Option<Self> {
        match s {
            "disabled" => Some(MembershipPolicy::Disabled),
            "logging" => Some(MembershipPolicy::Logging),
            "required" => Some(MembershipPolicy::Required),
            _ => None,
        }
    }

    /// The default when `RUNTARA_AUTH_MEMBERSHIP_POLICY` is unset.
    fn default_for(kind: AuthProviderKind, valkey_configured: bool) -> Self {
        match kind {
            AuthProviderKind::Oidc if valkey_configured => MembershipPolicy::Logging,
            AuthProviderKind::Oidc | AuthProviderKind::Local | AuthProviderKind::TrustProxy => {
                MembershipPolicy::Disabled
            }
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            MembershipPolicy::Disabled => "disabled",
            MembershipPolicy::Logging => "logging",
            MembershipPolicy::Required => "required",
        }
    }
}

/// Shared authentication state passed to the middleware. The middleware handles the
/// in-process bypass and the API-key fast path, then defers to `provider` for
/// everything else.
#[derive(Clone)]
pub struct AuthState {
    pub provider: Arc<dyn AuthProvider>,
    pub pool: PgPool,
    /// Per-tenant Valkey handle for membership/revocation reads. `None` when Valkey is not
    /// configured; the membership policy governs whether that is fatal.
    pub valkey: Option<ConnectionManager>,
    pub membership_policy: MembershipPolicy,
}

#[cfg(test)]
mod tests {
    use super::{AuthProviderKind, MembershipPolicy};

    #[test]
    fn parse_explicit_policy() {
        assert_eq!(
            MembershipPolicy::parse("disabled"),
            Some(MembershipPolicy::Disabled)
        );
        assert_eq!(
            MembershipPolicy::parse("logging"),
            Some(MembershipPolicy::Logging)
        );
        assert_eq!(
            MembershipPolicy::parse("required"),
            Some(MembershipPolicy::Required)
        );
        assert_eq!(MembershipPolicy::parse("Required"), None);
        assert_eq!(MembershipPolicy::parse("on"), None);
    }

    #[test]
    fn as_str_round_trips_parse() {
        for policy in [
            MembershipPolicy::Disabled,
            MembershipPolicy::Logging,
            MembershipPolicy::Required,
        ] {
            assert_eq!(MembershipPolicy::parse(policy.as_str()), Some(policy));
        }
    }

    #[test]
    fn default_policy_per_mode() {
        use AuthProviderKind::{Local, Oidc, TrustProxy};
        use MembershipPolicy::{Disabled, Logging};

        // OIDC only defaults to a live lookup when Valkey is actually configured.
        assert_eq!(MembershipPolicy::default_for(Oidc, true), Logging);
        assert_eq!(MembershipPolicy::default_for(Oidc, false), Disabled);

        // Unauthenticated-style modes never look up membership by default, Valkey or not.
        assert_eq!(MembershipPolicy::default_for(Local, true), Disabled);
        assert_eq!(MembershipPolicy::default_for(Local, false), Disabled);
        assert_eq!(MembershipPolicy::default_for(TrustProxy, true), Disabled);
        assert_eq!(MembershipPolicy::default_for(TrustProxy, false), Disabled);
    }
}
