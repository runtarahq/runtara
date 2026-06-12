//! Authorization primitives: the built-in roles, the permission vocabulary, and the
//! static role → permission map that runtara enforces.
//!
//! This map is the single source of truth for "which role can do what" (see
//! `docs/security/user-management-contracts.md` §4). The caller's [`Role`] is read from the
//! per-tenant Valkey `member:{sub}` entry on every authenticated request;
//! [`access_for`] then decides the [`Access`] for a given [`Permission`]. The `Own` resource
//! check that `Access::Own` implies is enforced by the handler.
//!
//! The map is expressed per role: each [`Role`] owns a constant list of the permissions it
//! grants and the scope of each (see [`Role::grants`]). A permission absent from a role's
//! list is denied.

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error as _};

/// Built-in tenant roles, most- to least-privileged.
///
/// Modeled in Auth0 as Organization Roles. runtara never reads the role from the JWT — it
/// reads it from the tenant's Valkey on every authenticated request. The wire and Valkey
/// form is the lowercase variant name (`owner`, `admin`, `member`, `viewer`), matching the
/// `member:{uid}` value in the contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Owner,
    Admin,
    Member,
    Viewer,
}

impl Role {
    /// All roles, most- to least-privileged.
    pub const ALL: [Role; 4] = [Role::Owner, Role::Admin, Role::Member, Role::Viewer];

    /// The permissions this role grants, each with its [`Access`] scope. A permission not in
    /// the returned list is denied.
    ///
    /// Each role has its own list. Owner and Admin currently grant the same set (this
    /// vocabulary is resource-only), but they are kept separate so the two can diverge by
    /// editing [`ADMIN_ACCESS`] alone.
    pub const fn grants(self) -> &'static [(Permission, Access)] {
        match self {
            Role::Owner => OWNER_ACCESS,
            Role::Admin => ADMIN_ACCESS,
            Role::Member => MEMBER_ACCESS,
            Role::Viewer => VIEWER_ACCESS,
        }
    }

    /// The lowercase wire identifier (`owner`/`admin`/`member`/`viewer`), matching the serde
    /// form and the Valkey `member:{uid}` value.
    pub const fn as_str(self) -> &'static str {
        match self {
            Role::Owner => "owner",
            Role::Admin => "admin",
            Role::Member => "member",
            Role::Viewer => "viewer",
        }
    }

    /// Parse the lowercase wire identifier back into a [`Role`]; `None` for anything else.
    pub fn from_wire(s: &str) -> Option<Role> {
        Self::ALL.into_iter().find(|r| r.as_str() == s)
    }
}

/// The decision the permission map yields for a `(role, permission)` pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Access {
    /// Permitted on any resource in the tenant.
    Allow,
    /// Permitted only on resources the caller created (`created_by == caller.sub`).
    /// Owner/Admin bypass the ownership check; the check itself is enforced in the handler.
    ///
    /// Used by exactly six cells for Member — `workflow`/`trigger`/`report` update/delete — the
    /// resources whose per-row owner (`created_by`) is recorded and a server-crate handler can
    /// check it. Resources without enforceable per-row ownership (database, connection) are flat
    /// `Allow`, never `Own`. The complete set is pinned by
    /// `own_is_restricted_to_the_ownable_permissions`.
    Own,
    /// Never permitted.
    Deny,
}

/// Every permission runtara authorizes. The wire form is the colon-style identifier
/// (`workflow:read`, `invocation_history:read`, …) — see [`Permission::as_str`]. The enum is
/// the canonical source of truth; the string form is the cross-service contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Permission {
    WorkflowRead,
    WorkflowCreate,
    WorkflowUpdate,
    WorkflowDelete,
    WorkflowExecute,
    InvocationHistoryRead,
    DatabaseRead,
    DatabaseCreate,
    DatabaseUpdate,
    DatabaseDelete,
    ReportRead,
    ReportCreate,
    ReportUpdate,
    ReportDelete,
    TriggerRead,
    TriggerCreate,
    TriggerUpdate,
    TriggerDelete,
    ConnectionRead,
    ConnectionCreate,
    ConnectionUpdate,
    ConnectionDelete,
    AnalyticsRead,
    /// UI-only capability: gates the "User Management" link in the runtara SPA that points at
    /// the smo-management control plane. No runtara route enforces it (see `permission_for`);
    /// it exists purely so `/me` advertises whether to show the link. Owner/Admin only.
    UserManagementAccess,
}

impl Permission {
    /// Every permission, in table order.
    pub const ALL: [Permission; 24] = [
        WorkflowRead,
        WorkflowCreate,
        WorkflowUpdate,
        WorkflowDelete,
        WorkflowExecute,
        InvocationHistoryRead,
        DatabaseRead,
        DatabaseCreate,
        DatabaseUpdate,
        DatabaseDelete,
        ReportRead,
        ReportCreate,
        ReportUpdate,
        ReportDelete,
        TriggerRead,
        TriggerCreate,
        TriggerUpdate,
        TriggerDelete,
        ConnectionRead,
        ConnectionCreate,
        ConnectionUpdate,
        ConnectionDelete,
        AnalyticsRead,
        UserManagementAccess,
    ];

    /// The colon-style wire identifier. This is the cross-service contract form — it must
    /// match the ticket's permission table, the contracts doc, and the admin UI.
    pub const fn as_str(self) -> &'static str {
        match self {
            Permission::WorkflowRead => "workflow:read",
            Permission::WorkflowCreate => "workflow:create",
            Permission::WorkflowUpdate => "workflow:update",
            Permission::WorkflowDelete => "workflow:delete",
            Permission::WorkflowExecute => "workflow:execute",
            Permission::InvocationHistoryRead => "invocation_history:read",
            Permission::DatabaseRead => "database:read",
            Permission::DatabaseCreate => "database:create",
            Permission::DatabaseUpdate => "database:update",
            Permission::DatabaseDelete => "database:delete",
            Permission::ReportRead => "report:read",
            Permission::ReportCreate => "report:create",
            Permission::ReportUpdate => "report:update",
            Permission::ReportDelete => "report:delete",
            Permission::TriggerRead => "trigger:read",
            Permission::TriggerCreate => "trigger:create",
            Permission::TriggerUpdate => "trigger:update",
            Permission::TriggerDelete => "trigger:delete",
            Permission::ConnectionRead => "connection:read",
            Permission::ConnectionCreate => "connection:create",
            Permission::ConnectionUpdate => "connection:update",
            Permission::ConnectionDelete => "connection:delete",
            Permission::AnalyticsRead => "analytics:read",
            Permission::UserManagementAccess => "user_management:access",
        }
    }

    /// Parse a colon-style wire identifier back into a `Permission`.
    pub fn from_wire(s: &str) -> Option<Permission> {
        Permission::ALL.into_iter().find(|p| p.as_str() == s)
    }
}

impl fmt::Display for Permission {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

// Custom serde so the wire form is the colon-style identifier, not a default serde rename
// (which can't produce `invocation_history:read`). The enum is the source of truth.
impl Serialize for Permission {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for Permission {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Permission::from_wire(&s)
            .ok_or_else(|| D::Error::custom(format!("unknown permission: {s}")))
    }
}

// ---------------------------------------------------------------------------------------
// The role → permission map. Each role is a constant list of the permissions it grants and
// the scope of each. Kept in sync with `docs/security/user-management-contracts.md` §4 and
// pinned by `permission_map_matches_contract`.
//
// Reads are allowed for everyone; create/execute for Member and above; update/delete are
// Member-`Own` (own resources only) and Allow for Owner/Admin. Viewer is read-only.
// ---------------------------------------------------------------------------------------

use Access::{Allow, Own};
use Permission::{
    AnalyticsRead, ConnectionCreate, ConnectionDelete, ConnectionRead, ConnectionUpdate,
    DatabaseCreate, DatabaseDelete, DatabaseRead, DatabaseUpdate, InvocationHistoryRead,
    ReportCreate, ReportDelete, ReportRead, ReportUpdate, TriggerCreate, TriggerDelete,
    TriggerRead, TriggerUpdate, UserManagementAccess, WorkflowCreate, WorkflowDelete,
    WorkflowExecute, WorkflowRead, WorkflowUpdate,
};

/// Owner: every permission, unconditionally.
const OWNER_ACCESS: &[(Permission, Access)] = &[
    (WorkflowRead, Allow),
    (WorkflowCreate, Allow),
    (WorkflowUpdate, Allow),
    (WorkflowDelete, Allow),
    (WorkflowExecute, Allow),
    (InvocationHistoryRead, Allow),
    (DatabaseRead, Allow),
    (DatabaseCreate, Allow),
    (DatabaseUpdate, Allow),
    (DatabaseDelete, Allow),
    (ReportRead, Allow),
    (ReportCreate, Allow),
    (ReportUpdate, Allow),
    (ReportDelete, Allow),
    (TriggerRead, Allow),
    (TriggerCreate, Allow),
    (TriggerUpdate, Allow),
    (TriggerDelete, Allow),
    (ConnectionRead, Allow),
    (ConnectionCreate, Allow),
    (ConnectionUpdate, Allow),
    (ConnectionDelete, Allow),
    (AnalyticsRead, Allow),
    (UserManagementAccess, Allow),
];

/// Admin: same as Owner for now. Kept as a separate list so the two can diverge by editing
/// here alone, without touching `Role::grants`.
const ADMIN_ACCESS: &[(Permission, Access)] = &[
    (WorkflowRead, Allow),
    (WorkflowCreate, Allow),
    (WorkflowUpdate, Allow),
    (WorkflowDelete, Allow),
    (WorkflowExecute, Allow),
    (InvocationHistoryRead, Allow),
    (DatabaseRead, Allow),
    (DatabaseCreate, Allow),
    (DatabaseUpdate, Allow),
    (DatabaseDelete, Allow),
    (ReportRead, Allow),
    (ReportCreate, Allow),
    (ReportUpdate, Allow),
    (ReportDelete, Allow),
    (TriggerRead, Allow),
    (TriggerCreate, Allow),
    (TriggerUpdate, Allow),
    (TriggerDelete, Allow),
    (ConnectionRead, Allow),
    (ConnectionCreate, Allow),
    (ConnectionUpdate, Allow),
    (ConnectionDelete, Allow),
    (AnalyticsRead, Allow),
    (UserManagementAccess, Allow),
];

/// Member: read + create + execute on any resource. Update/delete are `Own` (own resources
/// only) for workflow, trigger, and report; database and connection have no enforceable
/// per-row owner, so their update/delete are flat `Allow`.
const MEMBER_ACCESS: &[(Permission, Access)] = &[
    (WorkflowRead, Allow),
    (WorkflowCreate, Allow),
    (WorkflowUpdate, Own),
    (WorkflowDelete, Own),
    (WorkflowExecute, Allow),
    (InvocationHistoryRead, Allow),
    (DatabaseRead, Allow),
    (DatabaseCreate, Allow),
    // Object-model rows carry no per-row owner — we do not track `created_by` for database
    // objects — so database permissions are never `Own`. Member gets full update/delete on
    // any object. Guarded by `no_role_has_own_for_database`.
    (DatabaseUpdate, Allow),
    (DatabaseDelete, Allow),
    (ReportRead, Allow),
    (ReportCreate, Allow),
    (ReportUpdate, Own),
    (ReportDelete, Own),
    (TriggerRead, Allow),
    (TriggerCreate, Allow),
    (TriggerUpdate, Own),
    (TriggerDelete, Own),
    (ConnectionRead, Allow),
    (ConnectionCreate, Allow),
    // Connections live in another crate that does not bridge the caller's identity, so per-row
    // ownership is not enforceable. Like database, connection update/delete is flat `Allow` for
    // Member for now; revisit (flip to `Own`) once the ownership bridge lands.
    (ConnectionUpdate, Allow),
    (ConnectionDelete, Allow),
    (AnalyticsRead, Allow),
];

/// Viewer: read-only across the tenant.
const VIEWER_ACCESS: &[(Permission, Access)] = &[
    (WorkflowRead, Allow),
    (InvocationHistoryRead, Allow),
    (DatabaseRead, Allow),
    (ReportRead, Allow),
    (TriggerRead, Allow),
    (ConnectionRead, Allow),
    (AnalyticsRead, Allow),
];

/// The [`Access`] `role` has for `permission`, per the static map. A permission a role does
/// not grant is [`Access::Deny`].
pub fn access_for(role: Role, permission: Permission) -> Access {
    role.grants()
        .iter()
        .find(|(p, _)| *p == permission)
        .map(|(_, access)| *access)
        .unwrap_or(Access::Deny)
}

/// The full role → permission map serialized for distribution. This is the JSON the
/// `GET /api/runtime/permissions` endpoint returns — the cross-service contract smo-management
/// and the admin UI consume so they render exactly what runtara enforces (see
/// `docs/security/user-management-contracts.md`). Shape:
///
/// ```json
/// {
///   "version": 1,
///   "roles": ["owner", "admin", "member", "viewer"],
///   "permissions": {
///     "workflow:update": { "owner": "allow", "admin": "allow", "member": "own", "viewer": "deny" },
///     ...
///   }
/// }
/// ```
pub fn permission_map_json() -> serde_json::Value {
    let mut permissions = serde_json::Map::new();
    for permission in Permission::ALL {
        let mut row = serde_json::Map::new();
        for role in Role::ALL {
            row.insert(
                role.as_str().to_string(),
                serde_json::to_value(access_for(role, permission))
                    .expect("Access serializes to a string"),
            );
        }
        permissions.insert(
            permission.as_str().to_string(),
            serde_json::Value::Object(row),
        );
    }
    serde_json::json!({
        "version": 1,
        "roles": Role::ALL.iter().map(|r| r.as_str()).collect::<Vec<_>>(),
        "permissions": permissions,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn role_wire_form_is_lowercase() {
        // The lowercase form is the cross-service contract shared with the Valkey
        // `member:{uid}` value and the permission-map JSON. Guard it against accidental
        // serde-rename drift.
        for (role, wire) in [
            (Role::Owner, "owner"),
            (Role::Admin, "admin"),
            (Role::Member, "member"),
            (Role::Viewer, "viewer"),
        ] {
            assert_eq!(serde_json::to_value(role).unwrap(), serde_json::json!(wire));
            assert_eq!(
                serde_json::from_value::<Role>(serde_json::json!(wire)).unwrap(),
                role
            );
        }
    }

    #[test]
    fn role_rejects_unknown_value() {
        assert!(serde_json::from_value::<Role>(serde_json::json!("superuser")).is_err());
    }

    #[test]
    fn access_wire_form_is_lowercase() {
        assert_eq!(serde_json::to_value(Access::Allow).unwrap(), json("allow"));
        assert_eq!(serde_json::to_value(Access::Own).unwrap(), json("own"));
        assert_eq!(serde_json::to_value(Access::Deny).unwrap(), json("deny"));
    }

    #[test]
    fn permission_wire_form_roundtrips() {
        for permission in Permission::ALL {
            let wire = serde_json::to_value(permission).unwrap();
            // serializes to the colon-style identifier...
            assert_eq!(wire, json(permission.as_str()));
            // ...and parses back to the same variant.
            assert_eq!(
                serde_json::from_value::<Permission>(wire).unwrap(),
                permission
            );
        }
        // The underscore-in-resource case the default serde rename can't produce.
        assert_eq!(
            Permission::InvocationHistoryRead.as_str(),
            "invocation_history:read"
        );
    }

    #[test]
    fn permission_rejects_unknown_wire() {
        assert!(Permission::from_wire("workflow:teleport").is_none());
        assert!(serde_json::from_value::<Permission>(json("nope:nope")).is_err());
    }

    /// The exact-match guard the contracts doc references: an independent transcription of
    /// the permission table (`docs/security/user-management-contracts.md` §4), asserted cell
    /// by cell. Any drift between the per-role lists and the contract fails here.
    #[test]
    fn permission_map_matches_contract() {
        use Access::{Allow, Deny, Own};

        // [Owner, Admin, Member, Viewer] — transcribed from the contract, NOT derived from
        // the role grant lists, so this is a real cross-check rather than a tautology.
        let expected: &[(Permission, [Access; 4])] = &[
            (Permission::WorkflowRead, [Allow, Allow, Allow, Allow]),
            (Permission::WorkflowCreate, [Allow, Allow, Allow, Deny]),
            (Permission::WorkflowUpdate, [Allow, Allow, Own, Deny]),
            (Permission::WorkflowDelete, [Allow, Allow, Own, Deny]),
            (Permission::WorkflowExecute, [Allow, Allow, Allow, Deny]),
            (
                Permission::InvocationHistoryRead,
                [Allow, Allow, Allow, Allow],
            ),
            (Permission::DatabaseRead, [Allow, Allow, Allow, Allow]),
            (Permission::DatabaseCreate, [Allow, Allow, Allow, Deny]),
            (Permission::DatabaseUpdate, [Allow, Allow, Allow, Deny]),
            (Permission::DatabaseDelete, [Allow, Allow, Allow, Deny]),
            (Permission::ReportRead, [Allow, Allow, Allow, Allow]),
            (Permission::ReportCreate, [Allow, Allow, Allow, Deny]),
            (Permission::ReportUpdate, [Allow, Allow, Own, Deny]),
            (Permission::ReportDelete, [Allow, Allow, Own, Deny]),
            (Permission::TriggerRead, [Allow, Allow, Allow, Allow]),
            (Permission::TriggerCreate, [Allow, Allow, Allow, Deny]),
            (Permission::TriggerUpdate, [Allow, Allow, Own, Deny]),
            (Permission::TriggerDelete, [Allow, Allow, Own, Deny]),
            (Permission::ConnectionRead, [Allow, Allow, Allow, Allow]),
            (Permission::ConnectionCreate, [Allow, Allow, Allow, Deny]),
            (Permission::ConnectionUpdate, [Allow, Allow, Allow, Deny]),
            (Permission::ConnectionDelete, [Allow, Allow, Allow, Deny]),
            (Permission::AnalyticsRead, [Allow, Allow, Allow, Allow]),
            (Permission::UserManagementAccess, [Allow, Allow, Deny, Deny]),
        ];

        // Every permission is covered exactly once — adding a `Permission` variant without a
        // contract row fails here.
        assert_eq!(
            expected.len(),
            Permission::ALL.len(),
            "expected table is missing or has extra permission rows"
        );

        for (permission, row) in expected {
            for (role, &want) in Role::ALL.iter().zip(row.iter()) {
                let got = access_for(*role, *permission);
                assert_eq!(
                    got, want,
                    "access_for({role:?}, {permission}) = {got:?}, contract says {want:?}"
                );
            }
        }
    }

    /// `Own` is allowed for exactly six cells — workflow/trigger/report update/delete — the
    /// resources whose per-row owner (`created_by`) is recorded and a server-crate path can
    /// check it. Any other `Own` (a new resource, or connection/database regaining it before the
    /// storage layer can enforce it) fails here. Pins the whole `Own` set, both directions.
    #[test]
    fn own_is_restricted_to_the_ownable_permissions() {
        use std::collections::BTreeSet;

        let allowed: BTreeSet<&str> = [
            "workflow:update",
            "workflow:delete",
            "trigger:update",
            "trigger:delete",
            "report:update",
            "report:delete",
        ]
        .into_iter()
        .collect();

        let mut actual: BTreeSet<&str> = BTreeSet::new();
        for role in Role::ALL {
            for permission in Permission::ALL {
                if access_for(role, permission) == Access::Own {
                    actual.insert(permission.as_str());
                }
            }
        }

        assert_eq!(
            actual, allowed,
            "the set of Own-capable permissions drifted from the ownable cells"
        );
    }

    /// Database (object-model) rows have no per-row owner, so no role may have `Own` access to
    /// any `Database*` permission — they are flat Allow/Deny across the four static roles.
    /// Roles are not user-creatable, so this map is the whole story; this test fails loudly if
    /// a future edit reintroduces object-model ownership the storage layer cannot enforce.
    #[test]
    fn no_role_has_own_for_database() {
        let database_permissions = [
            Permission::DatabaseRead,
            Permission::DatabaseCreate,
            Permission::DatabaseUpdate,
            Permission::DatabaseDelete,
        ];
        for role in Role::ALL {
            for permission in database_permissions {
                assert_ne!(
                    access_for(role, permission),
                    Access::Own,
                    "{role:?} must not have Own access to {permission}"
                );
            }
        }
    }

    #[test]
    fn permission_map_json_covers_every_cell() {
        let map = permission_map_json();
        assert_eq!(map["version"], 1);
        assert_eq!(
            map["roles"],
            serde_json::json!(["owner", "admin", "member", "viewer"])
        );

        let perms = map["permissions"].as_object().expect("permissions object");
        assert_eq!(perms.len(), Permission::ALL.len());

        // Spot-check a representative trio against the map, and confirm the wire forms match.
        assert_eq!(perms["workflow:update"]["member"], "own");
        assert_eq!(perms["database:delete"]["member"], "allow");
        assert_eq!(perms["workflow:delete"]["viewer"], "deny");

        // Every cell is present and is one of the three access strings.
        for permission in Permission::ALL {
            let row = &perms[permission.as_str()];
            for role in Role::ALL {
                let cell = row[role.as_str()].as_str().expect("access string");
                assert!(matches!(cell, "allow" | "own" | "deny"), "{cell}");
            }
        }
    }

    fn json(s: &str) -> serde_json::Value {
        serde_json::Value::String(s.to_string())
    }
}
