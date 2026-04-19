use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use std::sync::OnceLock;

type HmacSha256 = Hmac<Sha256>;

static TOKEN_SECRET: OnceLock<Vec<u8>> = OnceLock::new();

/// Parsed and verified session token.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct SessionToken {
    pub org_id: String,
    pub workflow_id: String,
    pub session_id: String,
}

#[derive(Debug, thiserror::Error)]
pub enum SessionTokenError {
    #[error("SESSION_TOKEN_SECRET environment variable is not set")]
    MissingSecret,
    #[allow(dead_code)]
    #[error("invalid token format")]
    InvalidFormat,
    #[allow(dead_code)]
    #[error("invalid token signature")]
    InvalidSignature,
}

fn get_secret() -> Result<&'static [u8], SessionTokenError> {
    // Try to get already-initialized value first
    if let Some(v) = TOKEN_SECRET.get() {
        return Ok(v.as_slice());
    }
    // Initialize from env var
    let secret = std::env::var("SESSION_TOKEN_SECRET")
        .map(|s| s.into_bytes())
        .map_err(|_| SessionTokenError::MissingSecret)?;
    // set() may fail if another thread raced us — that's fine, just use get()
    let _ = TOKEN_SECRET.set(secret);
    TOKEN_SECRET
        .get()
        .map(|v| v.as_slice())
        .ok_or(SessionTokenError::MissingSecret)
}

/// Sign a session token: `base64url(org_id:workflow_id:session_id).base64url(hmac)`
pub fn sign(
    org_id: &str,
    workflow_id: &str,
    session_id: &str,
) -> Result<String, SessionTokenError> {
    let secret = get_secret()?;
    let payload = format!("{}:{}:{}", org_id, workflow_id, session_id);
    let encoded_payload = URL_SAFE_NO_PAD.encode(payload.as_bytes());

    let mut mac = HmacSha256::new_from_slice(secret).expect("HMAC can take key of any size");
    mac.update(encoded_payload.as_bytes());
    let signature = mac.finalize().into_bytes();
    let encoded_sig = URL_SAFE_NO_PAD.encode(signature);

    Ok(format!("{}.{}", encoded_payload, encoded_sig))
}

/// Verify a session token and return the parsed claims.
#[allow(dead_code)]
pub fn verify(token_str: &str) -> Result<SessionToken, SessionTokenError> {
    let secret = get_secret()?;

    let (encoded_payload, encoded_sig) = token_str
        .split_once('.')
        .ok_or(SessionTokenError::InvalidFormat)?;

    // Verify HMAC
    let mut mac = HmacSha256::new_from_slice(secret).expect("HMAC can take key of any size");
    mac.update(encoded_payload.as_bytes());

    let sig_bytes = URL_SAFE_NO_PAD
        .decode(encoded_sig)
        .map_err(|_| SessionTokenError::InvalidFormat)?;

    mac.verify_slice(&sig_bytes)
        .map_err(|_| SessionTokenError::InvalidSignature)?;

    // Decode payload
    let payload_bytes = URL_SAFE_NO_PAD
        .decode(encoded_payload)
        .map_err(|_| SessionTokenError::InvalidFormat)?;

    let payload = String::from_utf8(payload_bytes).map_err(|_| SessionTokenError::InvalidFormat)?;

    let parts: Vec<&str> = payload.splitn(3, ':').collect();
    if parts.len() != 3 {
        return Err(SessionTokenError::InvalidFormat);
    }

    Ok(SessionToken {
        org_id: parts[0].to_string(),
        workflow_id: parts[1].to_string(),
        session_id: parts[2].to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup_secret() {
        // Force re-init for tests (OnceLock is set once per process, so we use env var)
        // SAFETY: Tests are run single-threaded for this module (no concurrent env access).
        unsafe {
            std::env::set_var("SESSION_TOKEN_SECRET", "test-secret-key-for-hmac");
        }
        // Ensure the OnceLock is initialized
        let _ = get_secret();
    }

    #[test]
    fn test_sign_verify_roundtrip() {
        setup_secret();
        let token = sign("org_123", "workflow_abc", "session_xyz").unwrap();
        let parsed = verify(&token).unwrap();
        assert_eq!(parsed.org_id, "org_123");
        assert_eq!(parsed.workflow_id, "workflow_abc");
        assert_eq!(parsed.session_id, "session_xyz");
    }

    #[test]
    fn test_tamper_detection() {
        setup_secret();
        let token = sign("org_123", "workflow_abc", "session_xyz").unwrap();
        // Tamper with payload
        let tampered = format!("{}X", token);
        assert!(verify(&tampered).is_err());
    }

    #[test]
    fn test_invalid_format() {
        setup_secret();
        assert!(verify("no-dot-separator").is_err());
        assert!(verify("").is_err());
        assert!(verify(".").is_err());
    }

    #[test]
    fn test_consistent_signatures() {
        setup_secret();
        let token1 = sign("org_1", "s1", "sess1").unwrap();
        let token2 = sign("org_1", "s1", "sess1").unwrap();
        assert_eq!(token1, token2);
    }

    #[test]
    fn test_different_inputs_different_tokens() {
        setup_secret();
        let token1 = sign("org_1", "s1", "sess1").unwrap();
        let token2 = sign("org_1", "s1", "sess2").unwrap();
        assert_ne!(token1, token2);
    }
}
