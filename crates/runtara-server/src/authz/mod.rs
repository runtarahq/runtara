//! Authorization primitives.
//!
//! Phase 1.3 introduces the [`Role`] type so `AuthContext` can carry the caller's role.
//! The static permission map (`Permission` + `access_for`) lands in Phase 1.4; the role
//! is populated from the per-tenant Valkey `member:{sub}` entry in Phase 1.7. See
//! `docs/security/user-management-contracts.md` for the cross-service contract.

use serde::{Deserialize, Serialize};

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
}
