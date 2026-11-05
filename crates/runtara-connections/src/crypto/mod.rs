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
//! # Plaintext from before encryption was mandatory
//!
//! [`CredentialCipher::decrypt`] refuses a value that is not an envelope.
//! Rows stored as plaintext by older releases are encrypted when the server
//! starts ([`ConnectionRepository::encrypt_plaintext`]) and by
//! `runtara-server reencrypt-connections`
//! ([`ConnectionRepository::reencrypt_all`]), which also moves envelopes to
//! the current key.
//!
//! [`ConnectionRepository::encrypt_plaintext`]: crate::repository::connections::ConnectionRepository::encrypt_plaintext
//! [`ConnectionRepository::reencrypt_all`]: crate::repository::connections::ConnectionRepository::reencrypt_all
//!
//! # Pluggable backends
//!
//! [`CredentialCipher`] is a trait, so future KMS / Vault / cloud-secret
//! backends can replace [`aes_gcm::AesGcmCipher`] without touching the
//! repository or any consumer.

pub mod aes_gcm;
pub mod cipher;
pub mod factory;
/// Plaintext passthrough for tests only. Production builds cannot construct
/// it: credentials are always encrypted at rest.
#[cfg(any(test, feature = "test-util"))]
pub mod noop;
pub mod tenant_keyed;

pub use cipher::{
    BOUND_ENVELOPE_VERSION, CipherContext, CipherError, CredentialCipher, ENVELOPE_ALG,
    ENVELOPE_VERSION, is_envelope,
};
pub use factory::{ENCRYPTION_KEY_ENV, cipher_from_env};
pub use tenant_keyed::TenantKeyedCipher;
