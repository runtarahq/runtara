//! The [`CredentialCipher`] trait that all encryption backends implement.
//!
//! Implementations must be thread-safe (`Send + Sync`) and cheap to clone
//! through `Arc`, since the cipher is constructed once at startup and shared
//! across all repository calls.

use serde_json::Value;

/// Envelope schema version. Bump when changing the envelope format.
pub const ENVELOPE_VERSION: u32 = 1;

/// The envelope that binds a credential to its tenant and connection
/// (associated data, see [`CipherContext`]). New writes use it; version 1
/// envelopes still decrypt, unbound, until re-encrypted.
pub const BOUND_ENVELOPE_VERSION: u32 = 2;

/// What a stored credential belongs to. Its ciphertext is bound to both, so
/// one moved to another connection or tenant does not decrypt.
#[derive(Debug, Clone, Copy)]
pub struct CipherContext<'a> {
    pub tenant_id: &'a str,
    pub connection_id: &'a str,
}

impl CipherContext<'_> {
    /// The associated data authenticated with the ciphertext.
    pub fn aad(&self) -> Vec<u8> {
        format!(
            "runtara:connection:v2\0{}\0{}",
            self.tenant_id, self.connection_id
        )
        .into_bytes()
    }
}

/// Algorithm identifier stored in the envelope. Used for forward-compat so a
/// future cipher (XChaCha20-Poly1305, KMS-wrapped DEK, etc.) can coexist with
/// existing AES-GCM data.
pub const ENVELOPE_ALG: &str = "aes-256-gcm";

/// Error returned by cipher operations.
///
/// `Encrypt` and `Decrypt` deliberately do not include the underlying data in
/// their message to avoid leaking plaintext / ciphertext into logs.
#[derive(Debug, thiserror::Error)]
pub enum CipherError {
    #[error("encryption failed: {0}")]
    Encrypt(String),

    #[error("decryption failed: {0}")]
    Decrypt(String),

    #[error("invalid envelope: {0}")]
    InvalidEnvelope(String),

    #[error("invalid key material: {0}")]
    InvalidKey(String),
}

/// Pluggable cipher for connection parameter at-rest encryption.
///
/// The current production implementation is [`crate::crypto::aes_gcm::AesGcmCipher`],
/// which reads an AES-256 key from an environment variable. Future KMS-backed
/// implementations (AWS KMS DEKs, Vault transit, GCP KMS) plug in here without
/// any changes to the repository or consumers.
///
/// All methods are synchronous because real-world implementations cache a
/// data encryption key (DEK) locally at startup and perform per-op crypto in
/// memory. If a backend requires a network round-trip per operation, wrap it
/// in an in-process cache.
pub trait CredentialCipher: Send + Sync {
    /// Encrypt plaintext JSON, returning an envelope JSON value suitable for
    /// storing in the `connection_parameters` JSONB column.
    ///
    /// A fresh random nonce must be generated for every call — never reuse.
    fn encrypt(&self, plaintext: &Value, context: &CipherContext) -> Result<Value, CipherError>;

    /// Decrypt an envelope JSON value. If the input is not a recognized
    /// envelope (no `"v"` + `"alg"` fields), return it unchanged — this
    /// supports the gradual-rollout workflow where existing rows are still
    /// plaintext.
    fn decrypt(&self, stored: &Value, context: &CipherContext) -> Result<Value, CipherError>;

    /// Identifier for the key currently in use. Stored in the envelope's
    /// `"kid"` field to support key rotation (decrypt uses the key named in
    /// the envelope; encrypt uses the cipher's current key).
    fn key_id(&self) -> &str;

    /// Whether this cipher provides real encryption.
    ///
    /// Used for startup logging: operators should see a warning when running
    /// without encryption so it isn't silently disabled in production.
    fn is_encrypting(&self) -> bool;

    /// Whether `tenant`'s credentials are sealed with real encryption: for a
    /// cipher that keys each tenant apart, that tenant's own, whatever the
    /// others' or the default's.
    fn is_encrypting_for(&self, tenant: &str) -> bool {
        let _ = tenant;
        self.is_encrypting()
    }
}

/// Returns `true` if `value` looks like one of our encrypted envelopes.
///
/// Used by [`CredentialCipher::decrypt`] implementations to distinguish
/// envelopes from plaintext during the rollout window.
pub fn is_envelope(value: &Value) -> bool {
    let Some(obj) = value.as_object() else {
        return false;
    };
    matches!(obj.get("v"), Some(Value::Number(_)))
        && matches!(obj.get("alg"), Some(Value::String(_)))
        && matches!(obj.get("nonce"), Some(Value::String(_)))
        && matches!(obj.get("ct"), Some(Value::String(_)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn is_envelope_detects_envelope_shape() {
        let envelope = json!({
            "v": 1,
            "alg": "aes-256-gcm",
            "kid": "env",
            "nonce": "dGVzdA==",
            "ct": "aGVsbG8="
        });
        assert!(is_envelope(&envelope));
    }

    #[test]
    fn is_envelope_rejects_plaintext() {
        assert!(!is_envelope(&json!({"api_key": "sk-123"})));
        assert!(!is_envelope(&json!({})));
        assert!(!is_envelope(&json!(null)));
        assert!(!is_envelope(&json!("string")));
    }

    #[test]
    fn is_envelope_rejects_partial_envelope() {
        // Missing "ct"
        assert!(!is_envelope(&json!({
            "v": 1,
            "alg": "aes-256-gcm",
            "nonce": "dGVzdA=="
        })));
        // v is a string, not a number
        assert!(!is_envelope(&json!({
            "v": "1",
            "alg": "aes-256-gcm",
            "nonce": "dGVzdA==",
            "ct": "aGVsbG8="
        })));
    }
}
