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
//! The scope is one of three shapes:
//!
//! - [`ApiKeyScope::Full`] — no narrowing (a `NULL` column); every pre-scope key keeps this.
//! - [`ApiKeyScope::ReadOnly`] — a named preset: every `*:read` permission and nothing else.
//! - [`ApiKeyScope::Restricted`] — an explicit [`PermissionSet`]: the key may exercise exactly
//!   the listed permissions (still intersected with its issuing user's live role). This is what
//!   granular, least-privilege keys use (SYN-939).
//! - [`ApiKeyScope::Unrecognized`] — a row this build cannot parse (written by a newer server,
//!   seen after a rollback). It denies everything, so an unknown narrowing fails closed.

use axum::http::Method;
use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::Error as _,
    ser::{Error as _, SerializeSeq},
};

use super::Permission;

/// A set of [`Permission`]s, stored as a bitset over [`Permission::ALL`] (21 variants fit in a
/// `u32`). Kept `Copy` so [`ApiKeyScope`] — and everything that embeds it ([`crate::middleware`]
/// `ScopeDenial`, [`crate::auth::AuthContext`]) — stays `Copy`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct PermissionSet(u32);

impl PermissionSet {
    /// The empty set — grants nothing.
    pub const EMPTY: PermissionSet = PermissionSet(0);

    /// The bit index of a permission: its position in [`Permission::ALL`]. Total order is fixed
    /// by the enum, so the bit layout is stable across builds (only ever appended to).
    fn bit(permission: Permission) -> u32 {
        Permission::ALL
            .iter()
            .position(|p| *p == permission)
            .expect("every Permission is in Permission::ALL") as u32
    }

    /// This set with `permission` added.
    pub fn with(mut self, permission: Permission) -> PermissionSet {
        self.0 |= 1 << Self::bit(permission);
        self
    }

    /// Build a set from an iterator of permissions.
    pub fn from_permissions(iter: impl IntoIterator<Item = Permission>) -> PermissionSet {
        let mut bits = 0u32;
        for p in iter {
            bits |= 1 << Self::bit(p);
        }
        PermissionSet(bits)
    }

    /// Whether `permission` is in the set.
    pub fn contains(self, permission: Permission) -> bool {
        self.0 & (1 << Self::bit(permission)) != 0
    }

    /// The intersection of two sets — the clamp used to bound a requested scope to the issuing
    /// user's own permissions at create time.
    pub const fn intersect(self, other: PermissionSet) -> PermissionSet {
        PermissionSet(self.0 & other.0)
    }

    /// Whether the set grants nothing.
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// The granted permissions, in [`Permission::ALL`] order.
    pub fn iter(self) -> impl Iterator<Item = Permission> {
        Permission::ALL
            .into_iter()
            .filter(move |p| self.contains(*p))
    }

    /// The permissions this set grants that are *not* in `other` — i.e. what a requested scope
    /// asked for beyond the clamp. Used to build an actionable 403 at key creation.
    pub fn difference(self, other: PermissionSet) -> impl Iterator<Item = Permission> {
        PermissionSet(self.0 & !other.0).iter()
    }
}

/// How much of its issuing user's role a key may actually exercise.
///
/// The wire and column form is the snake_case name ([`ApiKeyScope::as_str`]) for the presets, or
/// a JSON array of permission wire-names for [`ApiKeyScope::Restricted`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ApiKeyScope {
    /// No narrowing: the key exercises its issuing user's role in full. This is what a `NULL`
    /// column means, so every key issued before scopes existed keeps its behavior.
    #[default]
    Full,
    /// Read-only preset: the key may perform read operations and nothing else. See
    /// [`ApiKeyScope::permits`] for what "read" means at a route level.
    ReadOnly,
    /// An explicit permission set (SYN-939): the key may exercise exactly these permissions,
    /// still intersected with its issuing user's live role by the role gate that runs after.
    Restricted(PermissionSet),
    /// A scope this build does not recognize — a row written by a newer server, seen after a
    /// rollback. Never selectable and never parsed from a column into anything permissive; it
    /// denies everything, so an unknown narrowing fails closed rather than degrading to `Full`.
    Unrecognized,
}

impl ApiKeyScope {
    /// The preset scope *names* a caller may choose as a keyword (presets). `Restricted` is
    /// chosen by sending an explicit permission array instead, and `Unrecognized` is a read-path
    /// artifact, never a choice.
    pub const SELECTABLE: [ApiKeyScope; 2] = [ApiKeyScope::Full, ApiKeyScope::ReadOnly];

    /// A short, stable label for logs, metrics and denial bodies. `Restricted` renders as
    /// `restricted` (the specific permission is already carried alongside in a denial);
    /// `Unrecognized` as `unknown`.
    pub const fn as_str(self) -> &'static str {
        match self {
            ApiKeyScope::Full => "full",
            ApiKeyScope::ReadOnly => "read_only",
            ApiKeyScope::Restricted(_) => "restricted",
            ApiKeyScope::Unrecognized => "unknown",
        }
    }

    /// Parse a preset keyword; `None` for anything else (including `unknown` and `restricted`,
    /// which are never accepted as a bare keyword).
    pub fn keyword_from_wire(s: &str) -> Option<ApiKeyScope> {
        match s {
            "full" => Some(ApiKeyScope::Full),
            "read_only" => Some(ApiKeyScope::ReadOnly),
            _ => None,
        }
    }

    /// Read the scope off an `api_keys.scope` column value.
    ///
    /// `NULL` is [`ApiKeyScope::Full`] — the pre-scope behavior every existing key keeps. A
    /// `read_only` keyword is the read preset. A JSON array of permission wire-names
    /// (`["workflow:read", …]`) is a [`ApiKeyScope::Restricted`] set. Anything this build cannot
    /// parse — an unknown keyword, or an array naming a permission this build doesn't know —
    /// becomes [`ApiKeyScope::Unrecognized`] (denies everything) rather than falling back to
    /// `Full`, so a downgrade cannot widen a key that was created narrow.
    pub fn from_column(value: Option<&str>) -> ApiKeyScope {
        let Some(s) = value else {
            return ApiKeyScope::Full;
        };
        let s = s.trim();
        if s.starts_with('[') {
            // JSON array of permission wire-names.
            match serde_json::from_str::<Vec<String>>(s) {
                Ok(names) => match names
                    .iter()
                    .map(|n| Permission::from_wire(n))
                    .collect::<Option<Vec<_>>>()
                {
                    Some(perms) => ApiKeyScope::Restricted(PermissionSet::from_permissions(perms)),
                    None => ApiKeyScope::Unrecognized,
                },
                Err(_) => ApiKeyScope::Unrecognized,
            }
        } else {
            ApiKeyScope::keyword_from_wire(s).unwrap_or(ApiKeyScope::Unrecognized)
        }
    }

    /// The column value to persist: `None` for [`ApiKeyScope::Full`], so "no narrowing" is a
    /// `NULL` and every unscoped key — old or new — is represented identically. `ReadOnly`
    /// persists as the keyword; `Restricted` as a JSON array of permission wire-names.
    pub fn to_column(self) -> Option<String> {
        match self {
            ApiKeyScope::Full => None,
            ApiKeyScope::ReadOnly => Some("read_only".to_string()),
            ApiKeyScope::Restricted(set) => {
                let names: Vec<&str> = set.iter().map(|p| p.as_str()).collect();
                Some(serde_json::to_string(&names).expect("Vec<&str> serializes"))
            }
            // Never actually persisted (not selectable), but round-trips to itself if it were.
            ApiKeyScope::Unrecognized => Some("unknown".to_string()),
        }
    }

    /// Whether this scope permits a request, given the [`Permission`] the route maps to (`None`
    /// for an ungated route) and the request method.
    ///
    /// - **Mapped route** → the permission must be granted. For [`ApiKeyScope::ReadOnly`] that
    ///   means [`Permission::is_read`]; for [`ApiKeyScope::Restricted`] it means membership in
    ///   the set. Keying on the permission rather than the method is what keeps POST-shaped reads
    ///   working — SQL `query`, report `preview`/`render`, graph `validate`, CSV export are all
    ///   `POST` and all map to a `*:read` permission.
    /// - **Ungated route** (`permission == None`) → the method must be safe. The ungated set is
    ///   mostly metadata, but it also includes the API-key management routes, which are gated by
    ///   ownership in their handlers rather than by role. Without this arm a narrowed key could
    ///   `POST /api/runtime/api-keys` and mint itself an unscoped one.
    pub fn permits(self, permission: Option<Permission>, method: &Method) -> bool {
        match self {
            ApiKeyScope::Full => true,
            ApiKeyScope::ReadOnly => match permission {
                Some(permission) => permission.is_read(),
                None => method.is_safe(),
            },
            ApiKeyScope::Restricted(set) => match permission {
                Some(permission) => set.contains(permission),
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

// Custom serde so the wire form is a preset keyword string *or* a JSON array of permission
// wire-names (for `Restricted`). Unknown keywords and unknown permission names are a hard error
// on deserialize (the API boundary), never a silent `Full`. `Unrecognized` serializes to
// `"unknown"` and deserializes back from it, so an `AuthContext` carrying it round-trips.
impl Serialize for ApiKeyScope {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            ApiKeyScope::Restricted(set) => {
                if set.is_empty() {
                    return Err(S::Error::custom("a restricted scope must grant at least one permission"));
                }
                let perms: Vec<Permission> = set.iter().collect();
                let mut seq = serializer.serialize_seq(Some(perms.len()))?;
                for p in perms {
                    seq.serialize_element(p.as_str())?;
                }
                seq.end()
            }
            other => serializer.serialize_str(other.as_str()),
        }
    }
}

impl<'de> Deserialize<'de> for ApiKeyScope {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Wire {
            Keyword(String),
            List(Vec<String>),
        }
        match Wire::deserialize(deserializer)? {
            Wire::Keyword(s) => match s.as_str() {
                "unknown" => Ok(ApiKeyScope::Unrecognized),
                other => ApiKeyScope::keyword_from_wire(other)
                    .ok_or_else(|| D::Error::custom(format!("unknown api key scope: {other}"))),
            },
            Wire::List(names) => {
                let perms = names
                    .iter()
                    .map(|n| {
                        Permission::from_wire(n)
                            .ok_or_else(|| D::Error::custom(format!("unknown permission: {n}")))
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                if perms.is_empty() {
                    return Err(D::Error::custom(
                        "a restricted scope must grant at least one permission",
                    ));
                }
                Ok(ApiKeyScope::Restricted(PermissionSet::from_permissions(perms)))
            }
        }
    }
}

// utoipa advertises the two accepted shapes as a `oneOf`: a preset keyword (`full` / `read_only`)
// or an array of permission wire-names (an explicit `Restricted` set). The generated OpenAPI —
// and the TypeScript client generated from it — then types the field as `"full" | "read_only" |
// Permission[]`, matching what deserialize accepts.
impl utoipa::PartialSchema for ApiKeyScope {
    fn schema() -> utoipa::openapi::RefOr<utoipa::openapi::schema::Schema> {
        use utoipa::openapi::schema::{ArrayBuilder, ObjectBuilder, OneOfBuilder, SchemaType, Type};
        use utoipa::openapi::{RefOr, Schema};

        let keyword: RefOr<Schema> = ObjectBuilder::new()
            .schema_type(SchemaType::Type(Type::String))
            .enum_values(Some(ApiKeyScope::SELECTABLE.into_iter().map(|s| s.as_str())))
            .into();
        let permission: RefOr<Schema> = ObjectBuilder::new()
            .schema_type(SchemaType::Type(Type::String))
            .enum_values(Some(Permission::ALL.into_iter().map(|p| p.as_str())))
            .into();
        let permission_array: RefOr<Schema> = ArrayBuilder::new().items(permission).into();

        OneOfBuilder::new()
            .item(keyword)
            .item(permission_array)
            .description(Some(
                "How much of the issuing user's role the key may exercise. Omitted or `full` \
                 means no narrowing; `read_only` is every read permission; an array of \
                 permission wire-names (e.g. [\"workflow:read\",\"workflow:execute\"]) is an \
                 explicit least-privilege set.",
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

    fn reads() -> PermissionSet {
        PermissionSet::from_permissions(Permission::ALL.into_iter().filter(|p| p.is_read()))
    }

    #[test]
    fn keyword_wire_form_round_trips() {
        for scope in ApiKeyScope::SELECTABLE {
            assert_eq!(ApiKeyScope::keyword_from_wire(scope.as_str()), Some(scope));
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
    fn unknown_keyword_is_rejected_not_defaulted() {
        assert!(ApiKeyScope::keyword_from_wire("readonly").is_none());
        assert!(serde_json::from_value::<ApiKeyScope>(serde_json::json!("readonly")).is_err());
        assert!(serde_json::from_value::<ApiKeyScope>(serde_json::json!("restricted")).is_err());
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
        assert_eq!(ApiKeyScope::ReadOnly.to_column(), Some("read_only".to_string()));
        assert!(ApiKeyScope::ReadOnly.is_narrowing());
    }

    #[test]
    fn restricted_column_round_trips() {
        let scope = ApiKeyScope::Restricted(PermissionSet::from_permissions([
            Permission::WorkflowRead,
            Permission::WorkflowExecute,
        ]));
        let col = scope.to_column().expect("restricted persists");
        assert_eq!(ApiKeyScope::from_column(Some(&col)), scope);
        assert!(scope.is_narrowing());
    }

    #[test]
    fn restricted_wire_round_trips_as_array() {
        let scope = ApiKeyScope::Restricted(PermissionSet::from_permissions([
            Permission::ConnectionRead,
            Permission::WorkflowRead,
        ]));
        let v = serde_json::to_value(scope).unwrap();
        // Serialized in Permission::ALL order (workflow:read before connection:read).
        assert_eq!(v, serde_json::json!(["workflow:read", "connection:read"]));
        assert_eq!(serde_json::from_value::<ApiKeyScope>(v).unwrap(), scope);
    }

    #[test]
    fn restricted_rejects_unknown_permission_on_the_wire() {
        assert!(
            serde_json::from_value::<ApiKeyScope>(serde_json::json!(["workflow:teleport"])).is_err()
        );
        // An empty set is not a meaningful scope.
        assert!(serde_json::from_value::<ApiKeyScope>(serde_json::json!([])).is_err());
    }

    #[test]
    fn unparseable_column_denies_everything() {
        // Rollback safety: a row written by a newer server must not widen to `Full`.
        for col in ["execute", "[\"workflow:teleport\"]", "{not json"] {
            let scope = ApiKeyScope::from_column(Some(col));
            assert_eq!(scope, ApiKeyScope::Unrecognized, "col = {col}");
            assert!(scope.is_narrowing());
            for permission in Permission::ALL {
                assert!(!scope.permits(Some(permission), &Method::GET));
            }
            assert!(!scope.permits(None, &Method::GET));
        }
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
    fn restricted_permits_exactly_its_set() {
        let scope = ApiKeyScope::Restricted(PermissionSet::from_permissions([
            Permission::WorkflowRead,
            Permission::WorkflowExecute,
        ]));
        assert!(scope.permits(Some(Permission::WorkflowRead), &Method::GET));
        assert!(scope.permits(Some(Permission::WorkflowExecute), &Method::POST));
        // A write not in the set is denied even on a safe method...
        assert!(!scope.permits(Some(Permission::WorkflowDelete), &Method::GET));
        // ...and a read not in the set is denied too (unlike ReadOnly, membership is explicit).
        assert!(!scope.permits(Some(Permission::ConnectionRead), &Method::GET));
    }

    #[test]
    fn read_only_permits_ungated_reads_but_not_ungated_writes() {
        // The escalation case: API-key management is ungated (ownership-gated in the handler),
        // so only the safe-method rule stops a narrowed key minting an unscoped one. Holds for
        // both ReadOnly and an explicit Restricted set.
        for scope in [
            ApiKeyScope::ReadOnly,
            ApiKeyScope::Restricted(PermissionSet::from_permissions([Permission::WorkflowRead])),
        ] {
            assert!(scope.permits(None, &Method::GET));
            assert!(scope.permits(None, &Method::HEAD));
            assert!(!scope.permits(None, &Method::POST));
            assert!(!scope.permits(None, &Method::DELETE));
            assert!(!scope.permits(None, &Method::PUT));
            assert!(!scope.permits(None, &Method::PATCH));
        }
    }

    #[test]
    fn permission_set_basics() {
        let set = PermissionSet::from_permissions([Permission::WorkflowRead, Permission::WorkflowExecute]);
        assert!(set.contains(Permission::WorkflowRead));
        assert!(!set.contains(Permission::WorkflowDelete));
        assert!(!set.is_empty());
        assert!(PermissionSet::EMPTY.is_empty());
        let got: Vec<_> = set.iter().collect();
        assert_eq!(got, vec![Permission::WorkflowRead, Permission::WorkflowExecute]);
    }

    #[test]
    fn permission_set_intersect_and_difference() {
        let requested = PermissionSet::from_permissions([
            Permission::WorkflowRead,
            Permission::WorkflowFolderRename,
        ]);
        let allowed = reads(); // owner-less example: just the reads
        let clamped = requested.intersect(allowed);
        assert!(clamped.contains(Permission::WorkflowRead));
        assert!(!clamped.contains(Permission::WorkflowFolderRename));
        let dropped: Vec<_> = requested.difference(allowed).collect();
        assert_eq!(dropped, vec![Permission::WorkflowFolderRename]);
    }
}
