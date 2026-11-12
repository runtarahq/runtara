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

/// The platform's own keyset: what it seals of its own, to use with any
/// tenant's records (a platform OAuth app's client secret). Its name is no
/// tenant's keyset: one that starts as a hashed name does is hashed again.
pub fn platform_keyset() -> KeysetId {
    KeysetId::new("t-platform").expect("the platform's keyset name is a valid identifier")
}

/// The tenant the platform's keys are stored for. No tenant: an id of one
/// is never empty.
const PLATFORM_TENANT: &str = "";

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
        if *keyset == platform_keyset() {
            return Some(PLATFORM_TENANT.to_owned());
        }
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
        if *keyset == platform_keyset() {
            // The platform's keys are no tenant's rows, so a tenant's scope
            // hides them. A task of its own leaves the scope.
            let (pool, keyset) = (self.pool.clone(), keyset.clone());
            return tokio::spawn(async move { read_keys(&pool, &keyset).await })
                .await
                .map_err(|_| KeyError::Unavailable)?;
        }
        read_keys(&self.pool, keyset).await
    }

    async fn store(&self, key: &WrappedKey) -> Result<(), KeyError> {
        let tenant = self
            .tenant_of(&key.keyset)
            .ok_or(KeyError::Invalid("a key for a keyset with no known tenant"))?;
        if key.keyset == platform_keyset() {
            let (pool, key) = (self.pool.clone(), key.clone());
            return tokio::spawn(async move { store_key(&pool, &key, &tenant).await })
                .await
                .map_err(|_| KeyError::Unavailable)?;
        }
        store_key(&self.pool, key, &tenant).await
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

async fn read_keys(pool: &PgPool, keyset: &KeysetId) -> Result<Vec<WrappedKey>, KeyError> {
    let rows = sqlx::query_as::<_, (String, String, i64, String, String)>(
        r#"
        SELECT kid, public_key, created, wrap_id, wrapped
        FROM seal_wrapped_key
        WHERE keyset = $1
        ORDER BY created, kid
        "#,
    )
    .bind(keyset.as_str())
    .fetch_all(pool)
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

/// Never replaces a key: a second key with the same id is refused.
async fn store_key(pool: &PgPool, key: &WrappedKey, tenant: &str) -> Result<(), KeyError> {
    let created = i64::try_from(key.created).map_err(|_| KeyError::Invalid("creation time"))?;
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
    .execute(pool)
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

/// Wraps again, under `current`, every seal key in the table that is still
/// wrapped under one of `retired`. Returns how many were.
///
/// This is how the key that wraps tenants' seal keys follows a rotation of
/// the key it is derived from: without it, every sealed secret would be lost
/// with the retired key. Each key is unwrapped and compared with its public
/// key before it is written, and only the row that was read is replaced.
pub async fn rewrap_retired(
    pool: &PgPool,
    current: &dyn KeyWrap,
    retired: &[Arc<dyn KeyWrap>],
) -> Result<usize, String> {
    use base64::Engine;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    let rows: Vec<(String, String, String, String, String)> = sqlx::query_as(
        "SELECT keyset, kid, public_key, wrap_id, wrapped FROM seal_wrapped_key \
         WHERE wrap_id <> $1 ORDER BY keyset, kid",
    )
    .bind(current.id())
    .fetch_all(pool)
    .await
    .map_err(|error| format!("could not read seal keys: {error}"))?;
    let mut rewrapped = 0;
    for (keyset, kid, public, wrap_id, wrapped) in rows {
        let Some(old) = retired.iter().find(|wrap| wrap.id() == wrap_id) else {
            continue;
        };
        let invalid = |what: &str| format!("seal key {keyset}/{kid}: {what}");
        let keyset_id = KeysetId::new(keyset.clone()).map_err(|_| invalid("its keyset"))?;
        let key_id = KeyId::new(kid.clone()).map_err(|_| invalid("its id"))?;
        let bytes = URL_SAFE_NO_PAD
            .decode(&wrapped)
            .map_err(|_| invalid("it is not base64url"))?;
        let private = old
            .unwrap(&keyset_id, &key_id, &bytes)
            .await
            .map_err(|_| invalid("it does not open with the retired key"))?;
        let matches = private
            .public_key()
            .ok()
            .zip(seal_format::PublicKey::from_base64url(&public).ok())
            .is_some_and(|(of_private, stored)| of_private == stored);
        if !matches {
            return Err(invalid("it does not match its public key"));
        }
        let again = current
            .wrap(&keyset_id, &key_id, &private)
            .await
            .map_err(|_| invalid("it could not be wrapped again"))?;
        drop(private);
        let updated = sqlx::query(
            "UPDATE seal_wrapped_key SET wrap_id = $3, wrapped = $4 \
             WHERE keyset = $1 AND kid = $2 AND wrap_id = $5",
        )
        .bind(&keyset)
        .bind(&kid)
        .bind(current.id())
        .bind(URL_SAFE_NO_PAD.encode(again))
        .bind(&wrap_id)
        .execute(pool)
        .await
        .map_err(|error| format!("could not store a seal key: {error}"))?;
        rewrapped += usize::try_from(updated.rows_affected()).unwrap_or(0);
    }
    Ok(rewrapped)
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

    /// No tenant's id names the platform's keyset.
    #[test]
    fn the_platforms_keyset_is_no_tenants() {
        let platform = platform_keyset();
        for tenant in ["t-platform", "platform", "", "t-"] {
            assert_ne!(keyset_for_tenant(tenant), platform, "{tenant}");
        }
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
