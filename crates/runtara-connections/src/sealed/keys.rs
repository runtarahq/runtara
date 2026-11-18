//! Where seal's keys live for this deployment.
//!
//! A tenant has one keyset. Its private keys are stored in Postgres, wrapped by
//! a key the database never holds, so a copy of the database opens nothing.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use async_trait::async_trait;
use hmac::{Hmac, Mac};
use seal_format::{KeyId, KeysetId};
use seal_keys::{
    DirKeySource, FileKeyWrap, KeyError, KeySource, KeyWrap, KeyWraps, RewrapOptions, WrappedKey,
};
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
            let mut rows = platform_rows(&self.pool).await?;
            let keys = read_keys(&mut *rows, keyset).await?;
            rows.commit().await.map_err(unavailable)?;
            return Ok(keys);
        }
        read_keys(&self.pool, keyset).await
    }

    async fn store(&self, key: &WrappedKey) -> Result<(), KeyError> {
        let tenant = self
            .tenant_of(&key.keyset)
            .ok_or(KeyError::Invalid("a key for a keyset with no known tenant"))?;
        if key.keyset == platform_keyset() {
            let mut rows = platform_rows(&self.pool).await?;
            store_key(&mut *rows, key, &tenant).await?;
            return rows.commit().await.map_err(unavailable);
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

    async fn keysets(&self) -> Result<Vec<KeysetId>, KeyError> {
        let names: Vec<String> =
            sqlx::query_scalar("SELECT DISTINCT keyset FROM seal_wrapped_key ORDER BY keyset")
                .fetch_all(&self.pool)
                .await
                .map_err(|error| {
                    tracing::error!(%error, "could not list seal keysets");
                    KeyError::Unavailable
                })?;
        names
            .into_iter()
            .map(|name| KeysetId::new(name).map_err(|_| KeyError::Invalid("stored keyset")))
            .collect()
    }

    /// Replaces a key with itself wrapped another way, and only if it is still
    /// the one that was read.
    async fn replace(&self, old: &WrappedKey, new: &WrappedKey) -> Result<(), KeyError> {
        let replaced = sqlx::query(
            "UPDATE seal_wrapped_key SET wrap_id = $3, wrapped = $4 \
             WHERE keyset = $1 AND kid = $2 AND wrap_id = $5 AND wrapped = $6",
        )
        .bind(old.keyset.as_str())
        .bind(old.kid.as_str())
        .bind(&new.wrap_id)
        .bind(&new.wrapped)
        .bind(&old.wrap_id)
        .bind(&old.wrapped)
        .execute(&self.pool)
        .await
        .map_err(|error| {
            tracing::error!(%error, "could not store a seal key wrapped again");
            KeyError::Unavailable
        })?;
        if replaced.rows_affected() == 0 {
            // Another instance wrapped it again first, or it is gone.
            return Err(KeyError::Unavailable);
        }
        Ok(())
    }
}

fn unavailable(error: sqlx::Error) -> KeyError {
    tracing::error!(%error, "could not reach the platform's seal keys");
    KeyError::Unavailable
}

/// A transaction that reaches the platform's own rows and no tenant's. The
/// platform's keys are no tenant's rows, so a tenant's scope hides them, and
/// a request for a tenant's connection on a platform OAuth app needs them
/// from inside one. The transaction names the platform as its tenant, for as
/// long as it lasts.
async fn platform_rows(
    pool: &PgPool,
) -> Result<sqlx::Transaction<'static, sqlx::Postgres>, KeyError> {
    let mut rows = pool.begin().await.map_err(unavailable)?;
    sqlx::query(
        "SELECT set_config('runtara.tenant_id', $1, true), \
                set_config('runtara.all_tenants', 'off', true)",
    )
    .bind(PLATFORM_TENANT)
    .execute(&mut *rows)
    .await
    .map_err(unavailable)?;
    Ok(rows)
}

async fn read_keys(
    pool: impl sqlx::PgExecutor<'_>,
    keyset: &KeysetId,
) -> Result<Vec<WrappedKey>, KeyError> {
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
async fn store_key(
    pool: impl sqlx::PgExecutor<'_>,
    key: &WrappedKey,
    tenant: &str,
) -> Result<(), KeyError> {
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

/// A key source that shows only the keys a set of wrapping keys can open.
/// seal's re-wrapping refuses to start while any key is under a wrapping key
/// it was not given; a key left under one that is long gone opens nothing
/// either way, and is not a reason to stop the rest.
struct Openable<'a> {
    keys: &'a PgKeySource,
    wraps: &'a KeyWraps,
    left: std::sync::atomic::AtomicUsize,
}

#[async_trait]
impl KeySource for Openable<'_> {
    async fn wrapped_keys(&self, keyset: &KeysetId) -> Result<Vec<WrappedKey>, KeyError> {
        let all = self.keys.wrapped_keys(keyset).await?;
        let count = all.len();
        let openable: Vec<WrappedKey> = all
            .into_iter()
            .filter(|key| self.wraps.by_id(&key.wrap_id).is_some())
            .collect();
        self.left
            .fetch_add(count - openable.len(), std::sync::atomic::Ordering::Relaxed);
        Ok(openable)
    }

    async fn keysets(&self) -> Result<Vec<KeysetId>, KeyError> {
        self.keys.keysets().await
    }

    async fn replace(&self, old: &WrappedKey, new: &WrappedKey) -> Result<(), KeyError> {
        self.keys.replace(old, new).await
    }
}

/// Wraps again, under `current`, every seal key in the table that is still
/// wrapped under one of `retired`. Returns how many were.
///
/// This is how the key that wraps tenants' seal keys follows a rotation of
/// the key it is derived from: without it, every sealed secret would be lost
/// with the retired key. The re-wrapping is seal's own: each key is unwrapped
/// and compared with its public key, wrapped, and opened again from its new
/// form before that replaces the old, and only the row that was read is
/// replaced.
pub async fn rewrap_retired(
    pool: &PgPool,
    current: Arc<dyn KeyWrap>,
    retired: &[Arc<dyn KeyWrap>],
) -> Result<usize, String> {
    let mut older: Vec<Arc<dyn KeyWrap>> = Vec::new();
    for wrap in retired {
        let repeated = wrap.id() == current.id() || older.iter().any(|w| w.id() == wrap.id());
        if !repeated {
            older.push(Arc::clone(wrap));
        }
    }
    let wraps = KeyWraps::new(current, older).map_err(|error| error.to_string())?;
    let keys = PgKeySource::new(pool.clone());
    let openable = Openable {
        keys: &keys,
        wraps: &wraps,
        left: std::sync::atomic::AtomicUsize::new(0),
    };
    let done = seal_keys::rewrap(&openable, &wraps, RewrapOptions::default())
        .await
        .map_err(|error| format!("seal keys could not be wrapped again: {error}"))?;
    let left = openable.left.load(std::sync::atomic::Ordering::Relaxed);
    if left > 0 {
        tracing::warn!(
            left,
            "seal keys are wrapped under a key that is neither the current nor a retired one, \
             and were left: nothing sealed to them opens"
        );
    }
    Ok(done.rewrapped)
}

/// Writes every seal key in the table into `dir`, as the key directory of a
/// seal that runs as a process of its own holds them
/// (`<dir>/<keyset>/<kid>.json`), still wrapped. Returns how many were
/// written and how many were already there.
///
/// The directory is seal's own (`DirKeySource`), so the layout is seal's and
/// not a copy of it. A key is never replaced: one already in the directory
/// under the same name is left, so the command can be run again.
pub async fn export_keys(pool: &PgPool, dir: &std::path::Path) -> Result<(usize, usize), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        if !dir.exists() {
            std::fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(dir)
                .map_err(|error| format!("{} could not be made: {error}", dir.display()))?;
        }
    }
    #[cfg(not(unix))]
    std::fs::create_dir_all(dir)
        .map_err(|error| format!("{} could not be made: {error}", dir.display()))?;
    let source = PgKeySource::new(pool.clone());
    let target = DirKeySource::new(dir, usize::MAX);
    let (mut written, mut present) = (0, 0);
    for keyset in source
        .keysets()
        .await
        .map_err(|error| format!("the keysets could not be listed: {error}"))?
    {
        for key in source
            .wrapped_keys(&keyset)
            .await
            .map_err(|error| format!("the keys of a keyset could not be read: {error}"))?
        {
            match target.store(&key).await {
                Ok(()) => written += 1,
                Err(KeyError::AlreadyExists) => present += 1,
                Err(error) => {
                    return Err(format!(
                        "key {}/{} could not be written: {error}",
                        key.keyset.as_str(),
                        key.kid.as_str()
                    ));
                }
            }
        }
    }
    Ok((written, present))
}

/// Writes the key that wraps tenants' seal keys to `path`, as seal reads a
/// wrapping key file: its 32 bytes, in a new file only its owner can read.
/// For moving the key to the host of a seal that runs as a process of its
/// own; nothing else should hold a copy afterwards.
pub fn export_wrapping_key(path: &std::path::Path, key: &[u8; 32]) -> Result<(), String> {
    use std::io::Write;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .map_err(|error| format!("{} could not be made: {error}", path.display()))?;
    file.write_all(key)
        .and_then(|()| file.sync_all())
        .map_err(|error| format!("{} could not be written: {error}", path.display()))
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
