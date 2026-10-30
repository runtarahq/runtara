//! Passthrough cipher used when encryption is not configured.
//!
//! This is **not secure** and is only appropriate for local development or
//! the initial rollout window before a key is provisioned.
//! [`crate::crypto::factory::cipher_from_env`] emits a warning log when it
//! falls back to this cipher so misconfigurations are visible.

use serde_json::Value;

use super::cipher::{CipherContext, CipherError, CredentialCipher};

pub struct NoOpCipher;

impl CredentialCipher for NoOpCipher {
    fn encrypt(&self, plaintext: &Value, _context: &CipherContext) -> Result<Value, CipherError> {
        Ok(plaintext.clone())
    }

    fn decrypt(&self, stored: &Value, _context: &CipherContext) -> Result<Value, CipherError> {
        Ok(stored.clone())
    }

    fn key_id(&self) -> &str {
        "noop"
    }

    fn is_encrypting(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn passes_through_unchanged() {
        let cipher = NoOpCipher;
        let value = json!({"api_key": "sk-123"});
        assert_eq!(
            cipher
                .encrypt(
                    &value,
                    &CipherContext {
                        tenant_id: "t",
                        connection_id: "c"
                    }
                )
                .unwrap(),
            value
        );
        assert_eq!(
            cipher
                .decrypt(
                    &value,
                    &CipherContext {
                        tenant_id: "t",
                        connection_id: "c"
                    }
                )
                .unwrap(),
            value
        );
    }

    #[test]
    fn reports_not_encrypting() {
        assert!(!NoOpCipher.is_encrypting());
    }
}
