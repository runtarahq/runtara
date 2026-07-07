//! OAuth2 Authorization Code flow service
//!
//! Handles authorization URL generation, code-to-token exchange, and
//! token storage in connection parameters.

use std::sync::Arc;

use crate::crypto::CredentialCipher;
use crate::repository::connections::ConnectionRepository;
use crate::repository::oauth::OAuthRepository;
use rand::RngCore;
use runtara_agents::registry::find_connection_type;
use runtara_dsl::agent_meta::OAuthConfig;
use serde_json::{Value, json};
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

        // Store state in DB
        self.oauth_repo
            .create_state(
                &state,
                tenant_id,
                connection_id,
                integration_id,
                &redirect_uri,
            )
            .await
            .map_err(|e| OAuthError::Internal(e.to_string()))?;

        // Build authorization URL
        let auth_url = format!(
            "{}?client_id={}&redirect_uri={}&scope={}&state={}",
            oauth_config.auth_url,
            urlencoding::encode(client_id),
            urlencoding::encode(&redirect_uri),
            urlencoding::encode(scopes),
            urlencoding::encode(&state),
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

        // Exchange authorization code for tokens
        let token_response = exchange_code(
            oauth_config,
            code,
            client_id,
            client_secret,
            &state_row.redirect_uri,
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

/// Exchange an authorization code for access + refresh tokens.
async fn exchange_code(
    oauth_config: &OAuthConfig,
    code: &str,
    client_id: &str,
    client_secret: &str,
    redirect_uri: &str,
) -> Result<Value, OAuthError> {
    let client = reqwest::Client::new();

    // Present credentials per the provider's token-endpoint style (Intuit requires
    // HTTP Basic; the OAuth2 default is credentials in the body).
    let (basic_auth, body) = crate::auth::token_cache::token_request_parts(
        oauth_config.token_endpoint_auth,
        vec![
            ("grant_type".to_string(), "authorization_code".to_string()),
            ("code".to_string(), code.to_string()),
            ("redirect_uri".to_string(), redirect_uri.to_string()),
        ],
        client_id,
        client_secret,
    );

    let mut request = client
        .post(oauth_config.token_url)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(body);
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

    if !status.is_success() {
        return Err(OAuthError::TokenExchangeFailed(format!(
            "HTTP {}: {}",
            status, body
        )));
    }

    if body.get("access_token").is_none() {
        return Err(OAuthError::TokenExchangeFailed(format!(
            "Response missing access_token: {}",
            body
        )));
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
