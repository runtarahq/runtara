pub mod jwks;
pub mod jwt_validator;
pub mod presented;
pub mod provider;
pub mod providers;

use std::sync::{Arc, OnceLock};

use redis::aio::ConnectionManager;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;

use crate::authz::{ApiKeyScope, Role};

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
    /// When true, a JWT that *omits* the `aud` claim is rejected (in addition to the
    /// default present-but-must-match behavior). Only set on the MCP provider when the
    /// strict flag `RUNTARA_MCP_REQUIRE_AUDIENCE` is on — which already guarantees
    /// `audience` is `Some`. Off (default) everywhere else, including the whole API path,
    /// so a missing `aud` keeps today's lax pass-through. See SYN-522.
    pub require_audience_present: bool,
    /// When true (the default), a JWT without an `org_id` claim is rejected. Off only
    /// when membership is required (`RUNTARA_AUTH_MEMBERSHIP_POLICY=required`): then the
    /// request names its tenant and the caller's membership there is what authorizes it.
    pub require_org_id: bool,
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
    /// How much of `role` this credential may actually exercise. [`ApiKeyScope::Full`] for JWTs
    /// and for every unscoped API key — the scope narrows an API key's inherited role and can
    /// never widen it. Enforced in [`crate::middleware::authorization`] ahead of the role gate,
    /// and inherited by MCP tool calls, which run their in-process API calls under a clone of
    /// this context.
    #[serde(default)]
    pub api_key_scope: ApiKeyScope,
    /// Token identity (`jti` claim / API-key token id). Key for the revocation denylist.
    pub jti: Option<String>,
    /// Identity claims passed through from the JWT, for logging and the `/me` response.
    pub email: Option<String>,
    pub name: Option<String>,
    /// Human-readable tenant identifier. Logging and `/me` only — `org_id` is the tenant key.
    pub tenant_slug: Option<String>,
    /// The JWT's issuer. With the subject it is the caller's identity; one deployment
    /// trusts one issuer, so membership is keyed by the subject alone.
    #[serde(default)]
    pub issuer: Option<String>,
    /// The one conversation this credential is held to, where it is a
    /// conversation token: it reads and sends there and does nothing else.
    #[serde(default)]
    pub conversation: Option<ConversationGrant>,
}

/// What a conversation token lets its holder do: read and send in one
/// conversation, as `sender`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationGrant {
    pub workflow_id: String,
    pub key: String,
    #[serde(default)]
    pub sender: Option<crate::api::services::conversations::Sender>,
}

impl ConversationGrant {
    /// Whether a request of `method` to `path` (as routed, a served tenant's
    /// segment taken out) is one this grant lets through: the conversation's
    /// own state, messages, events and stop, of this conversation.
    pub fn admits(&self, method: &axum::http::Method, path: &str) -> bool {
        use axum::http::Method;
        let Some(rest) = path.strip_prefix("/api/runtime/workflows/") else {
            return false;
        };
        let mut segments = rest.split('/');
        let (Some(workflow), Some("conversations"), Some(key)) =
            (segments.next(), segments.next(), segments.next())
        else {
            return false;
        };
        let key = urlencoding::decode(key).map(|key| key.into_owned());
        if workflow != self.workflow_id || key.as_deref() != Ok(self.key.as_str()) {
            return false;
        }
        matches!(
            (method, segments.next(), segments.next()),
            (&Method::GET, None, _)
                | (&Method::POST, Some("messages"), None)
                | (&Method::GET, Some("events"), None)
                | (&Method::POST, Some("stop"), None)
        )
    }
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
            api_key_scope: ApiKeyScope::Full,
            jti: None,
            email: None,
            name: None,
            tenant_slug: None,
            issuer: None,
            conversation: None,
        }
    }
}

/// How the request was authenticated.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub enum AuthMethod {
    Jwt,
    ApiKey,
    /// A conversation token: a member's, held to one conversation.
    ConversationToken,
    /// No per-request auth was performed inside RUNTARA — request was either trusted
    /// unconditionally (`local`) or trusted because it arrived through a reverse proxy
    /// that terminated auth upstream (`trust_proxy`).
    Unauthenticated,
}

impl AuthMethod {
    /// Stable lowercase identifier for structured logs and metric attributes. Kept distinct
    /// from the `Debug` form so log/metric consumers have a contract that does not move with
    /// the enum's `derive(Debug)` output.
    pub fn as_str(self) -> &'static str {
        match self {
            AuthMethod::Jwt => "jwt",
            AuthMethod::ApiKey => "api_key",
            AuthMethod::Unauthenticated => "unauthenticated",
            AuthMethod::ConversationToken => "conversation_token",
        }
    }
}

/// How runtara treats the per-tenant Valkey membership/revocation lookup.
///
/// One env var (`RUNTARA_AUTH_MEMBERSHIP_POLICY=disabled|logging|required`) is the only
/// switch; the rollout moves it `Disabled` (the default everywhere) → `Logging` →
/// `Required`, per tenant, explicitly. The auth middleware consumes it; [`AuthState`]
/// carries the policy and the Valkey handle.
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
    /// otherwise the default is `Disabled` in every mode: a deploy with no extra
    /// configuration must stay quiet — no per-request Valkey lookups, no shadow-denial
    /// logs — until an operator explicitly opts a tenant into the `logging` → `required`
    /// rollout. Panics on an unrecognized explicit value — a typo'd security policy must
    /// fail fast, not silently fall back.
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

    /// The default when `RUNTARA_AUTH_MEMBERSHIP_POLICY` is unset: `Disabled`, regardless of
    /// mode or Valkey availability. The parameters are kept so the signature documents what
    /// the default deliberately does NOT depend on (an earlier draft derived `Logging` from
    /// `oidc`+Valkey, which made a config-free deploy emit a shadow-denial warn on every
    /// request).
    fn default_for(_kind: AuthProviderKind, _valkey_configured: bool) -> Self {
        MembershipPolicy::Disabled
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

    pub fn as_str(self) -> &'static str {
        match self {
            MembershipPolicy::Disabled => "disabled",
            MembershipPolicy::Logging => "logging",
            MembershipPolicy::Required => "required",
        }
    }
}

/// Process-wide resolved membership policy, set once at startup. Handler-level authorization
/// (the `Own` ownership check) reads it to know whether enforcement is active, without
/// threading `AuthState` into every handler. The route-gate layer captures the same value at
/// wiring time; this is the same policy, exposed for code paths that have no layer handle.
static MEMBERSHIP_POLICY: OnceLock<MembershipPolicy> = OnceLock::new();

/// Record the resolved membership policy. Called once during server start, before any request
/// is served. A second call is ignored (the first wins).
pub fn set_membership_policy(policy: MembershipPolicy) {
    let _ = MEMBERSHIP_POLICY.set(policy);
}

/// The resolved membership policy. Defaults to [`MembershipPolicy::Disabled`] when unset (tests
/// and any context that never called [`set_membership_policy`]), i.e. no enforcement — the safe
/// default that preserves pre-enforcement behavior.
pub fn membership_policy() -> MembershipPolicy {
    MEMBERSHIP_POLICY
        .get()
        .copied()
        .unwrap_or(MembershipPolicy::Disabled)
}

/// Shared authentication state passed to the middleware. The middleware handles the
/// in-process bypass and the API-key fast path, then defers to `provider` for
/// everything else.
#[derive(Clone)]
pub struct AuthState {
    /// Which way in this state authenticates callers of.
    pub door: presented::Door,
    pub provider: Arc<dyn AuthProvider>,
    pub pool: PgPool,
    /// Per-tenant Valkey handle for membership/revocation reads. `None` when Valkey is not
    /// configured; the membership policy governs whether that is fatal.
    pub valkey: Option<ConnectionManager>,
    pub membership_policy: MembershipPolicy,
}

#[cfg(test)]
mod tests {
    use super::{AuthMethod, AuthProviderKind, MembershipPolicy};

    #[test]
    fn auth_method_as_str_is_stable() {
        // Structured logs and metric attributes depend on these exact strings; they must not
        // drift with the enum's derived Debug form.
        assert_eq!(AuthMethod::Jwt.as_str(), "jwt");
        assert_eq!(AuthMethod::ApiKey.as_str(), "api_key");
        assert_eq!(AuthMethod::Unauthenticated.as_str(), "unauthenticated");
    }

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
        use MembershipPolicy::Disabled;

        // A deploy with no explicit policy stays quiet in EVERY mode — enforcement (and
        // its shadow-denial logging) is strictly opt-in via the env var.
        for kind in [Oidc, Local, TrustProxy] {
            assert_eq!(MembershipPolicy::default_for(kind, true), Disabled);
            assert_eq!(MembershipPolicy::default_for(kind, false), Disabled);
        }
    }
}

#[cfg(test)]
mod conversation_grant_tests {
    use super::ConversationGrant;
    use axum::http::Method;

    /// A grant lets through its own conversation's state, messages, events
    /// and stop, and nothing else: not another conversation, another
    /// workflow, the conversation's other routes, or any other route.
    #[test]
    fn a_grant_admits_its_conversations_four_routes_and_nothing_else() {
        let grant = ConversationGrant {
            workflow_id: "wf-1".into(),
            key: "chat:42/a b".into(),
            sender: None,
        };
        let own = "/api/runtime/workflows/wf-1/conversations/chat%3A42%2Fa%20b";
        for (method, path) in [
            (Method::GET, own.to_owned()),
            (Method::POST, format!("{own}/messages")),
            (Method::GET, format!("{own}/events")),
            (Method::POST, format!("{own}/stop")),
        ] {
            assert!(grant.admits(&method, &path), "{method} {path}");
        }
        for (method, path) in [
            // Its conversation's other routes.
            (Method::PUT, own.to_owned()),
            (Method::DELETE, own.to_owned()),
            (Method::POST, format!("{own}/token")),
            (Method::GET, format!("{own}/messages")),
            (Method::GET, format!("{own}/events/more")),
            // Another conversation, another workflow, the list.
            (
                Method::GET,
                "/api/runtime/workflows/wf-1/conversations/chat%3A43".to_owned(),
            ),
            (Method::GET, own.replace("wf-1", "wf-2")),
            (
                Method::GET,
                "/api/runtime/workflows/wf-1/conversations".to_owned(),
            ),
            // Anything else.
            (Method::GET, "/api/runtime/workflows/wf-1".to_owned()),
            (Method::GET, "/api/runtime/connections".to_owned()),
            (Method::GET, "/api/runtime/me".to_owned()),
        ] {
            assert!(!grant.admits(&method, &path), "{method} {path}");
        }
    }
}
