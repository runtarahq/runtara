//! OAuth2 Authorization Code flow service
//!
//! Handles authorization URL generation, code-to-token exchange, and
//! token storage in connection parameters.

use std::sync::Arc;

use crate::crypto::CredentialCipher;
use crate::repository::connections::ConnectionRepository;
use crate::repository::oauth::OAuthRepository;
use crate::service::oauth_flow::{self, FlowBinding, FlowRecord, FlowStore};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use rand::RngCore;
use runtara_agents::registry::find_connection_type;
use runtara_dsl::agent_meta::OAuthConfig;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::PgPool;

/// The connection parameter naming the redirect URI a connection uses, so it
/// stays the one registered with the provider: `tenantless` for
/// `/api/oauth/callback`, anything else (or none, as on connections made
/// before the tenant-less callback) for `/api/oauth/{tenant}/callback`.
pub const OAUTH_CALLBACK_PARAM: &str = "oauth_callback";
pub const OAUTH_CALLBACK_TENANTLESS: &str = "tenantless";

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
    /// A flow started in a browser came back without that browser's cookie.
    BrowserMismatch,
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
            OAuthError::BrowserMismatch => write!(
                f,
                "The authorization was not finished in the browser that started it"
            ),
            OAuthError::TokenExchangeFailed(msg) => write!(f, "Token exchange failed: {}", msg),
            OAuthError::Internal(msg) => write!(f, "Internal error: {}", msg),
        }
    }
}

/// How a flow being started is to be bound to whoever starts it.
#[derive(Debug, Clone)]
pub enum StartBinding {
    /// In the console: a cookie the browser must bring back.
    Browser,
}

/// A flow just started.
#[derive(Debug)]
pub struct StartedFlow {
    /// Where to send the user.
    pub authorization_url: String,
    /// The flow's state, as in the authorization URL.
    pub state: String,
    /// The redirect URI the provider will send the user to.
    pub redirect_uri: String,
    /// For a browser flow, the nonce its cookie must carry.
    pub browser_nonce: Option<String>,
}

/// A flow completed.
#[derive(Debug, PartialEq, Eq)]
pub struct CompletedFlow {
    pub connection_id: String,
}

/// A redeemed flow: from Valkey, or from the `oauth_state` table for a flow
/// started before flows moved to Valkey (no binding then).
struct RedeemedFlow {
    connection_id: String,
    integration_id: String,
    redirect_uri: String,
    code_verifier: Option<String>,
    binding: Option<FlowBinding>,
}

pub struct OAuthService {
    /// Flows started before flows moved to Valkey, read once at the callback
    /// during the release that moves them.
    oauth_repo: OAuthRepository,
    connection_repo: ConnectionRepository,
    public_base_url: String,
    flows: Option<FlowStore>,
    tenantless_callback: bool,
}

impl OAuthService {
    pub fn new(pool: PgPool, cipher: Arc<dyn CredentialCipher>, public_base_url: String) -> Self {
        Self {
            oauth_repo: OAuthRepository::new(pool.clone()),
            connection_repo: ConnectionRepository::new(pool, cipher),
            public_base_url,
            flows: None,
            tenantless_callback: true,
        }
    }

    /// The service as a request's state configures it.
    pub fn from_state(state: &crate::config::ConnectionsState) -> Self {
        let service = Self::new(
            state.db_pool.clone(),
            state.cipher.clone(),
            state.public_base_url.clone(),
        )
        .with_tenantless_callback(state.oauth.tenantless_callback);
        match &state.redis_manager {
            Some(valkey) => service.with_flows(valkey.clone()),
            None => service,
        }
    }

    /// Keep flows in progress in `valkey`. Without it no flow can start.
    pub fn with_flows(mut self, valkey: redis::aio::ConnectionManager) -> Self {
        self.flows = Some(FlowStore::new(valkey));
        self
    }

    /// Whether connections marked `tenantless` get the callback that names no
    /// tenant. Off, every connection uses its tenant's callback.
    pub fn with_tenantless_callback(mut self, enabled: bool) -> Self {
        self.tenantless_callback = enabled;
        self
    }

    fn flows(&self) -> Result<&FlowStore, OAuthError> {
        self.flows.as_ref().ok_or_else(|| {
            OAuthError::Internal("OAuth flows need Valkey, and none is configured".to_string())
        })
    }

    /// Whether the public base URL is https, so cookies can be `Secure`.
    pub fn secure_cookies(&self) -> bool {
        self.public_base_url.starts_with("https://")
    }

    /// The redirect URI a connection with `params` uses in `tenant`.
    pub fn redirect_uri(&self, tenant_id: &str, params: &Value) -> String {
        let base = self.public_base_url.trim_end_matches('/');
        let tenantless = params[OAUTH_CALLBACK_PARAM].as_str() == Some(OAUTH_CALLBACK_TENANTLESS);
        if tenantless && self.tenantless_callback {
            format!("{base}/api/oauth/callback")
        } else {
            format!("{base}/api/oauth/{tenant_id}/callback")
        }
    }

    /// Start an authorization-code flow for a connection: the authorization
    /// URL, with the flow's record stored in Valkey.
    pub async fn start(
        &self,
        connection_id: &str,
        tenant_id: &str,
        binding: StartBinding,
    ) -> Result<StartedFlow, OAuthError> {
        let flows = self.flows()?;
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
        let redirect_uri = self.redirect_uri(tenant_id, &params);

        // PKCE (RFC 7636) when the provider descriptor requires it: attach an S256
        // code_challenge to the authorize URL and keep the verifier in the flow.
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

        let (binding, browser_nonce) = match binding {
            StartBinding::Browser => {
                let nonce = oauth_flow::new_nonce();
                (
                    FlowBinding::Browser {
                        nonce_hash: oauth_flow::sha256_hex(&nonce),
                    },
                    Some(nonce),
                )
            }
        };
        flows
            .put(
                &state,
                &FlowRecord {
                    tenant_id: tenant_id.to_string(),
                    connection_id: connection_id.to_string(),
                    integration_id: integration_id.to_string(),
                    redirect_uri: redirect_uri.clone(),
                    code_verifier,
                    binding,
                },
            )
            .await
            .map_err(|e| OAuthError::Internal(e.to_string()))?;

        // Build authorization URL. `response_type=code` is mandatory for the
        // authorization-code grant (RFC 6749 §4.1.1); strict providers (Intuit)
        // reject the request outright without it.
        let authorization_url = format!(
            "{}?response_type=code&client_id={}&redirect_uri={}&scope={}&state={}{}",
            effective.auth_url,
            urlencoding::encode(client_id),
            urlencoding::encode(&redirect_uri),
            urlencoding::encode(scopes),
            urlencoding::encode(&state),
            pkce_query,
        );

        Ok(StartedFlow {
            authorization_url,
            state,
            redirect_uri,
            browser_nonce,
        })
    }

    /// The tenant of the flow `state` names, if it is in progress. This only
    /// finds the tenant; the flow is verified when it is redeemed there.
    pub async fn tenant_of(&self, state: &str) -> Result<Option<String>, OAuthError> {
        self.flows()?
            .tenant_of(state)
            .await
            .map_err(|e| OAuthError::Internal(e.to_string()))
    }

    /// Redeem the flow `state` names in `tenant`, once.
    async fn redeem(&self, tenant_id: &str, state: &str) -> Result<RedeemedFlow, OAuthError> {
        if let Some(record) = self
            .flows()?
            .take(tenant_id, state)
            .await
            .map_err(|e| OAuthError::Internal(e.to_string()))?
        {
            return Ok(RedeemedFlow {
                connection_id: record.connection_id,
                integration_id: record.integration_id,
                redirect_uri: record.redirect_uri,
                code_verifier: record.code_verifier,
                binding: Some(record.binding),
            });
        }
        // A flow started before flows moved to Valkey, by a process from
        // before the change (a handover runs both). Consumed either way, and
        // redeemed only for the tenant it was issued for.
        let row = self
            .oauth_repo
            .get_and_delete_state(state)
            .await
            .map_err(|e| OAuthError::Internal(e.to_string()))?
            .ok_or(OAuthError::InvalidState)?;
        if row.tenant_id != tenant_id {
            return Err(OAuthError::InvalidState);
        }
        Ok(RedeemedFlow {
            connection_id: row.connection_id,
            integration_id: row.integration_id,
            redirect_uri: row.redirect_uri,
            code_verifier: row.code_verifier,
            binding: None,
        })
    }

    /// Complete the flow `state` names in `tenant`: check its binding,
    /// exchange the code for tokens, capture any provider-specific callback
    /// params, and store everything on the connection. `callback_params` is
    /// the full parsed callback query string, from which the descriptor's
    /// declared `extra_callback_params` (e.g. Intuit `realmId`) are captured.
    /// `browser_nonce` is the flow's cookie, if the request carried it.
    pub async fn complete(
        &self,
        tenant_id: &str,
        state: &str,
        code: &str,
        callback_params: &std::collections::HashMap<String, String>,
        browser_nonce: Option<&str>,
    ) -> Result<CompletedFlow, OAuthError> {
        let flow = self.redeem(tenant_id, state).await?;
        match &flow.binding {
            Some(FlowBinding::Browser { nonce_hash }) => {
                if !oauth_flow::nonce_matches(nonce_hash, browser_nonce) {
                    return Err(OAuthError::BrowserMismatch);
                }
            }
            // Not issued until flows started without a browser exist.
            Some(FlowBinding::Confirm { .. }) => return Err(OAuthError::InvalidState),
            // Started before flows were bound; accepted for this release only.
            None => {}
        }

        // Look up OAuthConfig
        let meta = find_connection_type(&flow.integration_id)
            .ok_or_else(|| OAuthError::NotOAuthIntegration(flow.integration_id.clone()))?;
        let oauth_config = meta
            .oauth_config
            .ok_or_else(|| OAuthError::NotOAuthIntegration(flow.integration_id.clone()))?;

        // Load connection to get client_id + client_secret
        let conn = self
            .connection_repo
            .get_with_parameters(&flow.connection_id, tenant_id)
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
            &flow.redirect_uri,
            flow.code_verifier.as_deref(),
        )
        .await?;

        let granted = granted_fields(&token_response, oauth_config, callback_params)?;
        let mut updated_params = params.clone();
        if let Some(obj) = updated_params.as_object_mut() {
            obj.extend(granted);
        }

        // Update connection: set parameters + status = ACTIVE
        self.connection_repo
            .update_parameters_and_status(&flow.connection_id, tenant_id, &updated_params, "ACTIVE")
            .await
            .map_err(|e| OAuthError::Internal(e.to_string()))?;

        Ok(CompletedFlow {
            connection_id: flow.connection_id,
        })
    }
}

/// What a completed flow adds to a connection: the tokens, their expiry, the
/// time of authorization, and the descriptor's extra callback params.
fn granted_fields(
    token_response: &Value,
    oauth_config: &OAuthConfig,
    callback_params: &std::collections::HashMap<String, String>,
) -> Result<serde_json::Map<String, Value>, OAuthError> {
    let mut obj = serde_json::Map::new();
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
    merge_extra_callback_params(&mut obj, oauth_config, callback_params)?;
    Ok(obj)
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
    // Re-check the endpoint before the code and client secret are sent: a
    // stored URL may predate save-time validation. Nothing is sent on failure.
    crate::net::validate_credentialed_endpoint(&effective.token_url).map_err(|reason| {
        OAuthError::TokenExchangeFailed(format!(
            "refusing to send credentials to the token endpoint: {reason}"
        ))
    })?;
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

    fn params_driven_cfg() -> OAuthConfig {
        OAuthConfig {
            params_driven: true,
            ..cfg_with(&[])
        }
    }

    async fn provider(route: &str, expected_calls: u64) -> wiremock::MockServer {
        use wiremock::matchers::{method, path};
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(method("POST"))
            .and(path(route))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(
                serde_json::json!({"access_token": "at-1", "refresh_token": "rt-1"}),
            ))
            .expect(expected_calls)
            .mount(&server)
            .await;
        server
    }

    async fn exchange(token_url: String) -> Result<Value, OAuthError> {
        exchange_code(
            &params_driven_cfg(),
            &serde_json::json!({ "token_url": token_url }),
            "code-1",
            "cid",
            "csec",
            "https://runtara.example/callback",
            None,
        )
        .await
    }

    #[tokio::test]
    async fn code_exchange_refuses_plain_http_token_endpoint_before_sending() {
        let server = provider("/token", 0).await;
        for token_url in [
            "http://auth.example.com/token".to_string(),
            format!("{}/token", server.uri()),
        ] {
            let Err(OAuthError::TokenExchangeFailed(message)) = exchange(token_url.clone()).await
            else {
                panic!("{token_url}: an http token endpoint must be refused")
            };
            assert!(message.contains("https"), "{token_url}: {message}");
        }
    }

    #[tokio::test]
    async fn code_exchange_sends_to_an_allowlisted_http_token_endpoint() {
        let _loopback = crate::net::test_allowlist::allow("127.0.0.1");
        let server = provider("/token", 1).await;
        let body = exchange(format!("{}/token", server.uri()))
            .await
            .unwrap_or_else(|err| panic!("allowlisted endpoint must be used: {err}"));
        assert_eq!(body["access_token"], "at-1");
    }

    #[tokio::test]
    async fn revocation_refuses_plain_http_endpoint_before_sending() {
        let server = provider("/revoke", 0).await;
        let params = serde_json::json!({
            "revocation_url": format!("{}/revoke", server.uri()),
            "refresh_token": "rt-1",
        });
        let err = crate::auth::provider_auth::revoke_oauth_token(
            crate::net::shared_hardened_client(),
            &params_driven_cfg(),
            &params,
        )
        .await
        .expect_err("an http revocation endpoint must be refused");
        assert!(err.contains("https"), "{err}");
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
