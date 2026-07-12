//! OAuth2 Authorization Code flow service
//!
//! Handles authorization URL generation, code-to-token exchange, and
//! token storage in connection parameters.

use std::sync::Arc;

use crate::crypto::CredentialCipher;
use crate::repository::connections::ConnectionRepository;
use crate::repository::oauth::OAuthRepository;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use rand::RngCore;
use runtara_agents::registry::find_connection_type;
use runtara_dsl::agent_meta::OAuthConfig;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::PgPool;

#[derive(Debug)]
pub enum OAuthError {
    /// Connection not found or not owned by tenant
    ConnectionNotFound,
    /// Integration does not support OAuth
    NotOAuthIntegration(String),
    /// Missing required connection parameter
    MissingParameter(String),
    /// State token not found or expired
    InvalidState,
    /// Token exchange failed
    TokenExchangeFailed(String),
    /// Internal error
    Internal(String),
}

impl std::fmt::Display for OAuthError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OAuthError::ConnectionNotFound => write!(f, "Connection not found"),
            OAuthError::NotOAuthIntegration(id) => {
                write!(f, "Integration '{}' does not support OAuth", id)
            }
            OAuthError::MissingParameter(p) => write!(f, "Missing connection parameter: {}", p),
            OAuthError::InvalidState => write!(f, "Invalid or expired OAuth state"),
            OAuthError::TokenExchangeFailed(msg) => write!(f, "Token exchange failed: {}", msg),
            OAuthError::Internal(msg) => write!(f, "Internal error: {}", msg),
        }
    }
}

pub struct OAuthService {
    oauth_repo: OAuthRepository,
    connection_repo: ConnectionRepository,
    public_base_url: String,
}

impl OAuthService {
    pub fn new(pool: PgPool, cipher: Arc<dyn CredentialCipher>, public_base_url: String) -> Self {
        Self {
            oauth_repo: OAuthRepository::new(pool.clone()),
            connection_repo: ConnectionRepository::new(pool, cipher),
            public_base_url,
        }
    }

    /// Generate an OAuth2 authorization URL for a connection.
    ///
    /// Creates a state token, stores it in the DB, and returns the full
    /// authorization URL the user should be redirected to.
    pub async fn generate_authorization_url(
        &self,
        connection_id: &str,
        tenant_id: &str,
    ) -> Result<String, OAuthError> {
        // Load the connection with parameters
        let conn = self
            .connection_repo
            .get_with_parameters(connection_id, tenant_id)
            .await
            .map_err(|e| OAuthError::Internal(e.to_string()))?
            .ok_or(OAuthError::ConnectionNotFound)?;

        let integration_id = conn
            .integration_id
            .as_deref()
            .ok_or(OAuthError::NotOAuthIntegration("none".to_string()))?;

        // Look up OAuthConfig from the connection type metadata
        let meta = find_connection_type(integration_id)
            .ok_or_else(|| OAuthError::NotOAuthIntegration(integration_id.to_string()))?;

        let oauth_config = meta
            .oauth_config
            .ok_or_else(|| OAuthError::NotOAuthIntegration(integration_id.to_string()))?;

        let params = conn.connection_parameters.unwrap_or(json!({}));

        // Effective config: static descriptor for curated providers; connection
        // params only for params-driven generic types (see provider_auth).
        let effective =
            crate::auth::provider_auth::resolve_effective_oauth_config(oauth_config, &params);
        if effective.auth_url.is_empty() {
            return Err(OAuthError::MissingParameter("auth_url".to_string()));
        }

        let client_id = params["client_id"]
            .as_str()
            .ok_or(OAuthError::MissingParameter("client_id".to_string()))?;

        let scopes = params["scopes"]
            .as_str()
            .unwrap_or(oauth_config.default_scopes);

        // Generate a cryptographically random state token
        let state = generate_state();

        // Build redirect URI
        let redirect_uri = format!(
            "{}/api/oauth/{}/callback",
            self.public_base_url.trim_end_matches('/'),
            tenant_id
        );

        // PKCE (RFC 7636) when the provider descriptor requires it: attach an S256
        // code_challenge to the authorize URL and stash the verifier on the state row.
        let (code_verifier, pkce_query) = if effective.pkce_required {
            let (verifier, challenge) = generate_pkce();
            let q = format!(
                "&code_challenge={}&code_challenge_method=S256",
                urlencoding::encode(&challenge)
            );
            (Some(verifier), q)
        } else {
            (None, String::new())
        };

        // Store state in DB
        self.oauth_repo
            .create_state(
                &state,
                tenant_id,
                connection_id,
                integration_id,
                &redirect_uri,
                code_verifier.as_deref(),
            )
            .await
            .map_err(|e| OAuthError::Internal(e.to_string()))?;

        // Build authorization URL. `response_type=code` is mandatory for the
        // authorization-code grant (RFC 6749 §4.1.1); strict providers (Intuit)
        // reject the request outright without it.
        let auth_url = format!(
            "{}?response_type=code&client_id={}&redirect_uri={}&scope={}&state={}{}",
            effective.auth_url,
            urlencoding::encode(client_id),
            urlencoding::encode(&redirect_uri),
            urlencoding::encode(scopes),
            urlencoding::encode(&state),
            pkce_query,
        );

        Ok(auth_url)
    }

    /// Handle the OAuth2 callback: validate state, exchange code for tokens,
    /// capture any provider-specific callback params, and store everything on the
    /// connection. `callback_params` is the full parsed callback query string, from
    /// which the descriptor's declared `extra_callback_params` (e.g. Intuit `realmId`)
    /// are captured.
    pub async fn handle_callback(
        &self,
        state: &str,
        code: &str,
        callback_params: &std::collections::HashMap<String, String>,
    ) -> Result<String, OAuthError> {
        // Atomically consume the state token
        let state_row = self
            .oauth_repo
            .get_and_delete_state(state)
            .await
            .map_err(|e| OAuthError::Internal(e.to_string()))?
            .ok_or(OAuthError::InvalidState)?;

        // Look up OAuthConfig
        let meta = find_connection_type(&state_row.integration_id)
            .ok_or_else(|| OAuthError::NotOAuthIntegration(state_row.integration_id.clone()))?;
        let oauth_config = meta
            .oauth_config
            .ok_or_else(|| OAuthError::NotOAuthIntegration(state_row.integration_id.clone()))?;

        // Load connection to get client_id + client_secret
        let conn = self
            .connection_repo
            .get_with_parameters(&state_row.connection_id, &state_row.tenant_id)
            .await
            .map_err(|e| OAuthError::Internal(e.to_string()))?
            .ok_or(OAuthError::ConnectionNotFound)?;

        let params = conn.connection_parameters.unwrap_or(json!({}));
        let client_id = params["client_id"]
            .as_str()
            .ok_or(OAuthError::MissingParameter("client_id".to_string()))?;
        let client_secret = params["client_secret"]
            .as_str()
            .ok_or(OAuthError::MissingParameter("client_secret".to_string()))?;

        // Exchange authorization code for tokens (sending the PKCE verifier if the
        // authorize step generated one).
        let token_response = exchange_code(
            oauth_config,
            &params,
            code,
            client_id,
            client_secret,
            &state_row.redirect_uri,
            state_row.code_verifier.as_deref(),
        )
        .await?;

        // Merge tokens into connection_parameters
        let mut updated_params = params.clone();
        if let Some(obj) = updated_params.as_object_mut() {
            if let Some(at) = token_response.get("access_token") {
                obj.insert("access_token".to_string(), at.clone());
            }
            if let Some(rt) = token_response.get("refresh_token") {
                obj.insert("refresh_token".to_string(), rt.clone());
            }
            if let Some(expires_in) = token_response["expires_in"].as_u64() {
                let expires_at = chrono::Utc::now() + chrono::Duration::seconds(expires_in as i64);
                obj.insert(
                    "token_expires_at".to_string(),
                    Value::String(expires_at.to_rfc3339()),
                );
            }
            // Stamp the successful authorization time for the grant-health card.
            obj.insert(
                "authorized_at".to_string(),
                Value::String(chrono::Utc::now().to_rfc3339()),
            );
            // Capture provider-specific callback params declared by the descriptor
            // (e.g. Intuit returns `realmId`, needed for every QuickBooks API path).
            merge_extra_callback_params(obj, oauth_config, callback_params)?;
        }

        // Update connection: set parameters + status = ACTIVE
        self.connection_repo
            .update_parameters_and_status(
                &state_row.connection_id,
                &state_row.tenant_id,
                &updated_params,
                "ACTIVE",
            )
            .await
            .map_err(|e| OAuthError::Internal(e.to_string()))?;

        Ok(state_row.connection_id)
    }
}

/// Merge the descriptor's declared extra callback params (e.g. Intuit `realmId`) from
/// the callback query into the connection params object. Errors if a required one is
/// missing.
fn merge_extra_callback_params(
    obj: &mut serde_json::Map<String, Value>,
    oauth_config: &OAuthConfig,
    callback_params: &std::collections::HashMap<String, String>,
) -> Result<(), OAuthError> {
    for ecp in oauth_config.extra_callback_params {
        match callback_params.get(ecp.query_name) {
            Some(val) => {
                obj.insert(ecp.param_name.to_string(), Value::String(val.clone()));
            }
            None if ecp.required => {
                return Err(OAuthError::MissingParameter(ecp.query_name.to_string()));
            }
            None => {}
        }
    }
    Ok(())
}

/// Generate a 32-byte hex-encoded random state token.
fn generate_state() -> String {
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    hex::encode(bytes)
}

/// Generate a PKCE (RFC 7636) `(code_verifier, code_challenge)` pair. The verifier
/// is 32 bytes of CSPRNG entropy, base64url-encoded (43 chars, all unreserved); the
/// challenge is the base64url of its SHA-256 (the `S256` method).
fn generate_pkce() -> (String, String) {
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    let verifier = URL_SAFE_NO_PAD.encode(bytes);
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    (verifier, challenge)
}

/// Exchange an authorization code for access + refresh tokens.
async fn exchange_code(
    oauth_config: &OAuthConfig,
    params: &Value,
    code: &str,
    client_id: &str,
    client_secret: &str,
    redirect_uri: &str,
    code_verifier: Option<&str>,
) -> Result<Value, OAuthError> {
    let effective =
        crate::auth::provider_auth::resolve_effective_oauth_config(oauth_config, params);
    if effective.token_url.is_empty() {
        return Err(OAuthError::MissingParameter("token_url".to_string()));
    }
    // Hardened egress: no redirect following (a 3xx must not carry the client
    // secret/Basic header to another host), DNS-guarded resolver (private-host
    // token endpoints rejected at connect time).
    let client = crate::net::shared_hardened_client();

    // Present credentials per the provider's token-endpoint style (Intuit requires
    // HTTP Basic; the OAuth2 default is credentials in the body).
    let mut grant_fields = vec![
        ("grant_type".to_string(), "authorization_code".to_string()),
        ("code".to_string(), code.to_string()),
        ("redirect_uri".to_string(), redirect_uri.to_string()),
    ];
    // PKCE: prove possession of the verifier that produced the authorize challenge.
    if let Some(verifier) = code_verifier {
        grant_fields.push(("code_verifier".to_string(), verifier.to_string()));
    }
    let (basic_auth, body) = crate::auth::token_cache::token_request_parts(
        effective.token_endpoint_auth,
        grant_fields,
        client_id,
        client_secret,
    );

    let mut request = client
        .post(&effective.token_url)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(body)
        // Bound a hostile/slow token endpoint, matching the mint/refresh/revoke
        // egress calls (token_cache.rs:269/304, provider_auth.rs revoke).
        .timeout(std::time::Duration::from_secs(10));
    if let Some(header) = basic_auth {
        request = request.header("Authorization", header);
    }

    let response = request
        .send()
        .await
        .map_err(|e| OAuthError::TokenExchangeFailed(e.to_string()))?;

    let status = response.status();
    let body: Value = response
        .json()
        .await
        .map_err(|e| OAuthError::TokenExchangeFailed(format!("Failed to parse response: {}", e)))?;

    // NEVER echo the full token-endpoint response body upward: it reaches the
    // popup page + postMessage and is persisted to the (plaintext) event store,
    // and a provider's error body can echo back the client_secret/tokens it was
    // sent. Log it host-side; surface only status + the standard OAuth fields.
    // Mirrors parse_token_response() in token_cache.rs.
    if !status.is_success() {
        tracing::debug!(status = %status, body = %body, "oauth code exchange returned non-success");
        let code = body["error"].as_str().unwrap_or("unknown_error");
        let desc = body["error_description"].as_str().unwrap_or("");
        return Err(OAuthError::TokenExchangeFailed(
            format!("token endpoint returned {status}: {code} {desc}")
                .trim()
                .to_string(),
        ));
    }

    if body.get("access_token").is_none() {
        tracing::debug!(body = %body, "oauth code exchange response missing access_token");
        return Err(OAuthError::TokenExchangeFailed(
            "token endpoint response missing access_token".to_string(),
        ));
    }

    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtara_dsl::agent_meta::{ExtraCallbackParam, TokenEndpointAuth};
    use std::collections::HashMap;

    fn cfg_with(extra: &'static [ExtraCallbackParam]) -> OAuthConfig {
        OAuthConfig {
            auth_url: "",
            token_url: "",
            default_scopes: "",
            token_endpoint_auth: TokenEndpointAuth::FormBody,
            refresh_token_rotates: false,
            base_url: "",
            sandbox_base_url: "",
            base_url_path_template: "",
            extra_callback_params: extra,
            reauth_on_error_codes: &[],
            revocation_endpoint: "",
            pkce_required: false,
            params_driven: false,
        }
    }

    static REALM: &[ExtraCallbackParam] = &[ExtraCallbackParam {
        query_name: "realmId",
        param_name: "realm_id",
        required: true,
    }];

    #[test]
    fn captures_declared_callback_param_under_stored_key() {
        let cfg = cfg_with(REALM);
        let mut obj = serde_json::Map::new();
        let mut q = HashMap::new();
        q.insert("realmId".to_string(), "12345".to_string());
        merge_extra_callback_params(&mut obj, &cfg, &q).unwrap();
        // Captured under the descriptor's param_name (realm_id), from the query name (realmId).
        assert_eq!(obj.get("realm_id").and_then(|v| v.as_str()), Some("12345"));
    }

    #[test]
    fn errors_when_required_callback_param_missing() {
        let cfg = cfg_with(REALM);
        let mut obj = serde_json::Map::new();
        let q = HashMap::new();
        assert!(matches!(
            merge_extra_callback_params(&mut obj, &cfg, &q),
            Err(OAuthError::MissingParameter(_))
        ));
    }

    #[test]
    fn pkce_challenge_is_s256_of_verifier() {
        let (verifier, challenge) = generate_pkce();
        // Verifier: 43 chars, all PKCE-unreserved (base64url of 32 bytes, no padding).
        assert_eq!(verifier.len(), 43);
        assert!(
            verifier
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        );
        // Challenge must equal base64url(SHA-256(verifier)) — the S256 method.
        let expected = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        assert_eq!(challenge, expected);
        assert!(
            !challenge.contains('='),
            "challenge must be unpadded base64url"
        );
    }

    #[test]
    fn ignores_missing_optional_callback_param() {
        static OPT: &[ExtraCallbackParam] = &[ExtraCallbackParam {
            query_name: "foo",
            param_name: "foo",
            required: false,
        }];
        let cfg = cfg_with(OPT);
        let mut obj = serde_json::Map::new();
        merge_extra_callback_params(&mut obj, &cfg, &HashMap::new()).unwrap();
        assert!(obj.is_empty());
    }
}
