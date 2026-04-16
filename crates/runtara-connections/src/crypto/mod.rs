//! At-rest encryption for connection parameters.
//!
//! Credentials stored in `connection_data_entity.connection_parameters` are
//! encrypted using a pluggable [`CredentialCipher`] trait. The repository
//! layer is the only caller — consumers of the facade always see decrypted
//! plaintext JSON.
//!
//! # Envelope format
//!
//! Encrypted payloads are stored as JSONB envelopes:
//!
//! ```json
//! {
//!   "v": 1,
//!   "alg": "aes-256-gcm",
//!   "kid": "env",
//!   "nonce": "<base64 12 bytes>",
//!   "ct": "<base64 ciphertext+tag>"
//! }
//! ```
//!
//! # Rollout / mixed state
//!
//! [`CredentialCipher::decrypt`] passes plaintext through unchanged when it
//! does not recognize the envelope shape. This lets the system be deployed
//! against existing plaintext data without a hard migration — new writes
//! encrypt, old plaintext is returned as-is. Run [`ReencryptJob`] to eagerly
//! convert existing plaintext to ciphertext.
//!
//! # Pluggable backends
//!
//! [`CredentialCipher`] is a trait, so future KMS / Vault / cloud-secret
//! backends can replace [`aes_gcm::AesGcmCipher`] without touching the
//! repository or any consumer.

pub mod aes_gcm;
pub mod cipher;
pub mod factory;
pub mod noop;

pub use cipher::{CipherError, CredentialCipher, ENVELOPE_ALG, ENVELOPE_VERSION};
pub use factory::{ENCRYPTION_KEY_ENV, cipher_from_env};
