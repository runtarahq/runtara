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

/// When set to a truthy value (`true`/`1`/`yes`/`on`, case-insensitive),
/// [`cipher_from_env`] refuses to fall back to [`NoOpCipher`] and returns an
/// error instead — plaintext-at-rest becomes a boot failure rather than a
/// log line.
pub const REQUIRE_ENCRYPTION_ENV: &str = "RUNTARA_REQUIRE_CREDENTIAL_ENCRYPTION";

/// Deployment-environment signal this crate understands. `RUNTARA_ENV=production`
/// implies the same requirement as [`REQUIRE_ENCRYPTION_ENV`] — a deployment that
/// has already declared itself production shouldn't need a second opt-in to
/// refuse plaintext credential storage.
pub const DEPLOYMENT_ENV_VAR: &str = "RUNTARA_ENV";

/// Whether a missing/invalid encryption key should hard-fail [`cipher_from_env`]
/// instead of falling back to [`NoOpCipher`].
fn encryption_is_required() -> bool {
    let explicitly_required = std::env::var(REQUIRE_ENCRYPTION_ENV)
        .map(|v| {
            matches!(
                v.trim().to_ascii_lowercase().as_str(),
                "true" | "1" | "yes" | "on"
            )
        })
        .unwrap_or(false);

    let is_production = std::env::var(DEPLOYMENT_ENV_VAR)
        .map(|v| v.trim().eq_ignore_ascii_case("production"))
        .unwrap_or(false);

    explicitly_required || is_production
}

/// Build a cipher by reading [`ENCRYPTION_KEY_ENV`].
///
/// Returns:
/// - `Ok(`[`AesGcmCipher`]`)` if the env var is set and valid
/// - `Ok(`[`NoOpCipher`]`)` with a prominent warning log if the key is missing or
///   invalid and encryption isn't required (the default — local/dev deployments
///   without the key configured keep working, just with plaintext at rest)
/// - `Err` if the key is missing or invalid *and* [`REQUIRE_ENCRYPTION_ENV`] or
///   `RUNTARA_ENV=production` is set — the caller is expected to hard-fail
///   startup rather than silently store credentials in cleartext
pub fn cipher_from_env() -> Result<Arc<dyn CredentialCipher>, String> {
    let required = encryption_is_required();

    let key_b64 = match std::env::var(ENCRYPTION_KEY_ENV) {
        Ok(v) if !v.trim().is_empty() => v,
        _ => {
            if required {
                return Err(format!(
                    "Connection parameter encryption is required ({} or {}=production is \
                     set) but {} is not set. Set it to a base64-encoded 32-byte key \
                     (openssl rand -base64 32).",
                    REQUIRE_ENCRYPTION_ENV, DEPLOYMENT_ENV_VAR, ENCRYPTION_KEY_ENV,
                ));
            }
            tracing::warn!(
                env_var = ENCRYPTION_KEY_ENV,
                "Connection parameter encryption DISABLED — env var not set. \
                 Credentials will be stored as plaintext in the database. \
                 Set {} to a base64-encoded 32-byte key to enable AES-256-GCM.",
                ENCRYPTION_KEY_ENV,
            );
            return Ok(Arc::new(NoOpCipher));
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
            Ok(Arc::new(cipher))
        }
        Err(e) => {
            if required {
                return Err(format!(
                    "Connection parameter encryption is required ({} or {}=production is \
                     set) but {} is invalid: {}",
                    REQUIRE_ENCRYPTION_ENV, DEPLOYMENT_ENV_VAR, ENCRYPTION_KEY_ENV, e,
                ));
            }
            tracing::error!(
                env_var = ENCRYPTION_KEY_ENV,
                error = %e,
                "Failed to initialize AES-GCM cipher from {}; falling back to NO-OP (plaintext at rest). \
                 This is a MISCONFIGURATION — fix the key and restart.",
                ENCRYPTION_KEY_ENV,
            );
            Ok(Arc::new(NoOpCipher))
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

    /// Sets (or clears, for `None`) all four env vars this module reads, runs
    /// `f`, then clears all four again — so a test that sets
    /// `REQUIRE_ENCRYPTION_ENV`/`DEPLOYMENT_ENV_VAR` can't leak into a later
    /// test that doesn't go through this helper for them.
    fn with_env<T>(
        key: Option<&str>,
        key_id: Option<&str>,
        require_encryption: Option<&str>,
        deployment_env: Option<&str>,
        f: impl FnOnce() -> T,
    ) -> T {
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
            match require_encryption {
                Some(v) => std::env::set_var(REQUIRE_ENCRYPTION_ENV, v),
                None => std::env::remove_var(REQUIRE_ENCRYPTION_ENV),
            }
            match deployment_env {
                Some(v) => std::env::set_var(DEPLOYMENT_ENV_VAR, v),
                None => std::env::remove_var(DEPLOYMENT_ENV_VAR),
            }
        }
        let result = f();
        // SAFETY: same as above.
        unsafe {
            std::env::remove_var(ENCRYPTION_KEY_ENV);
            std::env::remove_var(ENCRYPTION_KEY_ID_ENV);
            std::env::remove_var(REQUIRE_ENCRYPTION_ENV);
            std::env::remove_var(DEPLOYMENT_ENV_VAR);
        }
        result
    }

    #[test]
    fn falls_back_to_noop_when_env_missing() {
        with_env(None, None, None, None, || {
            let cipher = cipher_from_env().expect("not required, so this can't fail");
            assert!(!cipher.is_encrypting());
            assert_eq!(cipher.key_id(), "noop");
        });
    }

    #[test]
    fn falls_back_to_noop_on_empty_env() {
        with_env(Some(""), None, None, None, || {
            let cipher = cipher_from_env().expect("not required, so this can't fail");
            assert!(!cipher.is_encrypting());
        });
    }

    #[test]
    fn enables_aes_gcm_with_valid_key() {
        let key = BASE64.encode(vec![0u8; 32]);
        with_env(Some(&key), Some("prod-1"), None, None, || {
            let cipher = cipher_from_env().expect("valid key always succeeds");
            assert!(cipher.is_encrypting());
            assert_eq!(cipher.key_id(), "prod-1");
        });
    }

    #[test]
    fn falls_back_to_noop_on_invalid_key_material() {
        with_env(Some("not-valid-base64!!!!"), None, None, None, || {
            let cipher = cipher_from_env().expect("not required, so this can't fail");
            assert!(!cipher.is_encrypting());
        });
    }

    // Regression coverage for the silent-plaintext-fallback gap (SYN-461): a
    // deployment that has opted into requiring encryption — explicitly, or by
    // declaring itself production — must fail to boot rather than quietly
    // store credentials in cleartext.

    /// `Result::expect_err` needs `T: Debug`, which `Arc<dyn CredentialCipher>`
    /// doesn't implement — sensibly, for a type that can hold raw key
    /// material. Match it out by hand instead.
    fn expect_err(result: Result<Arc<dyn CredentialCipher>, String>, msg: &str) -> String {
        match result {
            Err(e) => e,
            Ok(_) => panic!("{msg}"),
        }
    }

    #[test]
    fn hard_fails_when_required_and_key_missing() {
        with_env(None, None, Some("true"), None, || {
            let err = expect_err(cipher_from_env(), "required + missing key must fail");
            assert!(err.contains(REQUIRE_ENCRYPTION_ENV), "{err}");
            assert!(err.contains(ENCRYPTION_KEY_ENV), "{err}");
        });
    }

    #[test]
    fn hard_fails_when_required_and_key_invalid() {
        with_env(Some("not-valid-base64!!!!"), None, Some("1"), None, || {
            let err = expect_err(cipher_from_env(), "required + invalid key must fail");
            assert!(err.contains(ENCRYPTION_KEY_ENV), "{err}");
        });
    }

    #[test]
    fn production_deployment_env_requires_encryption_without_explicit_flag() {
        with_env(None, None, None, Some("production"), || {
            let err = expect_err(
                cipher_from_env(),
                "RUNTARA_ENV=production must require encryption",
            );
            assert!(err.contains(DEPLOYMENT_ENV_VAR), "{err}");
        });
    }

    #[test]
    fn non_production_deployment_env_does_not_require_encryption() {
        with_env(None, None, None, Some("development"), || {
            let cipher =
                cipher_from_env().expect("non-production RUNTARA_ENV must not require encryption");
            assert!(!cipher.is_encrypting());
        });
    }

    #[test]
    fn required_flag_does_not_block_a_valid_key() {
        let key = BASE64.encode(vec![0u8; 32]);
        with_env(Some(&key), Some("prod-1"), Some("true"), None, || {
            let cipher = cipher_from_env().expect("a valid key satisfies the requirement");
            assert!(cipher.is_encrypting());
            assert_eq!(cipher.key_id(), "prod-1");
        });
    }
}
