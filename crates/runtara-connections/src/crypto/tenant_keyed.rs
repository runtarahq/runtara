//! Each tenant's credentials under that tenant's own keys.

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::Value;

use super::{CipherContext, CipherError, CredentialCipher};

/// Seals and opens each tenant's credentials with that tenant's own cipher,
/// and a tenant without one with `default`.
///
/// Every per-tenant cipher is its own keyring (a current key plus retired
/// ones), so rotating one tenant's key and re-encrypting its rows
/// (`reencrypt-connections --tenant-id`) leaves every other tenant's
/// ciphertext as it was. A host that keeps keys in a KMS builds the
/// per-tenant ciphers from the keys it unwraps there and supplies the result
/// through the server's options.
pub struct TenantKeyedCipher {
    default: Arc<dyn CredentialCipher>,
    tenants: HashMap<String, Arc<dyn CredentialCipher>>,
}

impl TenantKeyedCipher {
    /// Every tenant under `default` until given its own cipher.
    pub fn new(default: Arc<dyn CredentialCipher>) -> Self {
        Self {
            default,
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

    fn cipher_for(&self, tenant: &str) -> &dyn CredentialCipher {
        self.tenants
            .get(tenant)
            .map_or(self.default.as_ref(), |cipher| cipher.as_ref())
    }
}

impl CredentialCipher for TenantKeyedCipher {
    fn encrypt(&self, plaintext: &Value, context: &CipherContext) -> Result<Value, CipherError> {
        self.cipher_for(context.tenant_id)
            .encrypt(plaintext, context)
    }

    fn decrypt(&self, stored: &Value, context: &CipherContext) -> Result<Value, CipherError> {
        self.cipher_for(context.tenant_id).decrypt(stored, context)
    }

    /// The default cipher's key; each tenant's envelopes name their own.
    fn key_id(&self) -> &str {
        self.default.key_id()
    }

    fn is_encrypting(&self) -> bool {
        self.default.is_encrypting() && self.tenants.values().all(|c| c.is_encrypting())
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
