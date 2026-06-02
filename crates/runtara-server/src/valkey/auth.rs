//! Per-tenant Valkey reads for the SYN-437 auth contract.
//!
//! runtara resolves the caller's role and token-revocation status from the tenant's Valkey
//! on every authenticated request. This module owns those two reads; the keys and value
//! shapes are the cross-service contract documented in
//! `docs/security/user-management-contracts.md` §3:
//!
//! ```text
//! member:{uid}        -> { "role": "owner|admin|member|viewer", "updated_at": "..." }
//! token:revoked:{jti} -> { "revoked_at": "..." }   # presence = revoked; TTL-bound
//! ```
//!
//! Both are simple non-blocking `GET` / `EXISTS` commands, so they ride the process-wide
//! shared [`ConnectionManager`](super::SHARED_MANAGER) (clone-per-call, no new connection).
//! The Valkey instance is the tenant boundary — keys carry no tenant prefix.

use redis::AsyncCommands;
use redis::aio::ConnectionManager;

use crate::authz::Role;

/// Failure reading the auth contract from Valkey. The auth middleware treats any error as
/// fail-closed (Phase 1.7); the distinction here is for logging/metrics, not policy.
#[derive(Debug, thiserror::Error)]
pub enum AuthzValkeyError {
    /// The command itself failed — Valkey unreachable, timeout, wrong type, etc.
    #[error("valkey command failed: {0}")]
    Redis(#[from] redis::RedisError),
    /// The key existed but its value did not match the contract (bad JSON, unknown role,
    /// missing `role` field). Treated as fail-closed: a member record we cannot trust is
    /// not a valid membership.
    #[error("malformed member record: {0}")]
    Parse(String),
}

/// Valkey key for a member's role record.
fn member_key(user_id: &str) -> String {
    format!("member:{user_id}")
}

/// Valkey key for a token-revocation denylist entry.
fn revoked_token_key(jti: &str) -> String {
    format!("token:revoked:{jti}")
}

/// The `member:{uid}` value. Only `role` is load-bearing; `updated_at` (and any future
/// fields) are advisory and intentionally ignored here.
#[derive(Debug, serde::Deserialize)]
struct MemberRecord {
    role: Role,
}

/// Parse a raw `member:{uid}` value into the caller's [`Role`].
///
/// Kept separate from the I/O so the contract parsing — the part that actually matters for
/// correctness — is unit-testable without a live Valkey. An unknown role or malformed JSON
/// is an error, never a silent default: we fail closed rather than guess a role.
fn parse_member_record(raw: &str) -> Result<Role, AuthzValkeyError> {
    serde_json::from_str::<MemberRecord>(raw)
        .map(|record| record.role)
        .map_err(|e| AuthzValkeyError::Parse(e.to_string()))
}

/// Read the caller's role for this tenant from `member:{user_id}`.
///
/// Returns `Ok(None)` when the key is absent — the user is not a member of this tenant.
/// Returns `Err` if Valkey is unreachable or the stored record is malformed; the caller
/// fails the request closed in both cases.
pub async fn get_member_role(
    manager: &ConnectionManager,
    user_id: &str,
) -> Result<Option<Role>, AuthzValkeyError> {
    let mut conn = manager.clone();
    let raw: Option<String> = conn.get(member_key(user_id)).await?;
    match raw {
        None => Ok(None),
        Some(raw) => parse_member_record(&raw).map(Some),
    }
}

/// Whether the token identified by `jti` is on the revocation denylist
/// (`token:revoked:{jti}`).
///
/// Presence of the key means revoked — the body is advisory, so this is a plain `EXISTS`.
pub async fn token_is_revoked(
    manager: &ConnectionManager,
    jti: &str,
) -> Result<bool, AuthzValkeyError> {
    let mut conn = manager.clone();
    let revoked: bool = conn.exists(revoked_token_key(jti)).await?;
    Ok(revoked)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_follow_contract() {
        assert_eq!(member_key("auth0|abc"), "member:auth0|abc");
        assert_eq!(revoked_token_key("jti-1"), "token:revoked:jti-1");
    }

    #[test]
    fn parses_each_role() {
        for (wire, role) in [
            ("owner", Role::Owner),
            ("admin", Role::Admin),
            ("member", Role::Member),
            ("viewer", Role::Viewer),
        ] {
            let raw = format!(r#"{{"role":"{wire}","updated_at":"2026-05-28T12:00:00Z"}}"#);
            assert_eq!(parse_member_record(&raw).unwrap(), role);
        }
    }

    #[test]
    fn ignores_unknown_fields() {
        let raw = r#"{"role":"viewer","updated_at":"2026-05-28T12:00:00Z","extra":true}"#;
        assert_eq!(parse_member_record(raw).unwrap(), Role::Viewer);
    }

    #[test]
    fn unknown_role_fails_closed() {
        let err = parse_member_record(r#"{"role":"superuser"}"#).unwrap_err();
        assert!(matches!(err, AuthzValkeyError::Parse(_)));
    }

    #[test]
    fn malformed_value_fails_closed() {
        assert!(matches!(
            parse_member_record("not json").unwrap_err(),
            AuthzValkeyError::Parse(_)
        ));
        // Missing the load-bearing `role` field.
        assert!(matches!(
            parse_member_record(r#"{"updated_at":"2026-05-28T12:00:00Z"}"#).unwrap_err(),
            AuthzValkeyError::Parse(_)
        ));
    }
}
