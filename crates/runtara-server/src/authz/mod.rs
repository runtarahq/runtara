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
}

/// The decision the permission map yields for a `(role, permission)` pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Access {
    /// Permitted on any resource in the tenant.
    Allow,
    /// Permitted only on resources the caller created (`created_by == caller.sub`).
    /// Owner/Admin bypass the ownership check; the check itself is enforced in the handler.
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
}

impl Permission {
    /// Every permission, in table order.
    pub const ALL: [Permission; 23] = [
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
    TriggerRead, TriggerUpdate, WorkflowCreate, WorkflowDelete, WorkflowExecute, WorkflowRead,
    WorkflowUpdate,
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
];

/// Member: read + create + execute on any resource; update/delete only on own resources.
const MEMBER_ACCESS: &[(Permission, Access)] = &[
    (WorkflowRead, Allow),
    (WorkflowCreate, Allow),
    (WorkflowUpdate, Own),
    (WorkflowDelete, Own),
    (WorkflowExecute, Allow),
    (InvocationHistoryRead, Allow),
    (DatabaseRead, Allow),
    (DatabaseCreate, Allow),
    (DatabaseUpdate, Own),
    (DatabaseDelete, Own),
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
    (ConnectionUpdate, Own),
    (ConnectionDelete, Own),
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
            (Permission::DatabaseUpdate, [Allow, Allow, Own, Deny]),
            (Permission::DatabaseDelete, [Allow, Allow, Own, Deny]),
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
            (Permission::ConnectionUpdate, [Allow, Allow, Own, Deny]),
            (Permission::ConnectionDelete, [Allow, Allow, Own, Deny]),
            (Permission::AnalyticsRead, [Allow, Allow, Allow, Allow]),
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

    fn json(s: &str) -> serde_json::Value {
        serde_json::Value::String(s.to_string())
    }
}
