//! OAuth2 Authorization Code flow service
//!
//! Handles authorization URL generation, code-to-token exchange, and
//! token storage in connection parameters.

use std::sync::Arc;

use crate::crypto::CredentialCipher;
use crate::oauth_apps::{self, OAuthApps};
use crate::repository::connections::ConnectionRepository;
use crate::repository::oauth::OAuthRepository;
use crate::service::oauth_flow::{self, FlowBinding, FlowRecord, FlowStore};
use crate::types::ConnectionStatus;
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
    /// The connection's platform OAuth app cannot be used.
    App(String),
    /// The connection has no authorization waiting for confirmation.
    NotPending,
    /// The authorization waited too long for confirmation, and was dropped.
    ConfirmationExpired,
    /// The authorization waiting for confirmation is not the one named: a
    /// later flow replaced it, or it was already confirmed or rejected.
    PendingChanged,
    /// Token exchange failed
    TokenExchangeFailed(String),
    /// The provider issued tokens, but which of its accounts they were
    /// authorized for could not be told (Xero names its organisation only
    /// through its connections endpoint).
    AccountUnresolved(String),
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
            OAuthError::App(msg) => write!(f, "{msg}"),
            OAuthError::NotPending => {
                write!(
                    f,
                    "The connection has no authorization waiting for confirmation"
                )
            }
            OAuthError::PendingChanged => write!(
                f,
                "The authorization waiting for confirmation has changed; review it again"
            ),
            OAuthError::ConfirmationExpired => write!(
                f,
                "The authorization waited too long for confirmation; start it again"
            ),
            OAuthError::TokenExchangeFailed(msg) => write!(f, "Token exchange failed: {}", msg),
            OAuthError::AccountUnresolved(msg) => write!(f, "{msg}"),
            OAuthError::Internal(msg) => write!(f, "Internal error: {}", msg),
        }
    }
}

/// How a flow being started is to be bound to whoever starts it.
#[derive(Debug, Clone)]
pub enum StartBinding {
    /// In the console: a cookie the browser must bring back.
    Browser,
    /// Without a browser (an API client or an MCP tool handing someone a
    /// link): the connection waits for a member's confirmation. `started_by`
    /// is the caller, for the audit trail and the confirmation screen.
    Confirm { started_by: String },
}

/// How long a connection authorized by a flow started without a browser
/// waits for a member's confirmation, from the callback.
pub const PENDING_CONFIRMATION_SECONDS: i64 = 15 * 60;

/// The connection parameter holding what a flow started without a browser
/// obtained, until a member confirms it: the tokens (sealed with the rest of
/// the parameters, and never where a request would use them), the provider
/// account, who started the flow, and until when it may be confirmed.
pub const PENDING_OAUTH_PARAM: &str = "pending_oauth";

/// What a pending connection shows whoever is to confirm it. No secrets.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct PendingConfirmation {
    /// Names this authorization: confirming or rejecting it names it, so a
    /// later flow that replaced it is never confirmed unseen.
    pub id: String,
    /// The provider account connected, when the provider says which.
    pub account: Option<Value>,
    /// Who started the flow (`channel:id`).
    pub started_by: String,
    /// When the provider sent the user back.
    pub authorized_at: Option<String>,
    /// After this, the connection can no longer be confirmed.
    pub pending_until: String,
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
    /// The connection waits for a member's confirmation.
    pub pending_confirmation: bool,
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
    apps: Option<Arc<dyn OAuthApps>>,
    /// seal, if this deployment seals: the tokens a flow brings are sealed
    /// as they are stored.
    sealed: Arc<crate::sealed::SealedCredentials>,
}

impl OAuthService {
    /// A service that seals the tokens flows bring with `sealed`.
    pub fn new(
        pool: PgPool,
        cipher: Arc<dyn CredentialCipher>,
        public_base_url: String,
        sealed: Arc<crate::sealed::SealedCredentials>,
    ) -> Self {
        Self {
            oauth_repo: OAuthRepository::new(pool.clone()),
            connection_repo: ConnectionRepository::new(pool, cipher),
            public_base_url,
            flows: None,
            tenantless_callback: true,
            apps: None,
            sealed,
        }
    }

    /// A connection's parameters as they are stored: with its tokens sealed,
    /// if this deployment seals and the type's tokens can be. `stored` is
    /// what the connection holds now.
    async fn with_sealed_tokens(
        &self,
        tenant_id: &str,
        connection_id: &str,
        integration_id: &str,
        params: Value,
        stored: &Value,
    ) -> Result<Value, OAuthError> {
        let sealed = &self.sealed;
        // Tokens seal sealed as they came from the provider are the
        // connection's to hold, as if it held them already: they are kept if
        // their policy is the one the type calls for.
        let mut held = stored.clone();
        for token in [
            crate::sealed::policy::ACCESS_TOKEN_FIELD,
            crate::sealed::policy::REFRESH_TOKEN_FIELD,
        ] {
            if crate::sealed::is_sealed(&params[token]) {
                held[token] = params[token].clone();
            }
        }
        match sealed
            .seal_parameters(
                tenant_id,
                connection_id,
                integration_id,
                &params,
                Some(&held),
            )
            .await
        {
            Ok(Some(sealed)) => Ok(sealed),
            Ok(None) => Ok(params),
            Err(error) => Err(OAuthError::Internal(error.to_string())),
        }
    }

    /// Resolve connections' platform OAuth apps from `apps`.
    pub fn with_apps(mut self, apps: Option<Arc<dyn OAuthApps>>) -> Self {
        self.apps = apps;
        self
    }

    /// The platform app a connection's `params` name, for `tenant`.
    fn app_for(
        &self,
        tenant_id: &str,
        integration_id: &str,
        params: &Value,
    ) -> Result<Option<Arc<oauth_apps::OAuthAppCredentials>>, OAuthError> {
        oauth_apps::app_for(self.apps.as_deref(), tenant_id, integration_id, params)
            .map_err(OAuthError::App)
    }

    /// The service as a request's state configures it.
    pub fn from_state(state: &crate::config::ConnectionsState) -> Self {
        let service = Self::new(
            state.db_pool.clone(),
            state.cipher.clone(),
            state.public_base_url.clone(),
            state.sealed.clone(),
        )
        .with_tenantless_callback(state.oauth.tenantless_callback)
        .with_apps(state.oauth.apps.clone());
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
        // A platform app is registered with the one callback every tenant shares.
        let tenantless = params[OAUTH_CALLBACK_PARAM].as_str() == Some(OAUTH_CALLBACK_TENANTLESS)
            || params.get(oauth_apps::OAUTH_APP_PARAM).is_some();
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

        let app = self.app_for(tenant_id, integration_id, &params)?;
        if app.is_some() && !self.tenantless_callback {
            return Err(OAuthError::App(
                "platform OAuth apps need the OAuth callback that names no tenant".to_string(),
            ));
        }
        let (client_id, scopes) = match &app {
            Some(app) => (
                app.client_id.clone(),
                oauth_apps::scopes_for(app, params["scopes"].as_str(), oauth_config.default_scopes)
                    .map_err(OAuthError::App)?,
            ),
            None => (
                params["client_id"]
                    .as_str()
                    .ok_or(OAuthError::MissingParameter("client_id".to_string()))?
                    .to_string(),
                params["scopes"]
                    .as_str()
                    .unwrap_or(oauth_config.default_scopes)
                    .to_string(),
            ),
        };

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
            StartBinding::Confirm { started_by } => (FlowBinding::Confirm { started_by }, None),
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
        // The descriptor's own parameters go last, as written (Zoho issues a
        // refresh token only for `access_type=offline`).
        let authorize_params = match oauth_config.authorize_params {
            "" => String::new(),
            params => format!("&{params}"),
        };
        let authorization_url = format!(
            "{}?response_type=code&client_id={}&redirect_uri={}&scope={}&state={}{}{}",
            effective.auth_url,
            urlencoding::encode(&client_id),
            urlencoding::encode(&redirect_uri),
            urlencoding::encode(&scopes),
            urlencoding::encode(&state),
            pkce_query,
            authorize_params,
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
        let started_by = match &flow.binding {
            Some(FlowBinding::Browser { nonce_hash }) => {
                if !oauth_flow::nonce_matches(nonce_hash, browser_nonce) {
                    return Err(OAuthError::BrowserMismatch);
                }
                None
            }
            // Bound by a member's confirmation instead, whoever holds the link.
            Some(FlowBinding::Confirm { started_by }) => Some(started_by.clone()),
            // Started before flows were bound; accepted for this release only.
            None => None,
        };

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
        // seal sends the client secret with the code, so the pair comes back
        // already sealed. A platform app's secret is sealed for this exchange
        // as the connection's own would be, and is never written back.
        let app = self.app_for(tenant_id, &flow.integration_id, &params)?;
        let with_app = match &app {
            Some(app) => Some(
                self.sealed
                    .with_app_client(
                        tenant_id,
                        &flow.connection_id,
                        &flow.integration_id,
                        &params,
                        &app.client_id,
                        app.client_secret.as_str(),
                    )
                    .await
                    .map_err(|error| OAuthError::Internal(error.to_string()))?,
            ),
            None => None,
        };
        let exchanging = with_app.as_ref().unwrap_or(&params);
        if exchanging["client_id"].as_str().is_none() {
            return Err(OAuthError::MissingParameter("client_id".to_string()));
        }
        if !crate::sealed::policy::sealed_authorized_client(&flow.integration_id, exchanging) {
            return Err(OAuthError::MissingParameter("client_secret".to_string()));
        }
        check_callback_region(&flow.integration_id, oauth_config, &params, callback_params)?;

        let sealed = &self.sealed;
        let grant = sealed
            .exchange_code(
                tenant_id,
                &flow.connection_id,
                &flow.integration_id,
                exchanging,
                code,
                &flow.redirect_uri,
                flow.code_verifier.as_deref(),
            )
            .await
            .map_err(|error| {
                if let Some(app) = &app {
                    oauth_apps::record_provider_error(&app.app_id, "exchange", &error.message);
                }
                OAuthError::TokenExchangeFailed(error.message)
            })?;
        // What the access token says of the consent it came from, where seal
        // read it: a sealed token cannot be read here.
        let access_token_claims = grant.claims;
        let mut token_response = json!({
            "access_token": grant.access_token,
            "expires_in": grant.expires_in,
        });
        if let Some(refresh_token) = grant.refresh_token {
            token_response["refresh_token"] = refresh_token;
        }

        let mut granted = granted_fields(&token_response, oauth_config, callback_params)?;
        add_provider_account_fields_through_seal(
            sealed,
            tenant_id,
            &flow.connection_id,
            &flow.integration_id,
            &mut granted,
            &params,
            access_token_claims.as_ref(),
        )
        .await?;
        let mut updated_params = params.clone();
        let status = match started_by {
            None => {
                if let Some(obj) = updated_params.as_object_mut() {
                    obj.extend(granted);
                }
                ConnectionStatus::Active
            }
            Some(started_by) => {
                // Held apart until a member confirms: nothing that uses a
                // connection's tokens finds these.
                let account = oauth_account(&flow.integration_id, &granted).await;
                let previous_status = self
                    .connection_repo
                    .get_by_id(&flow.connection_id, tenant_id)
                    .await
                    .map_err(|e| OAuthError::Internal(e.to_string()))?
                    .map(|connection| connection.status)
                    .filter(|status| *status != ConnectionStatus::PendingConfirmation)
                    .unwrap_or(ConnectionStatus::RequiresReconnection);
                let pending_until =
                    chrono::Utc::now() + chrono::Duration::seconds(PENDING_CONFIRMATION_SECONDS);
                if let Some(obj) = updated_params.as_object_mut() {
                    obj.insert(
                        PENDING_OAUTH_PARAM.to_string(),
                        json!({
                            "id": oauth_flow::new_nonce(),
                            "fields": granted,
                            "account": account,
                            "started_by": started_by,
                            "pending_until": pending_until.to_rfc3339(),
                            "previous_status": previous_status.as_str(),
                        }),
                    );
                }
                ConnectionStatus::PendingConfirmation
            }
        };

        let updated_params = self
            .with_sealed_tokens(
                tenant_id,
                &flow.connection_id,
                &flow.integration_id,
                updated_params,
                &params,
            )
            .await?;
        self.connection_repo
            .update_parameters_and_status(
                &flow.connection_id,
                tenant_id,
                &updated_params,
                status.as_str(),
            )
            .await
            .map_err(|e| OAuthError::Internal(e.to_string()))?;

        Ok(CompletedFlow {
            connection_id: flow.connection_id,
            pending_confirmation: status == ConnectionStatus::PendingConfirmation,
        })
    }

    /// A connection's parameters and its pending authorization, if it has one.
    async fn pending(
        &self,
        tenant_id: &str,
        connection_id: &str,
    ) -> Result<(Value, Option<Value>), OAuthError> {
        let (params, pending, _) = self.pending_of(tenant_id, connection_id).await?;
        Ok((params, pending))
    }

    /// [`Self::pending`], with the connection's type.
    async fn pending_of(
        &self,
        tenant_id: &str,
        connection_id: &str,
    ) -> Result<(Value, Option<Value>, String), OAuthError> {
        let conn = self
            .connection_repo
            .get_with_parameters(connection_id, tenant_id)
            .await
            .map_err(|e| OAuthError::Internal(e.to_string()))?
            .ok_or(OAuthError::ConnectionNotFound)?;
        let integration_id = conn.integration_id.unwrap_or_default();
        let params = conn.connection_parameters.unwrap_or(json!({}));
        let pending = params.get(PENDING_OAUTH_PARAM).cloned();
        Ok((params, pending, integration_id))
    }

    /// What a pending connection shows whoever is to confirm it.
    pub async fn pending_confirmation(
        &self,
        tenant_id: &str,
        connection_id: &str,
    ) -> Result<Option<PendingConfirmation>, OAuthError> {
        let (_, pending) = self.pending(tenant_id, connection_id).await?;
        Ok(pending.map(|pending| PendingConfirmation {
            id: pending["id"].as_str().unwrap_or_default().to_string(),
            account: pending.get("account").filter(|a| !a.is_null()).cloned(),
            started_by: pending["started_by"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
            authorized_at: pending["fields"]["authorized_at"]
                .as_str()
                .map(str::to_string),
            pending_until: pending["pending_until"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
        }))
    }

    /// Confirm a connection a flow started without a browser authorized: its
    /// tokens become the connection's, and it becomes `ACTIVE`. Refused once
    /// its time to be confirmed is over; the tokens are then dropped.
    pub async fn confirm(
        &self,
        tenant_id: &str,
        connection_id: &str,
        pending_id: &str,
    ) -> Result<(), OAuthError> {
        let (mut params, pending, integration_id) =
            self.pending_of(tenant_id, connection_id).await?;
        let pending = pending.ok_or(OAuthError::NotPending)?;
        if !pending_named(&pending, pending_id) {
            return Err(OAuthError::PendingChanged);
        }
        if pending_expired(&pending, chrono::Utc::now()) {
            self.drop_pending(tenant_id, connection_id, params, &pending)
                .await?;
            return Err(OAuthError::ConfirmationExpired);
        }
        let stored = params.clone();
        if let Some(obj) = params.as_object_mut() {
            obj.remove(PENDING_OAUTH_PARAM);
            if let Some(fields) = pending["fields"].as_object() {
                obj.extend(fields.clone());
            }
        }
        // The tokens become the connection's, and are sealed as they do.
        let params = self
            .with_sealed_tokens(tenant_id, connection_id, &integration_id, params, &stored)
            .await?;
        self.connection_repo
            .update_parameters_and_status(
                connection_id,
                tenant_id,
                &params,
                ConnectionStatus::Active.as_str(),
            )
            .await
            .map_err(|e| OAuthError::Internal(e.to_string()))?;
        Ok(())
    }

    /// Reject a connection a flow started without a browser authorized: its
    /// tokens are dropped and revoked at the provider.
    pub async fn reject(
        &self,
        tenant_id: &str,
        connection_id: &str,
        pending_id: &str,
    ) -> Result<(), OAuthError> {
        let (params, pending) = self.pending(tenant_id, connection_id).await?;
        let pending = pending.ok_or(OAuthError::NotPending)?;
        if !pending_named(&pending, pending_id) {
            return Err(OAuthError::PendingChanged);
        }
        self.drop_pending(tenant_id, connection_id, params, &pending)
            .await
    }

    /// Drop and revoke the pending authorization of every connection of
    /// `tenants` whose time to be confirmed is over. Returns how many were
    /// dropped. Only the tenants a process serves: where processes share a
    /// database, another's tenants' credentials are sealed under keys this one
    /// does not hold. A connection that fails is logged and left for the next
    /// sweep; the others go on.
    pub async fn expire_pending(&self, tenants: &[String]) -> Result<u64, OAuthError> {
        let now = chrono::Utc::now();
        let mut dropped = 0;
        for (connection_id, tenant_id) in self
            .connection_repo
            .list_with_status(ConnectionStatus::PendingConfirmation.as_str(), tenants)
            .await
            .map_err(|e| OAuthError::Internal(e.to_string()))?
        {
            let expired = async {
                let (params, pending) = self.pending(&tenant_id, &connection_id).await?;
                match pending {
                    Some(pending) if pending_expired(&pending, now) => {
                        self.drop_pending(&tenant_id, &connection_id, params, &pending)
                            .await?;
                        Ok::<_, OAuthError>(true)
                    }
                    _ => Ok(false),
                }
            };
            match expired.await {
                Ok(true) => dropped += 1,
                Ok(false) | Err(OAuthError::ConnectionNotFound) => {}
                Err(error) => tracing::warn!(
                    connection_id,
                    %error,
                    "dropping an unconfirmed OAuth authorization failed; retried next sweep"
                ),
            }
        }
        Ok(dropped)
    }

    /// Remove a pending authorization, restore the status the connection had
    /// before, and revoke the dropped tokens at the provider (best effort).
    async fn drop_pending(
        &self,
        tenant_id: &str,
        connection_id: &str,
        mut params: Value,
        pending: &Value,
    ) -> Result<(), OAuthError> {
        if let Some(obj) = params.as_object_mut() {
            obj.remove(PENDING_OAUTH_PARAM);
        }
        let status = pending["previous_status"]
            .as_str()
            .map(ConnectionStatus::parse)
            .filter(|status| *status != ConnectionStatus::Unknown)
            .unwrap_or(ConnectionStatus::RequiresReconnection);
        self.connection_repo
            .update_parameters_and_status(connection_id, tenant_id, &params, status.as_str())
            .await
            .map_err(|e| OAuthError::Internal(e.to_string()))?;

        let integration_id = self
            .connection_repo
            .get_by_id(connection_id, tenant_id)
            .await
            .map_err(|e| OAuthError::Internal(e.to_string()))?
            .and_then(|connection| connection.integration_id);
        let oauth_config = integration_id
            .as_deref()
            .and_then(find_connection_type)
            .and_then(|meta| meta.oauth_config);
        if let (Some(_), Some(integration_id)) = (oauth_config, integration_id) {
            // The dropped tokens, with the connection's client credentials.
            // A platform app's client secret is sealed for this request.
            let mut revoked = match self.app_for(tenant_id, &integration_id, &params) {
                Ok(Some(app)) => self
                    .sealed
                    .with_app_client(
                        tenant_id,
                        connection_id,
                        &integration_id,
                        &params,
                        &app.client_id,
                        app.client_secret.as_str(),
                    )
                    .await
                    .unwrap_or_else(|_| params.clone()),
                _ => params.clone(),
            };
            if let (Some(obj), Some(fields)) =
                (revoked.as_object_mut(), pending["fields"].as_object())
            {
                obj.remove("access_token");
                obj.remove("refresh_token");
                obj.extend(fields.clone());
            }
            // The tokens came sealed, and seal names one to the provider.
            let outcome = self
                .sealed
                .revoke_token(tenant_id, connection_id, &integration_id, &revoked)
                .await
                .map(|_| ());
            if let Err(error) = outcome {
                tracing::warn!(
                    connection_id,
                    %error,
                    "revoking an unconfirmed OAuth authorization failed"
                );
            }
        }
        Ok(())
    }
}

/// Whether `pending` is the authorization `pending_id` names.
fn pending_named(pending: &Value, pending_id: &str) -> bool {
    !pending_id.is_empty() && pending["id"].as_str() == Some(pending_id)
}

/// Whether a pending authorization's time to be confirmed is over at `now`.
/// One with no readable deadline is over.
fn pending_expired(pending: &Value, now: chrono::DateTime<chrono::Utc>) -> bool {
    pending["pending_until"]
        .as_str()
        .and_then(|until| chrono::DateTime::parse_from_rfc3339(until).ok())
        .is_none_or(|until| until <= now)
}

/// The provider account a flow connected, for whoever confirms it: a stable
/// id and a display string, never a secret. `None` when the provider does not
/// say.
async fn oauth_account(
    integration_id: &str,
    granted: &serde_json::Map<String, Value>,
) -> Option<Value> {
    match integration_id {
        // Intuit names the company (its realm) in the callback.
        "quickbooks_online" => granted
            .get("realm_id")
            .and_then(Value::as_str)
            .map(|realm| json!({ "id": realm, "display": format!("QuickBooks company {realm}") })),
        // Xero's organisation was looked up after the exchange.
        "xero_oauth" => {
            let tenant = granted.get("tenant_id")?.as_str()?;
            let name = granted
                .get("tenant_name")
                .and_then(Value::as_str)
                .unwrap_or(tenant);
            Some(json!({ "id": tenant, "display": format!("Xero organisation {name}") }))
        }
        // Zoho's organisation was looked up after the exchange.
        "zoho_crm_oauth" => {
            let environment = granted.get("environment")?.as_str()?;
            let id = granted.get("org_id").and_then(Value::as_str);
            let name = granted
                .get("org_name")
                .and_then(Value::as_str)
                .or(id)
                .unwrap_or("?");
            Some(json!({
                "id": id,
                "display": format!("Zoho CRM organisation {name} ({environment})"),
            }))
        }
        // HubSpot names an account only to whoever shows it the token, in a
        // URL: a sealed token is shown to no one.
        _ => None,
    }
}

/// Refuses a callback from another region's login server than the one the
/// code is exchanged at. Zoho names the account's login server on the
/// callback (`accounts-server`); a code from another data centre cannot be
/// exchanged at this connection's, so the flow says which data centre the
/// account is in instead of failing at the exchange.
fn check_callback_region(
    integration_id: &str,
    oauth_config: &OAuthConfig,
    params: &Value,
    callback_params: &std::collections::HashMap<String, String>,
) -> Result<(), OAuthError> {
    if integration_id != "zoho_crm_oauth" {
        return Ok(());
    }
    let Some(server) = callback_params.get("accounts-server") else {
        return Ok(());
    };
    let token_url =
        crate::auth::provider_auth::resolve_effective_oauth_config(oauth_config, params).token_url;
    let expected = token_url
        .trim_start_matches("https://")
        .split('/')
        .next()
        .unwrap_or_default();
    let actual = server
        .trim()
        .trim_start_matches("https://")
        .trim_end_matches('/');
    if actual.eq_ignore_ascii_case(expected) {
        return Ok(());
    }
    Err(OAuthError::AccountUnresolved(
        match runtara_agents::extractors::connection_types::zoho_data_center_of(server) {
            Some(data_center) => format!(
                "this Zoho account is in the `{data_center}` data centre; set the connection's \
                 data centre to `{data_center}` and connect again"
            ),
            None => format!(
                "this Zoho account signed in at {actual}, which is no Zoho data centre this \
                 connection knows"
            ),
        },
    ))
}

/// Xero's list of the organisations a user has authorized for the app.
const XERO_CONNECTIONS_URL: &str = "https://api.xero.com/connections";

/// Fields a provider names only through its API, looked up with the tokens
/// just issued and stored with them. `params` are the connection's own, from
/// before this authorization. A failure fails the flow: the connection could
/// not reach the account the user chose.
///
/// The tokens came back sealed: the lookup goes through seal, which attaches
/// the token, and what the token says of its consent is what seal read from
/// it.
async fn add_provider_account_fields_through_seal(
    sealed: &crate::sealed::SealedCredentials,
    tenant_id: &str,
    connection_id: &str,
    integration_id: &str,
    granted: &mut serde_json::Map<String, Value>,
    params: &Value,
    access_token_claims: Option<&Value>,
) -> Result<(), OAuthError> {
    let lookup = AccountLookup {
        sealed,
        tenant_id,
        connection_id,
        integration_id,
    };
    match integration_id {
        "xero_oauth" => add_xero_organisation(&lookup, granted, params, access_token_claims).await,
        "zoho_crm_oauth" => add_zoho_organisation(&lookup, granted, params).await,
        _ => Ok(()),
    }
}

/// A lookup made with the access token just issued, which is sealed: seal
/// attaches it, and sends the request only where the token may go.
struct AccountLookup<'a> {
    sealed: &'a crate::sealed::SealedCredentials,
    tenant_id: &'a str,
    connection_id: &'a str,
    integration_id: &'a str,
}

impl AccountLookup<'_> {
    /// GETs `url` as the connection `params` describe, with the access token
    /// in `granted`. Returns the status and the body.
    async fn get(
        &self,
        granted: &serde_json::Map<String, Value>,
        params: &Value,
        url: &str,
    ) -> Result<(u16, Vec<u8>), String> {
        let mut presenting = params.clone();
        presenting[crate::sealed::policy::ACCESS_TOKEN_FIELD] = granted
            .get(crate::sealed::policy::ACCESS_TOKEN_FIELD)
            .cloned()
            .ok_or_else(|| "no access_token".to_string())?;
        let response = self
            .sealed
            .send_bounded(
                self.tenant_id,
                self.connection_id,
                self.integration_id,
                &presenting,
                "GET",
                url,
                [("accept".to_string(), "application/json".to_string())],
                None,
                1024 * 1024,
            )
            .await
            .map_err(|error| match error {
                // seal's detail has identifiers and static text only.
                crate::sealed::SendError::Seal(error) => error.to_string(),
                other => format!("{other:?}"),
            })?;
        Ok((response.status, response.body))
    }
}

async fn add_xero_organisation(
    lookup: &AccountLookup<'_>,
    granted: &mut serde_json::Map<String, Value>,
    params: &Value,
    access_token_claims: Option<&Value>,
) -> Result<(), OAuthError> {
    let unresolved =
        |cause: String| OAuthError::AccountUnresolved(format!("Xero organisations: {cause}"));
    let (status, body) = lookup
        .get(granted, params, XERO_CONNECTIONS_URL)
        .await
        .map_err(unresolved)?;
    if !(200..300).contains(&status) {
        return Err(unresolved(format!("HTTP {status}")));
    }
    let connections: Value =
        serde_json::from_slice(&body).map_err(|e| unresolved(e.to_string()))?;
    let event = access_token_claims
        .and_then(|claims| claims[crate::sealed::policy::XERO_EVENT_CLAIM].as_str())
        .map(str::to_owned);
    let fields = xero_account_fields(&connections, event, params["tenant_id"].as_str())
        .map_err(OAuthError::AccountUnresolved)?;
    granted.extend(fields);
    Ok(())
}

/// Zoho binds a token to the organisation chosen on its consent screen, in one
/// environment (production, a Developer Edition, a sandbox), and refuses it on
/// another environment's API host (`DOMAIN_TOKEN_MISMATCH`). The Organization
/// API on the data centre's `www.` host answers for any environment, with the
/// organisation's `type`, so the connection takes its environment from there.
async fn add_zoho_organisation(
    lookup: &AccountLookup<'_>,
    granted: &mut serde_json::Map<String, Value>,
    params: &Value,
) -> Result<(), OAuthError> {
    use runtara_agents::extractors::connection_types::zoho_hosts;
    let unresolved =
        |cause: String| OAuthError::AccountUnresolved(format!("Zoho CRM organisation: {cause}"));
    let mut production = params.clone();
    production["environment"] = json!("production");
    let base = zoho_hosts(&production)
        .ok_or_else(|| unresolved("the connection names no Zoho data centre".to_string()))?
        .base_url;
    let (status, body) = lookup
        .get(granted, params, &format!("{base}/crm/v8/org"))
        .await
        .map_err(unresolved)?;
    if !(200..300).contains(&status) {
        return Err(unresolved(zoho_lookup_refused(status)));
    }
    let org: Value = serde_json::from_slice(&body).map_err(|e| unresolved(e.to_string()))?;
    let fields = zoho_account_fields(&org, params).map_err(OAuthError::AccountUnresolved)?;
    granted.extend(fields);
    Ok(())
}

/// Why Zoho refused the organisation lookup. A token without
/// `ZohoCRM.org.READ` (scopes narrowed on the connection) gets a 401 or 403,
/// which says nothing of the scope, so the message names it.
fn zoho_lookup_refused(status: u16) -> String {
    match status {
        401 | 403 => format!(
            "HTTP {status}; the connection's scopes must include ZohoCRM.org.READ, \
             which the organisation lookup needs"
        ),
        _ => format!("HTTP {status}"),
    }
}

/// The fields a Zoho connection stores for the organisation its token is
/// bound to: the environment its API calls go to, and the organisation's ID
/// and name. All are always written, so nothing of an earlier one stays.
fn zoho_account_fields(
    org: &Value,
    params: &Value,
) -> Result<serde_json::Map<String, Value>, String> {
    let org = org["org"]
        .as_array()
        .and_then(|orgs| orgs.first())
        .ok_or("Zoho named no organisation for this authorization")?;
    let environment = org["type"]
        .as_str()
        .filter(|kind| matches!(*kind, "production" | "developer" | "sandbox"))
        .ok_or(
            "Zoho named no environment (production, developer or sandbox) for the organisation",
        )?;
    let mut bound = params.clone();
    bound["environment"] = json!(environment);
    if runtara_agents::extractors::connection_types::zoho_hosts(&bound)
        .is_none_or(|hosts| hosts.base_url.is_empty())
    {
        return Err(format!(
            "Zoho has no API host for a {environment} organisation in the `{}` data centre",
            params["data_center"].as_str().unwrap_or("us")
        ));
    }
    let id = match &org["id"] {
        Value::String(id) => Value::String(id.clone()),
        Value::Number(id) => Value::String(id.to_string()),
        _ => Value::Null,
    };
    let mut fields = serde_json::Map::new();
    fields.insert("environment".to_string(), json!(environment));
    fields.insert("org_id".to_string(), id);
    fields.insert(
        "org_name".to_string(),
        org["company_name"]
            .as_str()
            .map_or(Value::Null, |name| json!(name)),
    );
    Ok(fields)
}

/// The fields a Xero connection stores for the organisation it uses. The name
/// is always written, `null` when Xero gives none, so that a name from an
/// earlier organisation does not stay.
fn xero_account_fields(
    connections: &Value,
    event: Option<String>,
    previous: Option<&str>,
) -> Result<serde_json::Map<String, Value>, String> {
    let (tenant_id, tenant_name) = select_xero_tenant_of_event(connections, event, previous)?;
    let mut fields = serde_json::Map::new();
    fields.insert("tenant_id".to_string(), Value::String(tenant_id));
    fields.insert(
        "tenant_name".to_string(),
        tenant_name.map_or(Value::Null, Value::String),
    );
    Ok(fields)
}

/// The organisation a Xero consent authorized, from Xero's connections list:
/// the organisation whose `authEventId` is `event`, the access token's
/// `authentication_event_id` as seal read it.
///
/// Reconnecting authorizes no organisation anew: Xero's consent screen shows
/// those already connected as fixed, and the token's event names none of them.
/// The connection then keeps the organisation it had (`previous`) while Xero
/// still lists it, and fails once Xero does not: it never moves to another
/// organisation the consent did not name. A new connection whose consent named
/// none takes the only organisation Xero lists, as does one whose token does
/// not say. One consent can authorize several organisations, but a connection
/// works with one, so several (none of them the connection's own) is an error.
fn select_xero_tenant_of_event(
    connections: &Value,
    event: Option<String>,
    previous: Option<&str>,
) -> Result<(String, Option<String>), String> {
    let organisations: Vec<&Value> = connections
        .as_array()
        .ok_or("Xero's connections list is not a list")?
        .iter()
        .filter(|c| c["tenantType"].as_str() == Some("ORGANISATION"))
        .collect();
    let previous = previous.filter(|tenant| !tenant.is_empty());
    let authorized: Vec<&Value> = match event {
        Some(event) => organisations
            .iter()
            .copied()
            .filter(|c| c["authEventId"].as_str() == Some(event.as_str()))
            .collect(),
        None => Vec::new(),
    };
    fn listed<'a>(list: &[&'a Value], tenant: Option<&str>) -> Option<&'a Value> {
        let tenant = tenant?;
        list.iter()
            .copied()
            .find(|c| c["tenantId"].as_str() == Some(tenant))
    }
    let chosen = match (authorized.as_slice(), previous) {
        ([one], _) => *one,
        ([_, _, ..], _) => listed(&authorized, previous).ok_or_else(|| {
            format!(
                "{} Xero organisations were authorized at once; a connection uses one, \
                 so authorize again and choose only the organisation this connection is for",
                authorized.len()
            )
        })?,
        ([], Some(_)) => listed(&organisations, previous).ok_or(
            "Xero no longer lists the organisation this connection used; \
             authorize again and choose it",
        )?,
        ([], None) => match organisations.as_slice() {
            [only] => *only,
            [] => {
                return Err(
                    "the authorization named no Xero organisation; authorize again and choose one"
                        .to_string(),
                );
            }
            several => {
                return Err(format!(
                    "the authorization named no new Xero organisation, and the app is already \
                     connected to {}, which Xero does not offer to choose again; to use one of \
                     them here, disconnect it in Xero under Settings → Connected apps, then \
                     authorize again and choose it",
                    several.len()
                ));
            }
        },
    };
    let tenant_id = chosen["tenantId"]
        .as_str()
        .filter(|id| uuid::Uuid::parse_str(id).is_ok())
        .ok_or("Xero's connection has no valid tenantId")?;
    Ok((
        tenant_id.to_string(),
        chosen["tenantName"].as_str().map(str::to_string),
    ))
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
            hosts: None,
            authorize_params: "",
            token_header_prefix: "",
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

    /// The consent a token with `claims` names, as seal reads it.
    fn token_with(claims: Value) -> Option<String> {
        claims["authentication_event_id"]
            .as_str()
            .map(str::to_owned)
    }

    const ORG_A: &str = "a3b9b4c0-1d2e-4f50-8a6b-7c8d9e0f1a2b";
    const ORG_B: &str = "b4c0d1e2-2e3f-4a61-9b7c-8d9e0f1a2b3c";

    fn connection(tenant: &str, name: &str, event: &str, kind: &str) -> Value {
        json!({
            "id": "e1eede29-f875-4a5d-8470-17f6a29a88b1",
            "authEventId": event,
            "tenantId": tenant,
            "tenantType": kind,
            "tenantName": name,
        })
    }

    #[test]
    fn xero_tenant_is_the_organisation_of_this_consent() {
        let connections = json!([
            connection(ORG_A, "Earlier Org", "event-1", "ORGANISATION"),
            connection(ORG_B, "Demo Company (UK)", "event-2", "ORGANISATION"),
        ]);
        let token = token_with(json!({ "authentication_event_id": "event-2" }));
        assert_eq!(
            select_xero_tenant_of_event(&connections, token.clone(), None).unwrap(),
            (ORG_B.to_string(), Some("Demo Company (UK)".to_string()))
        );
    }

    #[test]
    fn xero_tenant_falls_back_to_the_only_organisation() {
        // A token that does not name its event, or names one no organisation
        // carries, still has an answer when only one organisation is connected.
        let connections = json!([
            connection(ORG_A, "Demo Company (US)", "event-1", "ORGANISATION"),
            connection(ORG_B, "Practice", "event-1", "PRACTICEMANAGER"),
        ]);
        for token in [
            None,
            token_with(json!({ "authentication_event_id": "event-9" })),
        ] {
            assert_eq!(
                select_xero_tenant_of_event(&connections, token.clone(), None)
                    .unwrap()
                    .0,
                ORG_A,
                "{token:?}"
            );
        }
    }

    #[test]
    fn xero_tenant_refuses_several_organisations_or_none() {
        let token = token_with(json!({ "authentication_event_id": "event-1" }));
        let both = json!([
            connection(ORG_A, "One", "event-1", "ORGANISATION"),
            connection(ORG_B, "Two", "event-1", "ORGANISATION"),
        ]);
        let err = select_xero_tenant_of_event(&both, token.clone(), None).unwrap_err();
        assert!(err.contains("2 Xero organisations"), "{err}");

        for connections in [json!([]), json!({ "not": "a list" })] {
            assert!(select_xero_tenant_of_event(&connections, token.clone(), None).is_err());
        }
        let invalid = json!([connection("not-a-uuid", "Bad", "event-1", "ORGANISATION")]);
        assert!(select_xero_tenant_of_event(&invalid, token.clone(), None).is_err());
    }

    #[test]
    fn xero_reconnect_keeps_the_connections_organisation() {
        // Reconnecting: both organisations were connected by earlier consents,
        // so this consent's event names neither.
        let token = token_with(json!({ "authentication_event_id": "event-3" }));
        let connected = json!([
            connection(ORG_A, "One", "event-1", "ORGANISATION"),
            connection(ORG_B, "Two", "event-2", "ORGANISATION"),
        ]);
        assert_eq!(
            select_xero_tenant_of_event(&connected, token.clone(), Some(ORG_B)).unwrap(),
            (ORG_B.to_string(), Some("Two".to_string()))
        );
        // An organisation Xero no longer lists is not kept.
        let gone = "c5d1e2f3-3f40-4b72-8c8d-9e0f1a2b3c4d";
        assert!(select_xero_tenant_of_event(&connected, token.clone(), Some(gone)).is_err());
        // Several authorized at once, one of them the connection's: that one.
        let both_now = json!([
            connection(ORG_A, "One", "event-3", "ORGANISATION"),
            connection(ORG_B, "Two", "event-3", "ORGANISATION"),
        ]);
        assert_eq!(
            select_xero_tenant_of_event(&both_now, token.clone(), Some(ORG_A))
                .unwrap()
                .0,
            ORG_A
        );
        // A new organisation chosen on reconnect replaces the old one.
        let switched = json!([
            connection(ORG_A, "One", "event-1", "ORGANISATION"),
            connection(ORG_B, "Two", "event-3", "ORGANISATION"),
        ]);
        assert_eq!(
            select_xero_tenant_of_event(&switched, token.clone(), Some(ORG_A))
                .unwrap()
                .0,
            ORG_B
        );
    }

    #[test]
    fn xero_reconnect_never_moves_to_another_organisation() {
        // The connection's organisation was disconnected in Xero; the one
        // Xero still lists came from an earlier consent, not this one.
        let token = token_with(json!({ "authentication_event_id": "event-3" }));
        let other = json!([connection(ORG_A, "Other", "event-1", "ORGANISATION")]);
        let err = select_xero_tenant_of_event(&other, token.clone(), Some(ORG_B)).unwrap_err();
        assert!(err.contains("no longer lists"), "{err}");
        // Nor when the token does not name its consent.
        assert!(select_xero_tenant_of_event(&other, None, Some(ORG_B)).is_err());
        // A new connection (no organisation yet, or an empty one) takes it.
        for previous in [None, Some("")] {
            assert_eq!(
                select_xero_tenant_of_event(&other, token.clone(), previous)
                    .unwrap()
                    .0,
                ORG_A,
                "{previous:?}"
            );
        }
    }

    #[test]
    fn xero_tenant_errors_say_what_to_do() {
        let token = token_with(json!({ "authentication_event_id": "event-3" }));
        // A new connection where every organisation is already connected:
        // Xero's consent screen offers none of them.
        let connected = json!([
            connection(ORG_A, "One", "event-1", "ORGANISATION"),
            connection(ORG_B, "Two", "event-2", "ORGANISATION"),
        ]);
        let err = select_xero_tenant_of_event(&connected, token.clone(), None).unwrap_err();
        assert!(err.contains("Settings → Connected apps"), "{err}");

        let both = json!([
            connection(ORG_A, "One", "event-3", "ORGANISATION"),
            connection(ORG_B, "Two", "event-3", "ORGANISATION"),
        ]);
        let err = select_xero_tenant_of_event(&both, token.clone(), None).unwrap_err();
        for message in [
            err,
            select_xero_tenant_of_event(&connected, token.clone(), None).unwrap_err(),
        ] {
            assert!(!message.contains("  "), "{message}");
        }
    }

    #[test]
    fn xero_account_fields_replace_an_earlier_name() {
        let token = token_with(json!({ "authentication_event_id": "event-1" }));
        let unnamed = json!([{
            "authEventId": "event-1",
            "tenantId": ORG_A,
            "tenantType": "ORGANISATION",
        }]);
        let mut params = json!({ "tenant_id": ORG_B, "tenant_name": "Earlier Org" });
        let fields = xero_account_fields(&unnamed, token, Some(ORG_B)).unwrap();
        params.as_object_mut().unwrap().extend(fields);
        assert_eq!(params, json!({ "tenant_id": ORG_A, "tenant_name": null }));
    }

    #[test]
    fn zoho_connection_takes_the_environment_its_token_is_bound_to() {
        let org = |kind: &str| json!({ "org": [{ "id": "4150868000000225013", "company_name": "Zylker", "type": kind }] });
        let params = json!({ "data_center": "eu", "environment": "production", "org_name": "Old" });
        for kind in ["production", "developer", "sandbox"] {
            let fields = zoho_account_fields(&org(kind), &params).unwrap();
            assert_eq!(
                Value::Object(fields),
                json!({ "environment": kind, "org_id": "4150868000000225013", "org_name": "Zylker" }),
                "{kind}"
            );
        }
        // A numeric ID, and no name: nothing of an earlier organisation stays.
        let fields = zoho_account_fields(
            &json!({ "org": [{ "id": 42, "type": "developer" }] }),
            &params,
        )
        .unwrap();
        assert_eq!(fields["org_id"], json!("42"));
        assert_eq!(fields["org_name"], Value::Null);
    }

    #[test]
    fn a_refused_zoho_lookup_names_the_scope_it_needs() {
        for status in [401, 403] {
            let message = zoho_lookup_refused(status);
            assert!(message.contains("ZohoCRM.org.READ"), "{message}");
        }
        assert_eq!(zoho_lookup_refused(502), "HTTP 502");
    }

    #[test]
    fn zoho_organisation_errors_say_what_is_missing() {
        let eu = json!({ "data_center": "eu" });
        for org in [json!({}), json!({ "org": [] })] {
            let err = zoho_account_fields(&org, &eu).unwrap_err();
            assert!(err.contains("no organisation"), "{err}");
        }
        for kind in [json!("staging"), Value::Null] {
            let err = zoho_account_fields(&json!({ "org": [{ "type": kind }] }), &eu).unwrap_err();
            assert!(err.contains("no environment"), "{err}");
        }
        // China has no Developer Edition host.
        let err = zoho_account_fields(
            &json!({ "org": [{ "type": "developer" }] }),
            &json!({ "data_center": "cn" }),
        )
        .unwrap_err();
        assert!(
            err.contains("no API host for a developer organisation"),
            "{err}"
        );
    }

    #[test]
    fn a_zoho_callback_from_another_data_centre_is_refused_before_the_exchange() {
        let cfg = find_connection_type("zoho_crm_oauth")
            .and_then(|meta| meta.oauth_config)
            .unwrap();
        let params = json!({ "data_center": "eu" });
        let callback = |server: &str| {
            std::collections::HashMap::from([("accounts-server".to_string(), server.to_string())])
        };
        for server in ["https://accounts.zoho.eu", "https://accounts.zoho.eu/"] {
            assert!(
                check_callback_region("zoho_crm_oauth", cfg, &params, &callback(server)).is_ok(),
                "{server}"
            );
        }
        assert!(check_callback_region("zoho_crm_oauth", cfg, &params, &HashMap::new()).is_ok());
        match check_callback_region(
            "zoho_crm_oauth",
            cfg,
            &params,
            &callback("https://accounts.zoho.com"),
        ) {
            Err(OAuthError::AccountUnresolved(message)) => {
                assert!(message.contains("in the `us` data centre"))
            }
            other => panic!("{other:?}"),
        }
        match check_callback_region(
            "zoho_crm_oauth",
            cfg,
            &params,
            &callback("https://evil.example"),
        ) {
            Err(OAuthError::AccountUnresolved(message)) => {
                assert!(message.contains("no Zoho data centre"))
            }
            other => panic!("{other:?}"),
        }
        // Other providers' callbacks are not read for it.
        let xero = find_connection_type("xero_oauth")
            .and_then(|meta| meta.oauth_config)
            .unwrap();
        assert!(check_callback_region("xero_oauth", xero, &params, &callback("x")).is_ok());
    }
}
