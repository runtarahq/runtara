//! Per-credential scope: an optional narrowing applied on top of an API key's inherited role.
//!
//! An `rt_*` API key acts as its issuing user — it resolves that user's *current* role from the
//! tenant Valkey on every request, so demoting the user degrades the key. What it could not do
//! before was act as *less* than that user: a key minted by an Owner for a read-only
//! integration could also delete workflows, run arbitrary SQL, and mint further keys.
//!
//! [`ApiKeyScope`] is that missing narrowing. It is stored per key (`api_keys.scope`), carried
//! on [`crate::auth::AuthContext`], and enforced in [`crate::middleware::authorization`] *before*
//! the role gate. Two properties are load-bearing:
//!
//! - **It only ever narrows.** The role gate still runs afterwards, unchanged, so a scope can
//!   never grant what the issuing user's role denies.
//! - **It does not depend on the membership rollout.** Unlike the role gate — dormant unless
//!   [`crate::auth::MembershipPolicy`] is `Required` — the scope is a property of the credential
//!   itself, so it is enforced in every mode. That is what makes it meaningful in the
//!   non-OIDC self-hosted modes, where an API key acts as `Role::Owner` outright.
//!
//! Adding a scope later (`execute`, `data_read`, …) is a variant, an arm in [`ApiKeyScope::permits`],
//! and a choice in the create dialog — the column stores a name, so no migration is involved.

use axum::http::Method;
use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error as _};

use super::Permission;

/// How much of its issuing user's role a key may actually exercise.
///
/// The wire and column form is the snake_case name ([`ApiKeyScope::as_str`]), matching the
/// colon-free half of the [`Permission`] wire convention.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ApiKeyScope {
    /// No narrowing: the key exercises its issuing user's role in full. This is what a `NULL`
    /// column means, so every key issued before scopes existed keeps its behavior.
    #[default]
    Full,
    /// Read-only: the key may perform read operations and nothing else. See
    /// [`ApiKeyScope::permits`] for what "read" means at a route level.
    ReadOnly,
    /// A scope name this build does not recognize — a row written by a newer server, seen after
    /// a rollback. Never selectable and never parsed from the wire; it denies everything, so an
    /// unknown narrowing fails closed rather than silently degrading to `Full`.
    Unrecognized,
}

impl ApiKeyScope {
    /// The scopes a caller may choose when creating a key, in narrowing order. `Unrecognized` is
    /// deliberately absent — it is a read-path artifact, not a choice.
    pub const SELECTABLE: [ApiKeyScope; 2] = [ApiKeyScope::Full, ApiKeyScope::ReadOnly];

    /// The wire/column identifier. `Unrecognized` renders as `unknown` for logs and metrics; it
    /// is never persisted or accepted as input under that name.
    pub const fn as_str(self) -> &'static str {
        match self {
            ApiKeyScope::Full => "full",
            ApiKeyScope::ReadOnly => "read_only",
            ApiKeyScope::Unrecognized => "unknown",
        }
    }

    /// Parse a selectable scope name; `None` for anything else (including `unknown`).
    pub fn from_wire(s: &str) -> Option<ApiKeyScope> {
        Self::SELECTABLE.into_iter().find(|s2| s2.as_str() == s)
    }

    /// Read the scope off an `api_keys.scope` column value.
    ///
    /// `NULL` is [`ApiKeyScope::Full`] — the pre-scope behavior every existing key keeps. A
    /// value this build cannot parse becomes [`ApiKeyScope::Unrecognized`] (denies everything)
    /// rather than falling back to `Full`, so a downgrade cannot widen a key that was created
    /// narrow.
    pub fn from_column(value: Option<&str>) -> ApiKeyScope {
        match value {
            None => ApiKeyScope::Full,
            Some(s) => ApiKeyScope::from_wire(s).unwrap_or(ApiKeyScope::Unrecognized),
        }
    }

    /// The column value to persist: `None` for [`ApiKeyScope::Full`], so "no narrowing" is a
    /// `NULL` and every unscoped key — old or new — is represented identically.
    pub const fn to_column(self) -> Option<&'static str> {
        match self {
            ApiKeyScope::Full => None,
            other => Some(other.as_str()),
        }
    }

    /// Whether this scope permits a request, given the [`Permission`] the route maps to (`None`
    /// for an ungated route) and the request method.
    ///
    /// [`ApiKeyScope::ReadOnly`] answers the two cases differently, and both halves matter:
    ///
    /// - **Mapped route** → the permission must be a read ([`Permission::is_read`]). Keying on
    ///   the permission rather than the method is what keeps POST-shaped reads working — SQL
    ///   `query`, report `preview`/`render`, graph `validate`, CSV export are all `POST` and all
    ///   map to a `*:read` permission.
    /// - **Ungated route** (`permission == None`) → the method must be safe. The ungated set is
    ///   mostly metadata, but it also includes the API-key management routes, which are gated by
    ///   ownership in their handlers rather than by role. Without this arm a read-only key could
    ///   `POST /api/runtime/api-keys` and mint itself an unscoped one.
    pub fn permits(self, permission: Option<Permission>, method: &Method) -> bool {
        match self {
            ApiKeyScope::Full => true,
            ApiKeyScope::ReadOnly => match permission {
                Some(permission) => permission.is_read(),
                None => method.is_safe(),
            },
            ApiKeyScope::Unrecognized => false,
        }
    }

    /// Whether this scope narrows anything at all. `false` for [`ApiKeyScope::Full`], which lets
    /// callers skip scope bookkeeping entirely on the overwhelmingly common path.
    pub const fn is_narrowing(self) -> bool {
        !matches!(self, ApiKeyScope::Full)
    }
}

// Custom serde so the wire form is the snake_case name and unknown values are a hard error
// rather than a silent `Full`. `Unrecognized` can be serialized (it appears in denial bodies)
// but never deserialized.
impl Serialize for ApiKeyScope {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for ApiKeyScope {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        ApiKeyScope::from_wire(&s)
            .ok_or_else(|| D::Error::custom(format!("unknown api key scope: {s}")))
    }
}

// utoipa sees the scope as a string enum of the selectable names, so the generated OpenAPI (and
// the TypeScript client generated from it) offers exactly what the API accepts.
impl utoipa::PartialSchema for ApiKeyScope {
    fn schema() -> utoipa::openapi::RefOr<utoipa::openapi::schema::Schema> {
        utoipa::openapi::ObjectBuilder::new()
            .schema_type(utoipa::openapi::schema::SchemaType::Type(
                utoipa::openapi::schema::Type::String,
            ))
            .enum_values(Some(
                ApiKeyScope::SELECTABLE.into_iter().map(|s| s.as_str()),
            ))
            .description(Some(
                "How much of the issuing user's role the key may exercise. \
                 Omitted or `full` means no narrowing.",
            ))
            .into()
    }
}

impl utoipa::ToSchema for ApiKeyScope {
    fn name() -> std::borrow::Cow<'static, str> {
        std::borrow::Cow::Borrowed("ApiKeyScope")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_form_round_trips_for_selectable_scopes() {
        for scope in ApiKeyScope::SELECTABLE {
            assert_eq!(ApiKeyScope::from_wire(scope.as_str()), Some(scope));
            assert_eq!(
                serde_json::to_value(scope).unwrap(),
                serde_json::json!(scope.as_str())
            );
            assert_eq!(
                serde_json::from_value::<ApiKeyScope>(serde_json::json!(scope.as_str())).unwrap(),
                scope
            );
        }
    }

    #[test]
    fn unknown_wire_value_is_rejected_not_defaulted() {
        // A typo or a scope from a newer build must be a 400 at the API boundary, never a
        // silent `Full`.
        assert!(ApiKeyScope::from_wire("readonly").is_none());
        assert!(serde_json::from_value::<ApiKeyScope>(serde_json::json!("readonly")).is_err());
        assert!(serde_json::from_value::<ApiKeyScope>(serde_json::json!("unknown")).is_err());
    }

    #[test]
    fn null_column_is_full_and_full_persists_as_null() {
        assert_eq!(ApiKeyScope::from_column(None), ApiKeyScope::Full);
        assert_eq!(ApiKeyScope::Full.to_column(), None);
        assert_eq!(ApiKeyScope::default(), ApiKeyScope::Full);
        assert!(!ApiKeyScope::Full.is_narrowing());
    }

    #[test]
    fn read_only_column_round_trips() {
        assert_eq!(
            ApiKeyScope::from_column(Some("read_only")),
            ApiKeyScope::ReadOnly
        );
        assert_eq!(ApiKeyScope::ReadOnly.to_column(), Some("read_only"));
        assert!(ApiKeyScope::ReadOnly.is_narrowing());
    }

    #[test]
    fn unparseable_column_denies_everything() {
        // Rollback safety: a row written by a newer server must not widen to `Full`.
        let scope = ApiKeyScope::from_column(Some("execute"));
        assert_eq!(scope, ApiKeyScope::Unrecognized);
        assert!(scope.is_narrowing());
        for permission in Permission::ALL {
            assert!(!scope.permits(Some(permission), &Method::GET));
        }
        assert!(!scope.permits(None, &Method::GET));
    }

    #[test]
    fn full_permits_everything() {
        for permission in Permission::ALL {
            for method in [Method::GET, Method::POST, Method::PUT, Method::DELETE] {
                assert!(ApiKeyScope::Full.permits(Some(permission), &method));
                assert!(ApiKeyScope::Full.permits(None, &method));
            }
        }
    }

    #[test]
    fn read_only_permits_exactly_the_read_permissions() {
        // Method is irrelevant on a mapped route: a POST-shaped read (SQL query, report render)
        // passes, and a GET-shaped write (the OAuth authorize redirect, gated at
        // `connection:update`) does not.
        for permission in Permission::ALL {
            let want = permission.is_read();
            for method in [Method::GET, Method::POST, Method::DELETE] {
                assert_eq!(
                    ApiKeyScope::ReadOnly.permits(Some(permission), &method),
                    want,
                    "{permission} via {method}"
                );
            }
        }
        assert!(ApiKeyScope::ReadOnly.permits(Some(Permission::DatabaseRead), &Method::POST));
        assert!(!ApiKeyScope::ReadOnly.permits(Some(Permission::ConnectionUpdate), &Method::GET));
    }

    #[test]
    fn read_only_permits_ungated_reads_but_not_ungated_writes() {
        // The escalation case: API-key management is ungated (ownership-gated in the handler),
        // so only the safe-method rule stops a read-only key minting an unscoped one.
        assert!(ApiKeyScope::ReadOnly.permits(None, &Method::GET));
        assert!(ApiKeyScope::ReadOnly.permits(None, &Method::HEAD));
        assert!(!ApiKeyScope::ReadOnly.permits(None, &Method::POST));
        assert!(!ApiKeyScope::ReadOnly.permits(None, &Method::DELETE));
        assert!(!ApiKeyScope::ReadOnly.permits(None, &Method::PUT));
        assert!(!ApiKeyScope::ReadOnly.permits(None, &Method::PATCH));
    }
}
