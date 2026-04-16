//! Construct a [`CredentialCipher`] from environment configuration.
//!
//! This is the single place the crate reads environment variables for
//! encryption setup. Host applications may also construct ciphers manually
//! and inject them into [`crate::ConnectionsConfig`] directly — the env-var
//! path is a convenience for typical deployments.

use std::sync::Arc;

use zeroize::Zeroizing;

use super::aes_gcm::AesGcmCipher;
use super::cipher::CredentialCipher;
use super::noop::NoOpCipher;

/// Name of the environment variable that supplies the AES-256 data key.
///
/// The value must be a base64-encoded 32-byte key. Generate one with:
///
/// ```bash
/// openssl rand -base64 32
/// ```
pub const ENCRYPTION_KEY_ENV: &str = "RUNTARA_CONNECTIONS_ENCRYPTION_KEY";

/// Optional key identifier used to tag envelopes. Useful for rotation.
/// Defaults to `"env"`.
pub const ENCRYPTION_KEY_ID_ENV: &str = "RUNTARA_CONNECTIONS_ENCRYPTION_KEY_ID";

/// Build a cipher by reading [`ENCRYPTION_KEY_ENV`].
///
/// Returns:
/// - [`AesGcmCipher`] wrapped in `Arc` if the env var is set and valid
/// - [`NoOpCipher`] with a prominent warning log otherwise (plaintext at rest)
pub fn cipher_from_env() -> Arc<dyn CredentialCipher> {
    let key_b64 = match std::env::var(ENCRYPTION_KEY_ENV) {
        Ok(v) if !v.trim().is_empty() => v,
        _ => {
            tracing::warn!(
                env_var = ENCRYPTION_KEY_ENV,
                "Connection parameter encryption DISABLED — env var not set. \
                 Credentials will be stored as plaintext in the database. \
                 Set {} to a base64-encoded 32-byte key to enable AES-256-GCM.",
                ENCRYPTION_KEY_ENV,
            );
            return Arc::new(NoOpCipher);
        }
    };

    // Read key material into a zeroizing buffer so the raw string doesn't
    // linger in memory after the cipher is built.
    let key_material = Zeroizing::new(key_b64);
    let key_id = std::env::var(ENCRYPTION_KEY_ID_ENV).unwrap_or_else(|_| "env".to_string());

    match AesGcmCipher::from_base64_key(&key_material, &key_id) {
        Ok(cipher) => {
            tracing::info!(
                key_id = %key_id,
                "Connection parameter encryption ENABLED (AES-256-GCM)"
            );
            Arc::new(cipher)
        }
        Err(e) => {
            tracing::error!(
                env_var = ENCRYPTION_KEY_ENV,
                error = %e,
                "Failed to initialize AES-GCM cipher from {}; falling back to NO-OP (plaintext at rest). \
                 This is a MISCONFIGURATION — fix the key and restart.",
                ENCRYPTION_KEY_ENV,
            );
            Arc::new(NoOpCipher)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;
    use base64::engine::general_purpose::STANDARD as BASE64;

    /// Serialize env-var tests so parallel execution doesn't race on the same var.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn with_env<T>(key: Option<&str>, key_id: Option<&str>, f: impl FnOnce() -> T) -> T {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // SAFETY: protected by ENV_LOCK. Tests that touch env vars are serialized.
        unsafe {
            match key {
                Some(v) => std::env::set_var(ENCRYPTION_KEY_ENV, v),
                None => std::env::remove_var(ENCRYPTION_KEY_ENV),
            }
            match key_id {
                Some(v) => std::env::set_var(ENCRYPTION_KEY_ID_ENV, v),
                None => std::env::remove_var(ENCRYPTION_KEY_ID_ENV),
            }
        }
        let result = f();
        // SAFETY: same as above.
        unsafe {
            std::env::remove_var(ENCRYPTION_KEY_ENV);
            std::env::remove_var(ENCRYPTION_KEY_ID_ENV);
        }
        result
    }

    #[test]
    fn falls_back_to_noop_when_env_missing() {
        with_env(None, None, || {
            let cipher = cipher_from_env();
            assert!(!cipher.is_encrypting());
            assert_eq!(cipher.key_id(), "noop");
        });
    }

    #[test]
    fn falls_back_to_noop_on_empty_env() {
        with_env(Some(""), None, || {
            let cipher = cipher_from_env();
            assert!(!cipher.is_encrypting());
        });
    }

    #[test]
    fn enables_aes_gcm_with_valid_key() {
        let key = BASE64.encode(vec![0u8; 32]);
        with_env(Some(&key), Some("prod-1"), || {
            let cipher = cipher_from_env();
            assert!(cipher.is_encrypting());
            assert_eq!(cipher.key_id(), "prod-1");
        });
    }

    #[test]
    fn falls_back_to_noop_on_invalid_key_material() {
        with_env(Some("not-valid-base64!!!!"), None, || {
            let cipher = cipher_from_env();
            assert!(!cipher.is_encrypting());
        });
    }
}
