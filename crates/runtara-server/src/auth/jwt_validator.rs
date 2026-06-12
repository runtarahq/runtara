use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header};

use super::JwtConfig;

/// The Auth0 custom-claim namespace runtara normalizes from.
///
/// Auth0 strips non-namespaced custom claims depending on audience config, so the Action may
/// emit `org_id` only as `https://runtara.io/org_id`. runtara owns normalization and accepts
/// both shapes; this is the documented prefix the `#[serde(alias = ...)]` attributes on
/// [`Claims`] use. It MUST match whatever the Auth0 Action emits.
/// serde aliases require string literals, so the prefix is repeated in the attributes below;
/// the `normalizes_namespaced_claims` test builds its keys from this const to guard against
/// the two drifting apart.
pub const CLAIM_NAMESPACE: &str = "https://runtara.io/";

/// JWT claims expected in the token payload.
///
/// Custom claims injected by the Auth0 Post-Login Action may arrive either in raw form
/// (`org_id`) or namespaced (`https://runtara.io/org_id`) — Auth0 strips non-namespaced
/// custom claims depending on the audience configuration, so we cannot assume one shape.
/// runtara owns normalization: each custom claim carries a `#[serde(alias = ...)]` for the
/// namespaced key so the rest of the code only ever reads the raw field name. The namespace
/// prefix below MUST match whatever the Auth0 Action emits.
///
/// Standard registered claims (`sub`, `iss`, `exp`, `aud`) are never namespaced by Auth0
/// and have no alias. See `docs/security/user-management-contracts.md` for the full
/// contract.
#[derive(Debug, serde::Deserialize)]
pub struct Claims {
    pub sub: Option<String>,
    #[serde(alias = "https://runtara.io/org_id")]
    pub org_id: Option<String>,
    /// Token identity, used as the key for the `token:revoked:{jti}` revocation denylist.
    #[serde(alias = "https://runtara.io/jti")]
    pub jti: Option<String>,
    #[serde(alias = "https://runtara.io/email")]
    pub email: Option<String>,
    #[serde(alias = "https://runtara.io/name")]
    pub name: Option<String>,
    /// Human-readable tenant identifier. Used for logging and the `/me` response only —
    /// `org_id` remains the tenant key everywhere internally.
    #[serde(alias = "https://runtara.io/tenant_slug")]
    pub tenant_slug: Option<String>,
    pub iss: Option<String>,
    pub exp: Option<u64>,
    pub aud: Option<Audience>,
}

/// The `aud` claim can be a single string or an array of strings
#[derive(Debug, serde::Deserialize)]
#[serde(untagged)]
pub enum Audience {
    Single(String),
    Multiple(Vec<String>),
}

/// Errors that can occur during JWT validation
#[derive(Debug, thiserror::Error)]
pub enum JwtError {
    #[error("Invalid token header: {0}")]
    InvalidHeader(String),

    #[error("Unknown signing key (kid: {0})")]
    UnknownKid(String),

    #[error("Token validation failed: {0}")]
    ValidationFailed(String),

    #[error("Missing org_id claim")]
    MissingOrgId,

    #[error("Missing jti claim")]
    MissingJti,
}

/// Decode JWT header to extract the `kid` (key ID).
pub fn extract_kid(token: &str) -> Result<String, JwtError> {
    let header = decode_header(token).map_err(|e| JwtError::InvalidHeader(e.to_string()))?;
    header
        .kid
        .ok_or_else(|| JwtError::InvalidHeader("Missing kid in token header".into()))
}

/// Validate and decode a JWT token using the provided decoding key and config.
/// Returns the validated claims.
pub fn validate_token(
    token: &str,
    key: &DecodingKey,
    config: &JwtConfig,
) -> Result<Claims, JwtError> {
    let mut validation = Validation::new(Algorithm::RS256);
    validation.set_issuer(&[&config.issuer]);

    if let Some(ref audience) = config.audience {
        validation.set_audience(&[audience]);
    } else {
        validation.validate_aud = false;
    }

    // exp is validated by default by jsonwebtoken

    let token_data = decode::<Claims>(token, key, &validation)
        .map_err(|e| JwtError::ValidationFailed(e.to_string()))?;

    // Ensure org_id is present
    if token_data.claims.org_id.is_none() {
        return Err(JwtError::MissingOrgId);
    }

    // jti is the revocation-denylist key; without it a token cannot be revoked. Once the
    // Auth0 Action emits jti on every token, require_jti is flipped on to reject tokens
    // that would otherwise be un-revokable.
    if config.require_jti && token_data.claims.jti.is_none() {
        return Err(JwtError::MissingJti);
    }

    Ok(token_data.claims)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn parse(value: serde_json::Value) -> Claims {
        serde_json::from_value(value).expect("claims should deserialize")
    }

    #[test]
    fn deserializes_raw_claims() {
        let claims = parse(json!({
            "sub": "auth0|abc",
            "org_id": "org_abc",
            "jti": "jti-1",
            "email": "user@acme.com",
            "name": "Ada",
            "tenant_slug": "acme",
        }));

        assert_eq!(claims.sub.as_deref(), Some("auth0|abc"));
        assert_eq!(claims.org_id.as_deref(), Some("org_abc"));
        assert_eq!(claims.jti.as_deref(), Some("jti-1"));
        assert_eq!(claims.email.as_deref(), Some("user@acme.com"));
        assert_eq!(claims.name.as_deref(), Some("Ada"));
        assert_eq!(claims.tenant_slug.as_deref(), Some("acme"));
    }

    #[test]
    fn normalizes_namespaced_claims() {
        // Auth0 may emit custom claims only under the namespaced key. They must land in the
        // same fields as the raw form. Keys are built from CLAIM_NAMESPACE so this fails if
        // the const and the `#[serde(alias = ...)]` literals ever diverge.
        let ns = CLAIM_NAMESPACE;
        let mut map = serde_json::Map::new();
        map.insert("sub".into(), json!("auth0|abc"));
        map.insert(format!("{ns}org_id"), json!("org_abc"));
        map.insert(format!("{ns}jti"), json!("jti-1"));
        map.insert(format!("{ns}email"), json!("user@acme.com"));
        map.insert(format!("{ns}name"), json!("Ada"));
        map.insert(format!("{ns}tenant_slug"), json!("acme"));
        let claims: Claims = serde_json::from_value(serde_json::Value::Object(map))
            .expect("namespaced claims should deserialize");

        assert_eq!(claims.org_id.as_deref(), Some("org_abc"));
        assert_eq!(claims.jti.as_deref(), Some("jti-1"));
        assert_eq!(claims.email.as_deref(), Some("user@acme.com"));
        assert_eq!(claims.name.as_deref(), Some("Ada"));
        assert_eq!(claims.tenant_slug.as_deref(), Some("acme"));
    }

    #[test]
    fn optional_claims_default_to_none() {
        let claims = parse(json!({ "org_id": "org_abc" }));
        assert_eq!(claims.org_id.as_deref(), Some("org_abc"));
        assert!(claims.jti.is_none());
        assert!(claims.email.is_none());
        assert!(claims.tenant_slug.is_none());
    }
}
