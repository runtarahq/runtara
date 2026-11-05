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

/// Retired keys that still decrypt (`kid:base64,kid:base64`), kept until
/// `runtara-server reencrypt-connections` has moved every credential to the
/// current key.
pub const RETIRED_KEYS_ENV: &str = "RUNTARA_CONNECTIONS_RETIRED_KEYS";

/// Build a cipher by reading [`ENCRYPTION_KEY_ENV`].
///
/// Connection parameters are always encrypted at rest: there is no plaintext
/// fallback. Returns:
/// - `Ok(`[`AesGcmCipher`]`)` if the env var is set and valid (plus any
///   [`RETIRED_KEYS_ENV`] keys, which still decrypt)
/// - `Err` if the key is missing, empty or invalid, or a retired key is
///   malformed. The caller is expected to refuse to start.
pub fn cipher_from_env() -> Result<Arc<dyn CredentialCipher>, String> {
    let key_b64 = match std::env::var(ENCRYPTION_KEY_ENV) {
        Ok(v) if !v.trim().is_empty() => v,
        _ => {
            return Err(format!(
                "{ENCRYPTION_KEY_ENV} is not set. Connection credentials are always \
                 encrypted at rest; set it to a base64-encoded 32-byte key \
                 (openssl rand -base64 32) and keep a backup of it — credentials \
                 stored under a lost key cannot be recovered."
            ));
        }
    };

    // Read key material into a zeroizing buffer so the raw string doesn't
    // linger in memory after the cipher is built.
    let key_material = Zeroizing::new(key_b64);
    let key_id = std::env::var(ENCRYPTION_KEY_ID_ENV).unwrap_or_else(|_| "env".to_string());

    let cipher = AesGcmCipher::from_base64_key(&key_material, &key_id)
        .and_then(|cipher| {
            let retired = Zeroizing::new(std::env::var(RETIRED_KEYS_ENV).unwrap_or_default());
            retired
                .split(',')
                .map(str::trim)
                .filter(|entry| !entry.is_empty())
                .try_fold(cipher, |cipher, entry| {
                    let (retired_id, retired_key) = entry.split_once(':').ok_or_else(|| {
                        super::cipher::CipherError::InvalidKey(format!(
                            "{RETIRED_KEYS_ENV} entries are kid:base64"
                        ))
                    })?;
                    cipher.with_retired_base64_key(retired_key, retired_id)
                })
        })
        .map_err(|e| format!("{ENCRYPTION_KEY_ENV} is invalid: {e}"))?;

    tracing::info!(
        key_id = %key_id,
        "Connection parameter encryption ENABLED (AES-256-GCM)"
    );
    Ok(Arc::new(cipher))
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;
    use base64::engine::general_purpose::STANDARD as BASE64;

    /// Serialize env-var tests so parallel execution doesn't race on the same var.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Sets (or clears, for `None`) every env var this module reads, runs
    /// `f`, then clears them all again so nothing leaks into a later test.
    fn with_env<T>(
        key: Option<&str>,
        key_id: Option<&str>,
        retired: Option<&str>,
        f: impl FnOnce() -> T,
    ) -> T {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let vars = [
            (ENCRYPTION_KEY_ENV, key),
            (ENCRYPTION_KEY_ID_ENV, key_id),
            (RETIRED_KEYS_ENV, retired),
        ];
        // SAFETY: protected by ENV_LOCK. Tests that touch env vars are serialized.
        unsafe {
            for (name, value) in vars {
                match value {
                    Some(v) => std::env::set_var(name, v),
                    None => std::env::remove_var(name),
                }
            }
        }
        let result = f();
        // SAFETY: same as above.
        unsafe {
            for (name, _) in vars {
                std::env::remove_var(name);
            }
        }
        result
    }

    /// `Result::expect_err` needs `T: Debug`, which `Arc<dyn CredentialCipher>`
    /// doesn't implement — sensibly, for a type that can hold raw key
    /// material. Match it out by hand instead.
    fn expect_err(result: Result<Arc<dyn CredentialCipher>, String>, msg: &str) -> String {
        match result {
            Err(e) => e,
            Ok(_) => panic!("{msg}"),
        }
    }

    // There is no plaintext fallback: a missing or bad key must stop the
    // process from starting rather than store credentials in cleartext.

    #[test]
    fn fails_when_key_missing() {
        with_env(None, None, None, || {
            let err = expect_err(cipher_from_env(), "missing key must fail");
            assert!(err.contains(ENCRYPTION_KEY_ENV), "{err}");
        });
    }

    #[test]
    fn fails_when_key_empty() {
        with_env(Some("   "), None, None, || {
            let err = expect_err(cipher_from_env(), "blank key must fail");
            assert!(err.contains(ENCRYPTION_KEY_ENV), "{err}");
        });
    }

    #[test]
    fn fails_on_invalid_key_material() {
        with_env(Some("not-valid-base64!!!!"), None, None, || {
            let err = expect_err(cipher_from_env(), "invalid key must fail");
            assert!(err.contains(ENCRYPTION_KEY_ENV), "{err}");
        });
    }

    #[test]
    fn fails_on_wrong_key_length() {
        let short = BASE64.encode(vec![0u8; 16]);
        with_env(Some(&short), None, None, || {
            expect_err(cipher_from_env(), "a 16-byte key must fail");
        });
    }

    #[test]
    fn fails_on_malformed_retired_key() {
        let key = BASE64.encode(vec![0u8; 32]);
        with_env(Some(&key), None, Some("no-colon-here"), || {
            let err = expect_err(cipher_from_env(), "malformed retired key must fail");
            assert!(err.contains(RETIRED_KEYS_ENV), "{err}");
        });
    }

    #[test]
    fn enables_aes_gcm_with_valid_key() {
        let key = BASE64.encode(vec![0u8; 32]);
        with_env(Some(&key), Some("prod-1"), None, || {
            let cipher = cipher_from_env().expect("valid key always succeeds");
            assert!(cipher.is_encrypting());
            assert_eq!(cipher.key_id(), "prod-1");
        });
    }

    #[test]
    fn defaults_key_id_to_env() {
        let key = BASE64.encode(vec![0u8; 32]);
        with_env(Some(&key), None, None, || {
            let cipher = cipher_from_env().expect("valid key always succeeds");
            assert_eq!(cipher.key_id(), "env");
        });
    }

    /// Variables from the removed opt-in no longer bring the fallback back.
    #[test]
    fn legacy_opt_out_variables_do_not_disable_encryption() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // SAFETY: protected by ENV_LOCK.
        unsafe {
            std::env::remove_var(ENCRYPTION_KEY_ENV);
            std::env::set_var("RUNTARA_REQUIRE_CREDENTIAL_ENCRYPTION", "false");
            std::env::set_var("RUNTARA_ENV", "development");
        }
        let result = cipher_from_env();
        // SAFETY: same as above.
        unsafe {
            std::env::remove_var("RUNTARA_REQUIRE_CREDENTIAL_ENCRYPTION");
            std::env::remove_var("RUNTARA_ENV");
        }
        expect_err(result, "no variable may opt back into plaintext");
    }
}
