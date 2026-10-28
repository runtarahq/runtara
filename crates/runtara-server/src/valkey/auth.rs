//! Valkey reads for the auth contract.
//!
//! runtara resolves the caller's role and token-revocation status from Valkey on every
//! authenticated request. This module owns those reads; the keys and value shapes are the
//! cross-service contract documented in `docs/security/user-management-contracts.md` §3:
//!
//! ```text
//! {tenant}:member:{uid}        -> { "role": "owner|admin|member|viewer", "updated_at": "..." }
//! {tenant}:token:revoked:{jti} -> { "revoked_at": "..." }   # presence = revoked; TTL-bound
//! ```
//!
//! One Valkey may serve many tenants, so every key names its tenant. Contract version 1
//! keys (`member:{uid}`, `token:revoked:{jti}`, from when each tenant had its own Valkey)
//! are still read: `member:{uid}` answers for the primary tenant only, unless
//! `RUNTARA_AUTH_LEGACY_MEMBER_KEYS=false`, and a version 1 revocation still revokes.
//!
//! These are simple non-blocking `GET` / `EXISTS` commands, so they ride the process-wide
//! shared [`ConnectionManager`](super::SHARED_MANAGER) (clone-per-call, no new connection).

use redis::AsyncCommands;
use redis::aio::ConnectionManager;

use crate::authz::Role;

/// Failure reading the auth contract from Valkey. The auth middleware treats any error as
/// fail-closed; the distinction here is for logging/metrics, not policy.
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

/// The member record of `user_id` in `tenant`.
fn member_key(tenant: &str, user_id: &str) -> String {
    format!("{tenant}:member:{user_id}")
}

/// The contract version 1 member record, read for the primary tenant only:
/// it carries no tenant, so it cannot answer for any other. Off where one
/// Valkey serves several processes (`RUNTARA_AUTH_LEGACY_MEMBER_KEYS=false`).
fn legacy_member_key(tenant: &str, user_id: &str) -> Option<String> {
    (crate::config::legacy_member_keys()
        && crate::config::try_tenant_id().is_none_or(|primary| primary == tenant))
    .then(|| format!("member:{user_id}"))
}

/// The revocation denylist entry for `jti` in `tenant`.
fn revoked_token_key(tenant: &str, jti: &str) -> String {
    format!("{tenant}:token:revoked:{jti}")
}

/// The contract version 1 denylist entry. A `jti` is unique, so it revokes in
/// any tenant.
fn legacy_revoked_token_key(jti: &str) -> String {
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

/// Read the caller's role in `tenant` (`{tenant}:member:{user_id}`, else the
/// version 1 `member:{user_id}` for the primary tenant).
///
/// Returns `Ok(None)` when the key is absent — the user is not a member of this tenant.
/// Returns `Err` if Valkey is unreachable or the stored record is malformed; the caller
/// fails the request closed in both cases.
pub async fn get_member_role(
    manager: &ConnectionManager,
    tenant: &str,
    user_id: &str,
) -> Result<Option<Role>, AuthzValkeyError> {
    let mut conn = manager.clone();
    let mut raw: Option<String> = conn.get(member_key(tenant, user_id)).await?;
    if raw.is_none()
        && let Some(legacy) = legacy_member_key(tenant, user_id)
    {
        raw = conn.get(legacy).await?;
    }
    match raw {
        None => Ok(None),
        Some(raw) => parse_member_record(&raw).map(Some),
    }
}

/// Whether the token identified by `jti` is on `tenant`'s revocation denylist
/// (`{tenant}:token:revoked:{jti}`, or the version 1 `token:revoked:{jti}`).
///
/// Presence of either key means revoked — the body is advisory, so this is one `EXISTS`.
/// A tenant-scoped process (`RUNTARA_VALKEY_TENANT_SCOPED`) looks only at the
/// tenant's key: the version 1 key is outside what it may read.
pub async fn token_is_revoked(
    manager: &ConnectionManager,
    tenant: &str,
    jti: &str,
) -> Result<bool, AuthzValkeyError> {
    let mut conn = manager.clone();
    let mut keys = vec![revoked_token_key(tenant, jti)];
    if !crate::config::valkey_tenant_scoped() {
        keys.push(legacy_revoked_token_key(jti));
    }
    let present: u32 = conn.exists(&keys).await?;
    Ok(present > 0)
}

/// Write a revocation entry for `jti` to `tenant`'s denylist.
///
/// `ttl_seconds` bounds the entry to the token's remaining lifetime so it self-cleans once
/// the token would have expired anyway. `None` writes a non-expiring entry (the caller has
/// no expiry to bound it by); `Some(0)` is a no-op — the token has already expired.
pub async fn revoke_token(
    manager: &ConnectionManager,
    tenant: &str,
    jti: &str,
    ttl_seconds: Option<u64>,
) -> Result<(), AuthzValkeyError> {
    let mut conn = manager.clone();
    let key = revoked_token_key(tenant, jti);
    let value = serde_json::json!({ "revoked_at": chrono::Utc::now().to_rfc3339() }).to_string();
    match ttl_seconds {
        Some(0) => Ok(()),
        Some(ttl) => {
            let _: () = conn.set_ex(key, value, ttl).await?;
            Ok(())
        }
        None => {
            let _: () = conn.set(key, value).await?;
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_follow_contract() {
        crate::config::init_for_test();
        let primary = crate::config::tenant_id();
        assert_eq!(
            member_key("tenant-b", "auth0|abc"),
            "tenant-b:member:auth0|abc"
        );
        assert_eq!(
            legacy_member_key(primary, "auth0|abc").as_deref(),
            Some("member:auth0|abc")
        );
        assert_eq!(legacy_member_key("tenant-b", "auth0|abc"), None);
        assert_eq!(
            revoked_token_key("tenant-b", "jti-1"),
            "tenant-b:token:revoked:jti-1"
        );
        assert_eq!(legacy_revoked_token_key("jti-1"), "token:revoked:jti-1");
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
