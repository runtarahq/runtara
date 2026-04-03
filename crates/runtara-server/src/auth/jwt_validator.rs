use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header};

use super::JwtConfig;

/// JWT claims expected in the token payload
#[derive(Debug, serde::Deserialize)]
pub struct Claims {
    pub sub: Option<String>,
    pub org_id: Option<String>,
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

    Ok(token_data.claims)
}
