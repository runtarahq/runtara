// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Stateless, tenant- and connection-bound endpoint references.
//!
//! An *endpoint ref* is an opaque, HMAC-signed token that binds a validated
//! base URL to a specific `(tenant, connection)`. It lets a credentialed
//! request reach a per-request base URL the connection itself did not pin —
//! the motivating case is a Microsoft Teams conversation's `serviceUrl`, which
//! is per-conversation and cannot be a static connection base.
//!
//! The design is deliberately storage-free. Everything a target needs
//! (serviceUrl, conversation id, owning tenant/connection) is known when the
//! Teams webhook finishes validating an inbound activity, so the ref carries
//! it inline and is signed rather than stored as a row. Durability then falls
//! out for free: the ref rides the durable workflow input envelope (survives
//! restart), it is not derived from connection secrets (survives rotation),
//! and it is revoked implicitly because the proxy re-resolves the connection
//! for auth on every send (a deleted/disabled connection kills its refs).
//!
//! Wire format: `base64url(json_payload).base64url(hmac_sha256)`. The payload
//! carries a `kid` selecting the signing key so the secret can be rotated
//! (accept the previous key during rollover). Verification is constant-time
//! (HMAC `verify_slice`).
//!
//! This module is integration-neutral: the proxy only consumes `tenant_id`,
//! `connection_id`, `base_url`, and (for a defense-in-depth path check)
//! `conversation_id`. The Teams-specific fields are opaque payload it carries.

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::sync::OnceLock;

type HmacSha256 = Hmac<Sha256>;

/// Current signing-key id. Bump (and move the old secret to the PREV slot)
/// to rotate.
const CURRENT_KID: &str = "1";
const PREVIOUS_KID: &str = "0";

/// Payload bound into an endpoint ref. `v` is the format version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EndpointBinding {
    pub v: u8,
    pub tenant_id: String,
    pub connection_id: String,
    /// The validated absolute base URL this ref pins to (e.g. a Teams
    /// serviceUrl). Always https and public — validated before minting.
    pub base_url: String,
    /// Provider conversation identifier (Teams conversation id, incl. any
    /// `;messageid=` thread suffix). Used by the proxy for a path cross-check.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ms_tenant_id: Option<String>,
    /// Issued-at (unix seconds). Informational; refs have no hard expiry.
    pub iat: i64,
}

impl EndpointBinding {
    pub const CURRENT_VERSION: u8 = 1;
}

#[derive(Debug, thiserror::Error)]
pub enum EndpointRefError {
    #[error("RUNTARA_ENDPOINT_REF_SECRET environment variable is not set")]
    MissingSecret,
    #[error("invalid endpoint ref format")]
    InvalidFormat,
    #[error("unknown signing key id")]
    UnknownKid,
    #[error("invalid endpoint ref signature")]
    InvalidSignature,
    #[error("unsupported endpoint ref version")]
    UnsupportedVersion,
}

/// A set of signing keys keyed by `kid`. The first entry is the *current* key
/// used for minting; any entry can verify (supports zero-downtime rotation).
pub struct EndpointRefKeyring {
    current_kid: String,
    keys: Vec<(String, Vec<u8>)>,
}

impl EndpointRefKeyring {
    pub fn new(current_kid: impl Into<String>, current_secret: impl Into<Vec<u8>>) -> Self {
        let kid = current_kid.into();
        Self {
            keys: vec![(kid.clone(), current_secret.into())],
            current_kid: kid,
        }
    }

    /// Add a non-current key that is still accepted for verification.
    pub fn with_additional(mut self, kid: impl Into<String>, secret: impl Into<Vec<u8>>) -> Self {
        self.keys.push((kid.into(), secret.into()));
        self
    }

    fn secret_for(&self, kid: &str) -> Option<&[u8]> {
        self.keys
            .iter()
            .find(|(k, _)| k == kid)
            .map(|(_, s)| s.as_slice())
    }

    /// Process-wide keyring built from the environment.
    ///
    /// `RUNTARA_ENDPOINT_REF_SECRET` is the current key (kid "1");
    /// `RUNTARA_ENDPOINT_REF_SECRET_PREV`, when set, is accepted for
    /// verification (kid "0") during a rotation window.
    pub fn from_env() -> Result<&'static Self, EndpointRefError> {
        static KEYRING: OnceLock<Option<EndpointRefKeyring>> = OnceLock::new();
        KEYRING
            .get_or_init(|| {
                let current = std::env::var("RUNTARA_ENDPOINT_REF_SECRET").ok()?;
                let mut keyring = EndpointRefKeyring::new(CURRENT_KID, current.into_bytes());
                if let Ok(prev) = std::env::var("RUNTARA_ENDPOINT_REF_SECRET_PREV")
                    && !prev.is_empty()
                {
                    keyring = keyring.with_additional(PREVIOUS_KID, prev.into_bytes());
                }
                Some(keyring)
            })
            .as_ref()
            .ok_or(EndpointRefError::MissingSecret)
    }
}

/// Mint an endpoint ref for `binding`, signed with the keyring's current key.
pub fn sign(keyring: &EndpointRefKeyring, binding: &EndpointBinding) -> String {
    let kid = &keyring.current_kid;
    let secret = keyring
        .secret_for(kid)
        .expect("current kid always present in keyring");
    let payload_json = serde_json::to_vec(binding).expect("EndpointBinding serializes");
    let encoded_payload = URL_SAFE_NO_PAD.encode(&payload_json);
    let signing_input = format!("{kid}.{encoded_payload}");

    let mut mac = HmacSha256::new_from_slice(secret).expect("HMAC takes any key size");
    mac.update(signing_input.as_bytes());
    let sig = URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes());

    format!("{signing_input}.{sig}")
}

/// Verify an endpoint ref and return its binding. Signature is checked in
/// constant time; the version is enforced.
pub fn verify(
    keyring: &EndpointRefKeyring,
    token: &str,
) -> Result<EndpointBinding, EndpointRefError> {
    let mut parts = token.splitn(3, '.');
    let kid = parts.next().ok_or(EndpointRefError::InvalidFormat)?;
    let encoded_payload = parts.next().ok_or(EndpointRefError::InvalidFormat)?;
    let encoded_sig = parts.next().ok_or(EndpointRefError::InvalidFormat)?;
    if parts.next().is_some() {
        return Err(EndpointRefError::InvalidFormat);
    }

    let secret = keyring
        .secret_for(kid)
        .ok_or(EndpointRefError::UnknownKid)?;
    let signing_input = format!("{kid}.{encoded_payload}");
    let mut mac = HmacSha256::new_from_slice(secret).expect("HMAC takes any key size");
    mac.update(signing_input.as_bytes());
    let sig_bytes = URL_SAFE_NO_PAD
        .decode(encoded_sig)
        .map_err(|_| EndpointRefError::InvalidFormat)?;
    mac.verify_slice(&sig_bytes)
        .map_err(|_| EndpointRefError::InvalidSignature)?;

    let payload_bytes = URL_SAFE_NO_PAD
        .decode(encoded_payload)
        .map_err(|_| EndpointRefError::InvalidFormat)?;
    let binding: EndpointBinding =
        serde_json::from_slice(&payload_bytes).map_err(|_| EndpointRefError::InvalidFormat)?;
    if binding.v != EndpointBinding::CURRENT_VERSION {
        return Err(EndpointRefError::UnsupportedVersion);
    }
    Ok(binding)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keyring() -> EndpointRefKeyring {
        EndpointRefKeyring::new(CURRENT_KID, b"unit-test-secret".to_vec())
    }

    fn binding() -> EndpointBinding {
        EndpointBinding {
            v: EndpointBinding::CURRENT_VERSION,
            tenant_id: "tenant-a".into(),
            connection_id: "conn-1".into(),
            base_url: "https://smba.trafficmanager.net/amer/".into(),
            conversation_id: Some("19:abc@thread.tacv2;messageid=1".into()),
            conversation_type: Some("channel".into()),
            ms_tenant_id: Some("ms-tenant".into()),
            iat: 1_700_000_000,
        }
    }

    #[test]
    fn sign_verify_roundtrip() {
        let kr = keyring();
        let token = sign(&kr, &binding());
        let parsed = verify(&kr, &token).unwrap();
        assert_eq!(parsed, binding());
    }

    #[test]
    fn tamper_in_payload_is_rejected() {
        let kr = keyring();
        let token = sign(&kr, &binding());
        // Flip a character in the payload segment.
        let mut parts: Vec<&str> = token.split('.').collect();
        let payload = parts[1].to_string();
        let mutated: String = payload
            .chars()
            .enumerate()
            .map(|(i, c)| {
                if i == 0 {
                    if c == 'A' { 'B' } else { 'A' }
                } else {
                    c
                }
            })
            .collect();
        parts[1] = &mutated;
        let tampered = parts.join(".");
        assert!(matches!(
            verify(&kr, &tampered),
            Err(EndpointRefError::InvalidSignature | EndpointRefError::InvalidFormat)
        ));
    }

    #[test]
    fn wrong_key_is_rejected() {
        let signer = keyring();
        let token = sign(&signer, &binding());
        let other = EndpointRefKeyring::new(CURRENT_KID, b"a-different-secret".to_vec());
        assert!(matches!(
            verify(&other, &token),
            Err(EndpointRefError::InvalidSignature)
        ));
    }

    #[test]
    fn unknown_kid_is_rejected() {
        let signer = EndpointRefKeyring::new("9", b"unit-test-secret".to_vec());
        let token = sign(&signer, &binding());
        // Verifier only knows kid "1".
        assert!(matches!(
            verify(&keyring(), &token),
            Err(EndpointRefError::UnknownKid)
        ));
    }

    #[test]
    fn previous_key_still_verifies_during_rotation() {
        // Token minted under the old key; verifier has rotated current forward
        // but still lists the old key as additional.
        let old = EndpointRefKeyring::new(PREVIOUS_KID, b"old-secret".to_vec());
        let token = sign(&old, &binding());
        let rotated = EndpointRefKeyring::new(CURRENT_KID, b"new-secret".to_vec())
            .with_additional(PREVIOUS_KID, b"old-secret".to_vec());
        assert_eq!(verify(&rotated, &token).unwrap(), binding());
    }

    #[test]
    fn malformed_tokens_are_rejected() {
        let kr = keyring();
        assert!(verify(&kr, "").is_err());
        assert!(verify(&kr, "one-part").is_err());
        assert!(verify(&kr, "a.b").is_err());
        assert!(verify(&kr, "a.b.c.d").is_err());
    }

    #[test]
    fn unsupported_version_is_rejected() {
        let kr = keyring();
        let mut b = binding();
        b.v = 99;
        let token = sign(&kr, &b);
        assert!(matches!(
            verify(&kr, &token),
            Err(EndpointRefError::UnsupportedVersion)
        ));
    }
}
