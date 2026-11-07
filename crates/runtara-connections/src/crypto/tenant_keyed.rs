//! Each tenant's credentials under that tenant's own keys.

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::Value;

use super::{CipherContext, CipherError, CredentialCipher};

/// Seals and opens each tenant's credentials with that tenant's own cipher,
/// and a tenant without one with the default, when there is one.
///
/// Every per-tenant cipher is its own keyring (a current key plus retired
/// ones), so rotating one tenant's key and re-encrypting its rows
/// (`reencrypt-connections --tenant-id`) leaves every other tenant's
/// ciphertext as it was. A host that keeps keys in a KMS builds the
/// per-tenant ciphers from the keys it unwraps there and supplies the result
/// through the server's options.
///
/// A host that gives every tenant it serves keys of its own needs no default
/// ([`TenantKeyedCipher::without_default`]): a tenant without keys is then
/// refused, rather than sealed under a key that is not its own.
pub struct TenantKeyedCipher {
    default: Option<Arc<dyn CredentialCipher>>,
    tenants: HashMap<String, Arc<dyn CredentialCipher>>,
}

/// The key id a cipher without a default reports: it seals nothing under
/// a key of its own.
const NO_DEFAULT_KEY_ID: &str = "none";

impl TenantKeyedCipher {
    /// Every tenant under `default` until given its own cipher.
    pub fn new(default: Arc<dyn CredentialCipher>) -> Self {
        Self {
            default: Some(default),
            tenants: HashMap::new(),
        }
    }

    /// Only the tenants given a cipher of their own: any other tenant's
    /// credentials are neither sealed nor opened.
    pub fn without_default() -> Self {
        Self {
            default: None,
            tenants: HashMap::new(),
        }
    }

    /// Seal and open `tenant`'s credentials with `cipher`.
    pub fn with_tenant(
        mut self,
        tenant: impl Into<String>,
        cipher: Arc<dyn CredentialCipher>,
    ) -> Self {
        self.tenants.insert(tenant.into(), cipher);
        self
    }

    fn cipher_for(&self, tenant: &str) -> Option<&dyn CredentialCipher> {
        self.tenants
            .get(tenant)
            .or(self.default.as_ref())
            .map(|cipher| cipher.as_ref())
    }

    fn no_keys(tenant: &str) -> String {
        format!("tenant {tenant} has no credential keys")
    }
}

impl CredentialCipher for TenantKeyedCipher {
    fn encrypt(&self, plaintext: &Value, context: &CipherContext) -> Result<Value, CipherError> {
        self.cipher_for(context.tenant_id)
            .ok_or_else(|| CipherError::Encrypt(Self::no_keys(context.tenant_id)))?
            .encrypt(plaintext, context)
    }

    fn decrypt(&self, stored: &Value, context: &CipherContext) -> Result<Value, CipherError> {
        self.cipher_for(context.tenant_id)
            .ok_or_else(|| CipherError::Decrypt(Self::no_keys(context.tenant_id)))?
            .decrypt(stored, context)
    }

    /// The default cipher's key; each tenant's envelopes name their own.
    fn key_id(&self) -> &str {
        self.default
            .as_ref()
            .map_or(NO_DEFAULT_KEY_ID, |default| default.key_id())
    }

    /// Whether every credential it seals is encrypted: the default's, if
    /// there is one, and every tenant's own. Without a default, a tenant with
    /// no keys has nothing sealed at all.
    fn is_encrypting(&self) -> bool {
        self.default
            .as_ref()
            .is_none_or(|default| default.is_encrypting())
            && self.tenants.values().all(|c| c.is_encrypting())
    }

    /// A process serving several tenants may have no default key: a tenant
    /// with its own is encrypted all the same. A tenant with no cipher at all
    /// is not: nothing of its can be sealed or re-encrypted.
    fn is_encrypting_for(&self, tenant: &str) -> bool {
        self.cipher_for(tenant)
            .is_some_and(|cipher| cipher.is_encrypting())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::aes_gcm::AesGcmCipher;
    use serde_json::json;
    use zeroize::Zeroizing;

    fn key(byte: u8) -> Zeroizing<Vec<u8>> {
        Zeroizing::new(vec![byte; 32])
    }

    fn keyring(current: (u8, &str), retired: &[(u8, &str)]) -> Arc<dyn CredentialCipher> {
        let mut cipher = AesGcmCipher::new(key(current.0), current.1).unwrap();
        for (byte, kid) in retired {
            cipher = cipher.with_retired_key(key(*byte), *kid).unwrap();
        }
        Arc::new(cipher)
    }

    fn ctx<'a>(tenant: &'a str, connection: &'a str) -> CipherContext<'a> {
        CipherContext {
            tenant_id: tenant,
            connection_id: connection,
        }
    }

    #[test]
    fn a_tenant_with_its_own_keys_is_sealed_under_them() {
        let cipher = TenantKeyedCipher::new(keyring((1, "default-1"), &[]))
            .with_tenant("acme", keyring((2, "acme-1"), &[]));
        let secret = json!({"token": "s"});

        let acme = cipher.encrypt(&secret, &ctx("acme", "c1")).unwrap();
        let other = cipher.encrypt(&secret, &ctx("beta", "c2")).unwrap();
        assert_eq!(acme["kid"], "acme-1");
        assert_eq!(other["kid"], "default-1");
        assert_eq!(cipher.decrypt(&acme, &ctx("acme", "c1")).unwrap(), secret);
        assert_eq!(cipher.decrypt(&other, &ctx("beta", "c2")).unwrap(), secret);
        // Another tenant's keys cannot open it, whatever the connection.
        assert!(cipher.decrypt(&acme, &ctx("beta", "c1")).is_err());
    }

    /// A process serving several tenants with no default key encrypts each
    /// tenant it has keys for, so one tenant's rows may be re-encrypted.
    #[test]
    fn a_tenant_with_its_own_keys_is_encrypted_without_a_default_key() {
        let cipher = TenantKeyedCipher::new(Arc::new(crate::crypto::noop::NoOpCipher))
            .with_tenant("acme", keyring((2, "acme-1"), &[]));
        assert!(!cipher.is_encrypting(), "not every tenant is encrypted");
        assert!(cipher.is_encrypting_for("acme"));
        assert!(
            !cipher.is_encrypting_for("beta"),
            "beta has no keys of its own"
        );
    }

    /// Without a default, the tenants with keys are sealed under them, any
    /// other is refused, and the whole counts as encrypting, so a server
    /// boots with it.
    #[test]
    fn without_a_default_a_tenant_without_keys_is_refused() {
        let cipher =
            TenantKeyedCipher::without_default().with_tenant("acme", keyring((2, "acme-1"), &[]));
        assert!(cipher.is_encrypting());
        assert!(cipher.is_encrypting_for("acme"));
        assert!(!cipher.is_encrypting_for("beta"));

        let secret = json!({"token": "s"});
        let acme = cipher.encrypt(&secret, &ctx("acme", "c1")).unwrap();
        assert_eq!(acme["kid"], "acme-1");
        assert_eq!(cipher.decrypt(&acme, &ctx("acme", "c1")).unwrap(), secret);

        let refused = cipher.encrypt(&secret, &ctx("beta", "c2")).unwrap_err();
        assert!(refused.to_string().contains("beta"));
        assert!(cipher.decrypt(&acme, &ctx("beta", "c1")).is_err());
        // Nothing comes back in the clear either.
        assert!(cipher.decrypt(&secret, &ctx("beta", "c2")).is_err());

        // One tenant's plaintext cipher still makes the whole not encrypting.
        let leaky = TenantKeyedCipher::without_default()
            .with_tenant("acme", Arc::new(crate::crypto::noop::NoOpCipher));
        assert!(!leaky.is_encrypting());
    }

    /// Before any tenant is given keys, it seals nothing and still boots.
    #[test]
    fn without_a_default_or_tenants_nothing_is_sealed() {
        let cipher = TenantKeyedCipher::without_default();
        assert!(cipher.is_encrypting());
        assert_eq!(cipher.key_id(), "none");
        assert!(
            cipher
                .encrypt(&json!({"token": "s"}), &ctx("acme", "c1"))
                .is_err()
        );
    }

    #[test]
    fn rotating_one_tenants_key_leaves_the_others_untouched() {
        let beta = keyring((3, "beta-1"), &[]);
        let before = TenantKeyedCipher::new(keyring((1, "default-1"), &[]))
            .with_tenant("acme", keyring((2, "acme-1"), &[]))
            .with_tenant("beta", beta.clone());
        let secret = json!({"token": "s"});
        let acme_old = before.encrypt(&secret, &ctx("acme", "c1")).unwrap();
        let beta_row = before.encrypt(&secret, &ctx("beta", "c2")).unwrap();

        // acme rotates: a new current key, the old one retired while its
        // rows are re-encrypted.
        let rotating = TenantKeyedCipher::new(keyring((1, "default-1"), &[]))
            .with_tenant("acme", keyring((4, "acme-2"), &[(2, "acme-1")]))
            .with_tenant("beta", beta.clone());
        let opened = rotating.decrypt(&acme_old, &ctx("acme", "c1")).unwrap();
        let acme_new = rotating.encrypt(&opened, &ctx("acme", "c1")).unwrap();
        assert_eq!(acme_new["kid"], "acme-2");

        // Then the old key is dropped: acme's re-encrypted row opens, its old
        // ciphertext no longer does, and beta's row, never rewritten, still
        // opens with beta's unchanged keys.
        let rotated = TenantKeyedCipher::new(keyring((1, "default-1"), &[]))
            .with_tenant("acme", keyring((4, "acme-2"), &[]))
            .with_tenant("beta", beta);
        assert_eq!(
            rotated.decrypt(&acme_new, &ctx("acme", "c1")).unwrap(),
            secret
        );
        assert!(rotated.decrypt(&acme_old, &ctx("acme", "c1")).is_err());
        assert_eq!(
            rotated.decrypt(&beta_row, &ctx("beta", "c2")).unwrap(),
            secret
        );
        assert_eq!(beta_row["kid"], "beta-1");
    }
}
