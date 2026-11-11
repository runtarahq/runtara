//! Where seal's keys live for this deployment.
//!
//! A tenant has one keyset. Its private keys are stored in Postgres, wrapped by
//! a key the database never holds, so a copy of the database opens nothing.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use async_trait::async_trait;
use hmac::{Hmac, Mac};
use seal_format::{KeyId, KeysetId};
use seal_keys::{FileKeyWrap, KeyError, KeySource, KeyWrap, WrappedKey};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use zeroize::Zeroizing;

/// The keyset of a tenant.
///
/// A tenant id that is a valid seal identifier is its own keyset. Any other is
/// named by its hash, as is one that looks like such a name, so that two
/// tenants never share a keyset.
pub fn keyset_for_tenant(tenant_id: &str) -> KeysetId {
    const HASHED: &str = "t-";
    if !tenant_id.starts_with(HASHED)
        && let Ok(keyset) = KeysetId::new(tenant_id)
    {
        return keyset;
    }
    let digest = hex::encode(Sha256::digest(tenant_id.as_bytes()));
    KeysetId::new(format!("{HASHED}{digest}")).expect("a hashed keyset name is a valid identifier")
}

/// The key that wraps tenants' private keys, derived from the deployment's
/// credential encryption key. The two keys differ, so a value made with one
/// means nothing to the other.
pub fn wrapping_key_from_encryption_key(encryption_key: &[u8]) -> Zeroizing<[u8; 32]> {
    let mut mac =
        Hmac::<Sha256>::new_from_slice(encryption_key).expect("HMAC accepts a key of any length");
    mac.update(b"runtara/seal/wrapping-key/v1");
    let mut key = Zeroizing::new([0_u8; 32]);
    key.copy_from_slice(&mac.finalize().into_bytes());
    key
}

/// What wraps tenants' private keys, from 32 bytes of key.
pub fn key_wrap(wrapping_key: &[u8]) -> Result<Arc<dyn KeyWrap>, KeyError> {
    Ok(Arc::new(FileKeyWrap::from_bytes(wrapping_key)?))
}

/// Wrapped keys in the `seal_wrapped_key` table.
pub struct PgKeySource {
    pool: PgPool,
    /// The tenant of each keyset this process may create a key for. seal names
    /// a key by its keyset only; the row also names the tenant, for row-level
    /// security.
    tenants: Mutex<HashMap<String, String>>,
}

impl PgKeySource {
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool,
            tenants: Mutex::new(HashMap::new()),
        }
    }

    /// Says which tenant a keyset is for, before a key is made for it.
    pub fn expect_tenant(&self, tenant_id: &str) -> KeysetId {
        let keyset = keyset_for_tenant(tenant_id);
        self.tenants
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(keyset.as_str().to_owned(), tenant_id.to_owned());
        keyset
    }

    fn tenant_of(&self, keyset: &KeysetId) -> Option<String> {
        self.tenants
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(keyset.as_str())
            .cloned()
    }
}

#[async_trait]
impl KeySource for PgKeySource {
    async fn wrapped_keys(&self, keyset: &KeysetId) -> Result<Vec<WrappedKey>, KeyError> {
        let rows = sqlx::query_as::<_, (String, String, i64, String, String)>(
            r#"
            SELECT kid, public_key, created, wrap_id, wrapped
            FROM seal_wrapped_key
            WHERE keyset = $1
            ORDER BY created, kid
            "#,
        )
        .bind(keyset.as_str())
        .fetch_all(&self.pool)
        .await
        .map_err(|error| {
            tracing::error!(%error, "could not read seal keys");
            KeyError::Unavailable
        })?;
        rows.into_iter()
            .map(|(kid, public, created, wrap_id, wrapped)| {
                Ok(WrappedKey {
                    keyset: keyset.clone(),
                    kid: KeyId::new(kid).map_err(|_| KeyError::Invalid("stored kid"))?,
                    public,
                    created: u64::try_from(created)
                        .map_err(|_| KeyError::Invalid("stored creation time"))?,
                    wrap_id,
                    wrapped,
                })
            })
            .collect()
    }

    async fn store(&self, key: &WrappedKey) -> Result<(), KeyError> {
        let tenant = self
            .tenant_of(&key.keyset)
            .ok_or(KeyError::Invalid("a key for a keyset with no known tenant"))?;
        let created = i64::try_from(key.created).map_err(|_| KeyError::Invalid("creation time"))?;
        // Never replaces a key: a second key with the same id is refused.
        let stored = sqlx::query(
            r#"
            INSERT INTO seal_wrapped_key
                (keyset, kid, tenant_id, public_key, created, wrap_id, wrapped)
            VALUES ($1, $2, $3, $4, $5, $6, $7)
            ON CONFLICT (keyset, kid) DO NOTHING
            "#,
        )
        .bind(key.keyset.as_str())
        .bind(key.kid.as_str())
        .bind(tenant)
        .bind(&key.public)
        .bind(created)
        .bind(&key.wrap_id)
        .bind(&key.wrapped)
        .execute(&self.pool)
        .await
        .map_err(|error| {
            tracing::error!(%error, "could not store a seal key");
            KeyError::Unavailable
        })?;
        if stored.rows_affected() == 0 {
            return Err(KeyError::AlreadyExists);
        }
        Ok(())
    }

    async fn delete_keyset(&self, keyset: &KeysetId) -> Result<(), KeyError> {
        sqlx::query("DELETE FROM seal_wrapped_key WHERE keyset = $1")
            .bind(keyset.as_str())
            .execute(&self.pool)
            .await
            .map_err(|error| {
                tracing::error!(%error, "could not delete seal keys");
                KeyError::Unavailable
            })?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_valid_tenant_id_is_its_own_keyset() {
        for tenant in ["acme", "org_2abc", "a.b-c:d", "0123"] {
            assert_eq!(keyset_for_tenant(tenant).as_str(), tenant);
        }
    }

    #[test]
    fn any_other_tenant_id_is_named_by_its_hash() {
        let long = "x".repeat(200);
        for tenant in ["auth0|abc", "a b", "", ".hidden", "ünï", long.as_str()] {
            let keyset = keyset_for_tenant(tenant);
            assert!(keyset.as_str().starts_with("t-"), "{tenant}");
            assert_eq!(keyset.as_str().len(), 66, "{tenant}");
        }
    }

    /// A tenant cannot name itself after another tenant's hashed keyset.
    #[test]
    fn a_tenant_id_that_looks_hashed_is_hashed_again() {
        let victim = keyset_for_tenant("auth0|victim");
        let impostor = keyset_for_tenant(victim.as_str());
        assert_ne!(victim, impostor);
        assert!(impostor.as_str().starts_with("t-"));
    }

    #[test]
    fn the_wrapping_key_is_derived_and_differs_from_the_encryption_key() {
        let encryption_key = [7_u8; 32];
        let first = wrapping_key_from_encryption_key(&encryption_key);
        assert_eq!(*first, *wrapping_key_from_encryption_key(&encryption_key));
        assert_ne!(*first, encryption_key);
        assert_ne!(*first, *wrapping_key_from_encryption_key(&[8_u8; 32]));
        key_wrap(first.as_ref()).unwrap();
    }

    #[test]
    fn a_wrapping_key_of_the_wrong_length_is_refused() {
        assert!(key_wrap(&[0_u8; 16]).is_err());
    }
}
