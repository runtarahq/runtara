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
}

impl OAuthService {
    pub fn new(pool: PgPool, cipher: Arc<dyn CredentialCipher>, public_base_url: String) -> Self {
        Self {
            oauth_repo: OAuthRepository::new(pool.clone()),
            connection_repo: ConnectionRepository::new(pool, cipher),
            public_base_url,
            flows: None,
            tenantless_callback: true,
            apps: None,
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
        let authorization_url = format!(
            "{}?response_type=code&client_id={}&redirect_uri={}&scope={}&state={}{}",
            effective.auth_url,
            urlencoding::encode(&client_id),
            urlencoding::encode(&redirect_uri),
            urlencoding::encode(&scopes),
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
        // A platform app's credentials go to the provider only: `effective`
        // is never written back.
        let effective = match self.app_for(tenant_id, &flow.integration_id, &params)? {
            Some(app) => oauth_apps::with_app_credentials(&params, &app),
            None => params.clone(),
        };
        let client_id = effective["client_id"]
            .as_str()
            .ok_or(OAuthError::MissingParameter("client_id".to_string()))?;
        let client_secret = effective["client_secret"]
            .as_str()
            .ok_or(OAuthError::MissingParameter("client_secret".to_string()))?;

        // Exchange authorization code for tokens (sending the PKCE verifier if the
        // authorize step generated one).
        let token_response = exchange_code(
            oauth_config,
            &effective,
            code,
            client_id,
            client_secret,
            &flow.redirect_uri,
            flow.code_verifier.as_deref(),
        )
        .await?;

        let granted = granted_fields(&token_response, oauth_config, callback_params)?;
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
        let conn = self
            .connection_repo
            .get_with_parameters(connection_id, tenant_id)
            .await
            .map_err(|e| OAuthError::Internal(e.to_string()))?
            .ok_or(OAuthError::ConnectionNotFound)?;
        let params = conn.connection_parameters.unwrap_or(json!({}));
        let pending = params.get(PENDING_OAUTH_PARAM).cloned();
        Ok((params, pending))
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
        let (mut params, pending) = self.pending(tenant_id, connection_id).await?;
        let pending = pending.ok_or(OAuthError::NotPending)?;
        if !pending_named(&pending, pending_id) {
            return Err(OAuthError::PendingChanged);
        }
        if pending_expired(&pending, chrono::Utc::now()) {
            self.drop_pending(tenant_id, connection_id, params, &pending)
                .await?;
            return Err(OAuthError::ConfirmationExpired);
        }
        if let Some(obj) = params.as_object_mut() {
            obj.remove(PENDING_OAUTH_PARAM);
            if let Some(fields) = pending["fields"].as_object() {
                obj.extend(fields.clone());
            }
        }
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

    /// Drop and revoke the pending authorization of every connection whose
    /// time to be confirmed is over. Returns how many were dropped. Runs on
    /// the system path, across the tenants the pool sees. A connection that
    /// fails is logged and left for the next sweep; the others go on.
    pub async fn expire_pending(&self) -> Result<u64, OAuthError> {
        let now = chrono::Utc::now();
        let mut dropped = 0;
        for (connection_id, tenant_id) in self
            .connection_repo
            .list_with_status(ConnectionStatus::PendingConfirmation.as_str())
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
        if let (Some(oauth_config), Some(integration_id)) = (oauth_config, integration_id) {
            // The dropped tokens, with the connection's client credentials.
            let mut revoked = match self.app_for(tenant_id, &integration_id, &params) {
                Ok(Some(app)) => oauth_apps::with_app_credentials(&params, &app),
                _ => params.clone(),
            };
            if let (Some(obj), Some(fields)) =
                (revoked.as_object_mut(), pending["fields"].as_object())
            {
                obj.remove("access_token");
                obj.remove("refresh_token");
                obj.extend(fields.clone());
            }
            if let Err(error) = crate::auth::provider_auth::revoke_oauth_token(
                crate::net::shared_hardened_client(),
                oauth_config,
                &revoked,
            )
            .await
            {
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
        // HubSpot describes a token: its portal (hub) and the approving user.
        "hubspot_private_app" => {
            let token = granted.get("access_token")?.as_str()?;
            let response = crate::net::shared_hardened_client()
                .get(format!(
                    "https://api.hubapi.com/oauth/v1/access-tokens/{}",
                    urlencoding::encode(token)
                ))
                .timeout(std::time::Duration::from_secs(10))
                .send()
                .await
                .ok()?;
            if !response.status().is_success() {
                return None;
            }
            let info: Value = response.json().await.ok()?;
            let hub = info["hub_id"].as_u64()?;
            let domain = info["hub_domain"].as_str().unwrap_or_default();
            let user = info["user"].as_str().unwrap_or_default();
            Some(json!({
                "id": hub.to_string(),
                "display": format!("{domain} ({user})").trim().to_string(),
            }))
        }
        _ => None,
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

    #[tokio::test]
    async fn a_platform_apps_credentials_go_to_the_provider_and_nowhere_else() {
        use wiremock::matchers::{body_string_contains, header, method, path};
        let _loopback = crate::net::test_allowlist::allow("127.0.0.1");
        let server = wiremock::MockServer::start().await;
        // base64("platform-client:platform-secret")
        wiremock::Mock::given(method("POST"))
            .and(path("/token"))
            .and(header(
                "Authorization",
                "Basic cGxhdGZvcm0tY2xpZW50OnBsYXRmb3JtLXNlY3JldA==",
            ))
            .and(body_string_contains("code=code-1"))
            .respond_with(
                wiremock::ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"access_token": "at-1"})),
            )
            .expect(1)
            .mount(&server)
            .await;
        let config = OAuthConfig {
            token_url: Box::leak(format!("{}/token", server.uri()).into_boxed_str()),
            token_endpoint_auth: TokenEndpointAuth::HttpBasic,
            ..cfg_with(&[])
        };
        let app = crate::oauth_apps::tests::app("hubspot", "hubspot_private_app");
        let stored = serde_json::json!({ "oauth_app": "hubspot", "scopes": "oauth" });
        let effective = crate::oauth_apps::with_app_credentials(&stored, &app);
        exchange_code(
            &config,
            &effective,
            "code-1",
            effective["client_id"].as_str().unwrap(),
            effective["client_secret"].as_str().unwrap(),
            "https://runtara.example/api/oauth/callback",
            None,
        )
        .await
        .unwrap_or_else(|err| panic!("exchange: {err}"));

        // What a completed flow writes back is the stored parameters and the
        // grant; neither holds the app's secret.
        let mut written = stored.clone();
        written.as_object_mut().unwrap().extend(
            granted_fields(
                &serde_json::json!({"access_token": "at-1"}),
                &config,
                &HashMap::new(),
            )
            .unwrap(),
        );
        assert!(
            !written.to_string().contains("platform-secret"),
            "{written}"
        );
        assert!(written.get("client_id").is_none());
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
