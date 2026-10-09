//! Webhook registration/unregistration for channel connections.
//!
//! When a Channel trigger is created, updated, or deleted, the external
//! platform (Telegram, Slack, etc.) needs to be told where to send events.
//! This module handles that lifecycle.

use runtara_connections::ConnectionsFacade;
use serde_json::{Value, json};
use sqlx::PgPool;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;
use tracing::{info, warn};

use crate::api::dto::triggers::{InvocationTrigger, TriggerType};
use crate::api::repositories::triggers::TriggerRepository;

/// Bounds on calls to an external platform's API (e.g. Telegram setWebhook).
/// They run inline in API requests after the database change has committed,
/// and every caller treats a failure as best-effort, so a stalled platform
/// must turn into a logged failure rather than a request that never returns.
const PLATFORM_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const PLATFORM_REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

fn platform_http_client(connect_timeout: Duration, request_timeout: Duration) -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(connect_timeout)
        .timeout(request_timeout)
        .build()
        .expect("failed to build reqwest client")
}

/// Map a failed platform call to a `WebhookError` without the request URL.
/// Telegram puts the bot token in the URL path, and reqwest's error text
/// includes the URL, so keeping it would leak the token wherever the error
/// is logged.
///
/// A connect failure means the request never reached the platform, so it
/// certainly applied nothing. Any later failure (a timeout waiting for the
/// reply, a dropped connection, an unreadable reply) leaves the outcome
/// unknown: the platform may have applied the request.
fn platform_error(error: reqwest::Error) -> WebhookError {
    let unreached = error.is_connect();
    let message = error.without_url().to_string();
    if unreached {
        WebhookError::NotApplied(message)
    } else {
        WebhookError::PlatformError(message)
    }
}

/// Manages webhook registration with external platforms.
///
/// Called by the trigger service when Channel triggers are created/updated/deleted.
pub struct WebhookManager {
    facade: Arc<ConnectionsFacade>,
    http_client: reqwest::Client,
    /// Public base URL of this runtime instance (e.g. "https://runtime.example.com")
    base_url: Option<String>,
    /// Base URL of the Telegram Bot API.
    telegram_api_base: String,
}

const TELEGRAM_API_BASE: &str = "https://api.telegram.org";

impl WebhookManager {
    pub fn new(facade: Arc<ConnectionsFacade>) -> Self {
        let base_url = std::env::var("WEBHOOK_BASE_URL").ok();
        Self {
            facade,
            http_client: platform_http_client(PLATFORM_CONNECT_TIMEOUT, PLATFORM_REQUEST_TIMEOUT),
            base_url,
            telegram_api_base: TELEGRAM_API_BASE.to_string(),
        }
    }

    #[cfg(test)]
    fn for_test(
        facade: Arc<ConnectionsFacade>,
        base_url: Option<String>,
        telegram_api_base: String,
    ) -> Self {
        Self {
            facade,
            http_client: platform_http_client(Duration::from_secs(1), Duration::from_millis(500)),
            base_url,
            telegram_api_base,
        }
    }

    /// Whether this runtime registers webhooks with platforms itself, i.e.
    /// knows its own public URL. Without it `register` always fails before
    /// contacting a platform.
    pub fn auto_registers(&self) -> bool {
        self.base_url.is_some()
    }

    /// Register a webhook for a Channel trigger, sending `webhook_secret` to
    /// platforms that echo it back on every request (Telegram).
    ///
    /// Returns the platform identifier to store alongside the secret in the
    /// trigger's configuration. The caller owns the secret: it must be stored
    /// before this is called, because a platform can apply it even when the
    /// call itself fails or times out.
    pub async fn register(
        &self,
        connection_id: &str,
        tenant_id: &str,
        webhook_secret: &str,
    ) -> Result<String, WebhookError> {
        let base_url = self
            .base_url
            .as_deref()
            .ok_or_else(|| {
                WebhookError::NotConfigured(
                    "WEBHOOK_BASE_URL not set — cannot register webhooks".into(),
                )
            })?
            .trim_end_matches('/');

        let conn = self.load_connection(connection_id, tenant_id).await?;
        let integration_id = conn.integration_id.as_deref().unwrap_or("");
        let params = conn.connection_parameters.as_ref().ok_or_else(|| {
            WebhookError::InvalidConnection("Connection has no parameters".into())
        })?;

        // Map integration_id to platform URL segment.
        let platform = match integration_id {
            "telegram_bot" => "telegram",
            "slack_bot" => "slack",
            "teams_bot" => "teams",
            "mailgun" => "mailgun",
            _ => "channel",
        }
        .to_string();

        match integration_id {
            "telegram_bot" => {
                let bot_token = params["bot_token"]
                    .as_str()
                    .ok_or_else(|| WebhookError::InvalidConnection("Missing bot_token".into()))?;

                let webhook_url = format!(
                    "{}/api/events/{}/webhook/telegram/{}",
                    base_url, tenant_id, connection_id
                );
                self.telegram_set_webhook(bot_token, &webhook_url, webhook_secret)
                    .await?;
                info!(
                    connection_id = %connection_id,
                    webhook_url = %webhook_url,
                    "Telegram webhook registered"
                );
            }
            "slack_bot" => {
                // Slack doesn't support auto-registration. The user must configure
                // the Event Subscription URL in the Slack app dashboard manually.
                let webhook_url = format!(
                    "{}/api/events/{}/webhook/slack/{}",
                    base_url, tenant_id, connection_id
                );
                info!(
                    connection_id = %connection_id,
                    webhook_url = %webhook_url,
                    "Slack webhook URL ready (configure in Slack app dashboard)"
                );
            }
            "teams_bot" => {
                // Teams doesn't support auto-registration. The user must set
                // the messaging endpoint in the Azure Bot resource configuration.
                let webhook_url = format!(
                    "{}/api/events/{}/webhook/teams/{}",
                    base_url, tenant_id, connection_id
                );
                info!(
                    connection_id = %connection_id,
                    webhook_url = %webhook_url,
                    "Teams webhook URL ready (configure in Azure Bot resource)"
                );
            }
            "mailgun" => {
                let webhook_url = format!(
                    "{}/api/events/{}/webhook/mailgun/{}",
                    base_url, tenant_id, connection_id
                );
                info!(
                    connection_id = %connection_id,
                    webhook_url = %webhook_url,
                    "Mailgun webhook URL ready (configure in Mailgun Routes)"
                );
            }
            other => {
                tracing::debug!(
                    integration_id = %other,
                    "Connection type does not support webhook registration"
                );
            }
        }

        Ok(platform)
    }

    /// Unregister a webhook for a Channel trigger.
    pub async fn unregister(
        &self,
        connection_id: &str,
        tenant_id: &str,
    ) -> Result<(), WebhookError> {
        let conn = self.load_connection(connection_id, tenant_id).await?;
        let integration_id = conn.integration_id.as_deref().unwrap_or("");
        let params = conn.connection_parameters.as_ref().ok_or_else(|| {
            WebhookError::InvalidConnection("Connection has no parameters".into())
        })?;

        // Mirror register(): every channel type has an explicit arm so the
        // unregister contract is visible. Only Telegram auto-registers, so the
        // others are deliberate no-ops (their inbound endpoint is configured by
        // the user in the provider's dashboard and cannot be revoked from here).
        match integration_id {
            "telegram_bot" => {
                let bot_token = params["bot_token"]
                    .as_str()
                    .ok_or_else(|| WebhookError::InvalidConnection("Missing bot_token".into()))?;
                self.telegram_delete_webhook(bot_token).await?;
                info!(connection_id = %connection_id, "Telegram webhook unregistered");
            }
            "slack_bot" | "teams_bot" | "mailgun" => {
                tracing::debug!(
                    connection_id = %connection_id,
                    integration_id,
                    "Channel type has no auto-registered webhook to remove (configured in the provider dashboard)"
                );
            }
            other => {
                tracing::debug!(
                    integration_id = %other,
                    "Connection type does not support webhook unregistration"
                );
            }
        }

        Ok(())
    }

    async fn load_connection(
        &self,
        connection_id: &str,
        tenant_id: &str,
    ) -> Result<runtara_connections::ConnectionWithParameters, WebhookError> {
        self.facade
            .get_with_parameters(connection_id, tenant_id)
            .await
            .map_err(|e| WebhookError::DatabaseError(e.to_string()))?
            .ok_or_else(|| {
                WebhookError::InvalidConnection(format!("Connection not found: {}", connection_id))
            })
    }

    async fn telegram_set_webhook(
        &self,
        bot_token: &str,
        webhook_url: &str,
        secret_token: &str,
    ) -> Result<(), WebhookError> {
        let url = format!("{}/bot{}/setWebhook", self.telegram_api_base, bot_token);
        let resp = self
            .http_client
            .post(&url)
            .json(&json!({
                "url": webhook_url,
                "allowed_updates": ["message"],
                "secret_token": secret_token,
            }))
            .send()
            .await
            .map_err(platform_error)?;

        let body: Value = resp.json().await.map_err(platform_error)?;

        if body["ok"].as_bool() != Some(true) {
            return Err(WebhookError::NotApplied(format!(
                "Telegram setWebhook refused: {}",
                body
            )));
        }

        Ok(())
    }

    async fn telegram_delete_webhook(&self, bot_token: &str) -> Result<(), WebhookError> {
        let url = format!("{}/bot{}/deleteWebhook", self.telegram_api_base, bot_token);
        let resp = self
            .http_client
            .post(&url)
            .send()
            .await
            .map_err(platform_error)?;

        let body: Value = resp.json().await.map_err(platform_error)?;

        if body["ok"].as_bool() != Some(true) {
            warn!("Telegram deleteWebhook returned: {}", body);
        }

        Ok(())
    }
}

/// Generate a cryptographically random webhook secret (64 hex chars).
fn generate_webhook_secret() -> String {
    use rand::Rng;
    let bytes: [u8; 32] = rand::thread_rng().r#gen();
    hex::encode(bytes)
}

/// Extract connection_id from a Channel trigger's configuration.
pub fn extract_connection_id(configuration: &Option<Value>) -> Option<&str> {
    configuration
        .as_ref()
        .and_then(|c| c.get("connection_id"))
        .and_then(|v| v.as_str())
}

/// The webhook secret stored in a Channel trigger's configuration, if any.
pub fn stored_webhook_secret(configuration: &Option<Value>) -> Option<&str> {
    configuration
        .as_ref()
        .and_then(|c| c.get("webhook_secret"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
}

/// Whether Telegram accepts `secret` as a webhook `secret_token`: 1 to 256
/// characters of `A-Z`, `a-z`, `0-9`, `_` and `-`. Stored secrets are reused
/// as they are, so one the platform would refuse must be replaced instead.
fn usable_webhook_secret(secret: &str) -> bool {
    (1..=256).contains(&secret.len())
        && secret
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// The secret a registration sends, whether the trigger does not hold it yet
/// and so must store it before the platform is contacted, and whether it was
/// generated for this registration (so the platform cannot hold it unless
/// this registration applied it).
#[derive(Debug, PartialEq)]
struct SecretChoice {
    secret: String,
    store_first: bool,
    generated: bool,
}

/// Pick the secret to register: `preferred` when given, else the trigger's
/// `stored` secret, else a new one; a secret the platform would refuse is
/// skipped. Reusing the stored secret keeps re-registration idempotent, so a
/// failed call cannot strand the platform on a secret the trigger never kept.
fn choose_webhook_secret(stored: Option<&str>, preferred: Option<&str>) -> SecretChoice {
    let stored = stored.filter(|s| usable_webhook_secret(s));
    match (preferred.filter(|s| usable_webhook_secret(s)), stored) {
        (Some(preferred), stored) => SecretChoice {
            secret: preferred.to_string(),
            store_first: stored != Some(preferred),
            generated: false,
        },
        (None, Some(stored)) => SecretChoice {
            secret: stored.to_string(),
            store_first: false,
            generated: false,
        },
        (None, None) => SecretChoice {
            secret: generate_webhook_secret(),
            store_first: true,
            generated: true,
        },
    }
}

/// Best-effort webhook registration for an active Channel trigger.
///
/// Sends `preferred_secret` when given, else the trigger's stored secret, else
/// a new one. A platform can apply the secret even when the call fails or
/// times out, so a secret the trigger does not hold yet is stored first:
/// whatever the outcome, the platform can only end up holding a secret the
/// trigger holds too. If that write fails, the platform is not contacted.
///
/// When the platform certainly did not apply the call (it was never reached,
/// or it refused), a secret generated for this registration is rolled back:
/// the platform cannot hold it, and keeping it would reject every update the
/// platform keeps sending with its previous secret. A preferred secret is
/// kept, because it is chosen as the one the platform already holds.
pub async fn register_trigger_webhook(
    pool: &PgPool,
    manager: &WebhookManager,
    trigger: &InvocationTrigger,
    tenant_id: &str,
    preferred_secret: Option<&str>,
) {
    if trigger.trigger_type == TriggerType::Channel
        && trigger.active
        && let Some(conn_id) = extract_connection_id(&trigger.configuration)
    {
        let repo = TriggerRepository::new(pool.clone());
        let original_config = trigger.configuration.clone().unwrap_or_else(|| json!({}));
        let mut config = original_config.clone();
        let choice = choose_webhook_secret(
            stored_webhook_secret(&trigger.configuration),
            preferred_secret,
        );

        let stored_first = choice.store_first && manager.auto_registers();
        if stored_first {
            if let Some(obj) = config.as_object_mut() {
                obj.insert(
                    "webhook_secret".to_string(),
                    Value::String(choice.secret.clone()),
                );
            }
            if let Err(e) = repo.update_configuration(&trigger.id, &config).await {
                warn!(
                    error = %e,
                    trigger_id = %trigger.id,
                    "Failed to store webhook secret; not registering the webhook"
                );
                return;
            }
        }

        match manager.register(conn_id, tenant_id, &choice.secret).await {
            Ok(platform) => {
                // Store webhook secret and platform in the trigger's configuration.
                if let Some(obj) = config.as_object_mut() {
                    obj.insert("webhook_secret".to_string(), Value::String(choice.secret));
                    obj.insert("platform".to_string(), Value::String(platform));
                }
                if let Err(e) = repo.update_configuration(&trigger.id, &config).await {
                    warn!(error = %e, "Failed to store webhook secret in trigger");
                }
            }
            Err(e) => {
                warn!(error = %e, connection_id = %conn_id, "Failed to register webhook");
                if stored_first && choice.generated && e.platform_unchanged() {
                    match repo
                        .update_configuration(&trigger.id, &original_config)
                        .await
                    {
                        Ok(()) => info!(
                            trigger_id = %trigger.id,
                            "Platform did not apply the webhook; restored the previous secret"
                        ),
                        Err(e) => warn!(
                            error = %e,
                            trigger_id = %trigger.id,
                            "Failed to restore the previous webhook secret"
                        ),
                    }
                }
            }
        }
    }
}

/// Best-effort webhook registration for a Channel trigger that has just
/// become active, by being created or switched on.
///
/// A webhook is registered per connection, and inbound requests are
/// validated against the secret of the newest active Channel trigger on it.
/// So what to register depends on the other live triggers on the connection:
/// - a newer one: its secret is the one validated, and the one the platform
///   holds, so leave the webhook as it is;
/// - an older one (or one created at the same instant): this trigger is now
///   the one validated. It adopts that trigger's secret rather than sending
///   its own: the platform most likely holds it already, so even a failed
///   call leaves the platform and this trigger in agreement, and on a
///   `created_at` tie both triggers hold the same secret;
/// - none: register with this trigger's own secret.
pub async fn activate_channel_webhook(
    pool: &PgPool,
    manager: &WebhookManager,
    trigger: &InvocationTrigger,
    tenant_id: &str,
) {
    if trigger.trigger_type != TriggerType::Channel || !trigger.active {
        return;
    }
    let Some(connection_id) = extract_connection_id(&trigger.configuration) else {
        return;
    };

    let other = match TriggerRepository::new(pool.clone())
        .newest_live_channel_trigger(connection_id, tenant_id, Some(&trigger.id))
        .await
    {
        Ok(other) => other,
        Err(e) => {
            warn!(
                error = %e,
                connection_id,
                trigger_id = %trigger.id,
                "Failed to look up other channel triggers; registering with the trigger's own secret"
            );
            None
        }
    };

    match other {
        Some(newer) if newer.created_at > trigger.created_at => {
            info!(
                connection_id,
                trigger_id = %trigger.id,
                newer_trigger_id = %newer.id,
                "Not registering channel webhook: a newer trigger's secret is the one in use"
            );
        }
        other => {
            let preferred_secret = other
                .as_ref()
                .and_then(|o| stored_webhook_secret(&o.configuration));
            register_trigger_webhook(pool, manager, trigger, tenant_id, preferred_secret).await;
        }
    }
}

/// Best-effort: bring the platform webhooks of Channel triggers that are no
/// longer live (deactivated or deleted) in line with the triggers that
/// remain.
///
/// Pass only triggers that were active before the change. Their rows may
/// already read inactive, so this cannot tell on its own.
///
/// A webhook is registered per connection, and inbound requests are
/// validated against the secret of the newest active Channel trigger on it.
/// So for each connection:
/// - no live trigger left: unregister;
/// - a removed trigger was not older than the newest survivor: the platform
///   may hold the removed trigger's secret (validation order is undefined on
///   a `created_at` tie), so register again for the survivor. The survivor
///   adopts that secret rather than sending its own: the platform most
///   likely holds it already, so even a failed call leaves the platform and
///   the survivor in agreement;
/// - otherwise the survivor's secret is already the one in use; leave it.
pub async fn reconcile_channel_webhooks(
    pool: &PgPool,
    manager: &WebhookManager,
    removed: &[InvocationTrigger],
    tenant_id: &str,
) {
    // Newest removed trigger per connection.
    let mut newest_removed: BTreeMap<&str, &InvocationTrigger> = BTreeMap::new();
    for trigger in removed
        .iter()
        .filter(|t| t.trigger_type == TriggerType::Channel)
    {
        if let Some(connection_id) = extract_connection_id(&trigger.configuration) {
            let newest = newest_removed.entry(connection_id).or_insert(trigger);
            if trigger.created_at > newest.created_at {
                *newest = trigger;
            }
        }
    }

    let trigger_repository = TriggerRepository::new(pool.clone());
    for (connection_id, removed) in newest_removed {
        match trigger_repository
            .newest_live_channel_trigger(connection_id, tenant_id, None)
            .await
        {
            Ok(None) => {
                if let Err(e) = manager.unregister(connection_id, tenant_id).await {
                    warn!(
                        tenant_id,
                        connection_id,
                        error = %e,
                        "Failed to unregister channel webhook"
                    );
                }
            }
            Ok(Some(survivor)) if survivor.created_at <= removed.created_at => {
                info!(
                    tenant_id,
                    connection_id,
                    trigger_id = %survivor.id,
                    "Re-registering channel webhook for the newest remaining trigger"
                );
                register_trigger_webhook(
                    pool,
                    manager,
                    &survivor,
                    tenant_id,
                    stored_webhook_secret(&removed.configuration),
                )
                .await;
            }
            Ok(Some(survivor)) => {
                info!(
                    tenant_id,
                    connection_id,
                    trigger_id = %survivor.id,
                    "Keeping channel webhook registered: a newer trigger still uses the connection"
                );
            }
            Err(e) => {
                warn!(
                    tenant_id,
                    connection_id,
                    error = %e,
                    "Failed to look up remaining channel triggers; leaving webhook as is"
                );
            }
        }
    }
}

#[derive(Debug)]
pub enum WebhookError {
    /// WEBHOOK_BASE_URL not set.
    NotConfigured(String),
    /// Connection not found or missing required fields.
    InvalidConnection(String),
    /// Platform API call failed with an unknown outcome: the platform may
    /// have applied the request before the failure.
    PlatformError(String),
    /// The platform certainly did not apply the request: it never reached
    /// the platform, or the platform refused it.
    NotApplied(String),
    /// Database error.
    DatabaseError(String),
}

impl WebhookError {
    /// Whether the platform's state is certainly unchanged by the failed
    /// call. Every variant but `PlatformError` fails either before the
    /// platform is contacted or with the platform's explicit refusal.
    pub fn platform_unchanged(&self) -> bool {
        !matches!(self, Self::PlatformError(_))
    }
}

impl std::fmt::Display for WebhookError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotConfigured(msg) => write!(f, "Not configured: {}", msg),
            Self::InvalidConnection(msg) => write!(f, "Invalid connection: {}", msg),
            Self::PlatformError(msg) => write!(f, "Platform error: {}", msg),
            Self::NotApplied(msg) => write!(f, "Not applied by platform: {}", msg),
            Self::DatabaseError(msg) => write!(f, "Database error: {}", msg),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::StatusCode;
    use std::net::SocketAddr;
    use std::time::Instant;

    /// A platform that accepts connections and holds them open without ever
    /// responding.
    async fn silent_platform() -> SocketAddr {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok((stream, _)) = listener.accept().await {
                held.push(stream);
            }
        });
        addr
    }

    /// A call a fake Telegram received.
    #[derive(Debug, Clone, PartialEq)]
    pub(super) enum TelegramCall {
        /// `setWebhook`, with the secret_token it was sent.
        SetWebhook(String),
        DeleteWebhook,
    }

    /// Calls a fake Telegram received, in order.
    pub(super) type Received = Arc<std::sync::Mutex<Vec<TelegramCall>>>;

    /// Telegram applied the request, but the reply that reached us is not
    /// Telegram's: the outcome is unknown to the caller.
    pub(super) const APPLIED_REPLY_LOST: (StatusCode, &str) = (
        StatusCode::BAD_GATEWAY,
        "applied, but the reply never made it",
    );
    /// Telegram refused the request.
    pub(super) const REFUSED: (StatusCode, &str) = (
        StatusCode::TOO_MANY_REQUESTS,
        r#"{"ok":false,"error_code":429,"description":"Too Many Requests: retry after 5"}"#,
    );
    pub(super) const ACCEPTED: (StatusCode, &str) =
        (StatusCode::OK, r#"{"ok":true,"result":true}"#);

    /// A fake Telegram Bot API that records every `setWebhook` (with the
    /// secret_token it was sent) and `deleteWebhook`, and answers both with
    /// `reply`.
    pub(super) async fn fake_telegram(reply: (StatusCode, &'static str)) -> (String, Received) {
        let received = Received::default();
        let set_recorder = received.clone();
        let delete_recorder = received.clone();
        let app = axum::Router::new()
            .route(
                "/{bot}/setWebhook",
                axum::routing::post(move |axum::Json(body): axum::Json<Value>| async move {
                    let secret = body["secret_token"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string();
                    set_recorder
                        .lock()
                        .unwrap()
                        .push(TelegramCall::SetWebhook(secret));
                    reply
                }),
            )
            .route(
                "/{bot}/deleteWebhook",
                axum::routing::post(move || async move {
                    delete_recorder
                        .lock()
                        .unwrap()
                        .push(TelegramCall::DeleteWebhook);
                    reply
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (format!("http://{addr}"), received)
    }

    pub(super) fn connections_facade(pool: PgPool) -> Arc<ConnectionsFacade> {
        use runtara_connections::{
            ConnectionsConfig, ConnectionsState, crypto::noop::NoOpCipher,
            integration_compatibility::IntegrationCompatibility,
        };
        Arc::new(ConnectionsFacade::new(ConnectionsState::from_config(
            ConnectionsConfig {
                db_pool: pool,
                redis_manager: None,
                public_base_url: "http://localhost".into(),
                http_client: runtara_connections::net::build_hardened_client(),
                cipher: Arc::new(NoOpCipher),
                compatibility: Arc::new(IntegrationCompatibility::new(Default::default())),
                agent_catalog: Arc::new(runtara_dsl::agent_meta::AgentCatalog::from_agents(
                    Vec::new(),
                )),
                connection_events: None,
            },
        )))
    }

    /// One `setWebhook` call against `telegram_api_base`, with no database.
    async fn set_webhook_against(telegram_api_base: String) -> Result<(), WebhookError> {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgresql://localhost:1/unused")
            .unwrap();
        WebhookManager::for_test(
            connections_facade(pool),
            Some("http://runtime.test".into()),
            telegram_api_base,
        )
        .telegram_set_webhook("000000:test-token", "http://runtime.test/hook", "secret")
        .await
    }

    #[tokio::test]
    async fn platform_client_gives_up_on_a_platform_that_never_answers() {
        let addr = silent_platform().await;

        let client = platform_http_client(Duration::from_secs(1), Duration::from_millis(300));
        let started = Instant::now();
        let result = tokio::time::timeout(
            Duration::from_secs(5),
            client.post(format!("http://{addr}/setWebhook")).send(),
        )
        .await
        .expect("the client's own timeout must fire before the guard");

        let error = result.expect_err("a platform that never answers is an error");
        assert!(error.is_timeout(), "expected a timeout, got: {error}");
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[tokio::test]
    async fn platform_error_does_not_leak_the_bot_token_in_the_url() {
        let addr = silent_platform().await;
        let client = platform_http_client(Duration::from_secs(1), Duration::from_millis(300));
        let token = "123456:token-that-must-not-be-logged";

        let error = client
            .post(format!("http://{addr}/bot{token}/setWebhook"))
            .send()
            .await
            .expect_err("a platform that never answers is an error");
        assert!(
            error.to_string().contains(token),
            "precondition: reqwest's own error text carries the URL"
        );

        let message = platform_error(error).to_string();
        assert!(!message.contains(token), "token leaked: {message}");
        assert!(!message.contains("/bot"), "URL leaked: {message}");
    }

    #[tokio::test]
    async fn a_refusal_from_telegram_is_known_not_to_have_applied() {
        let (base, received) = fake_telegram(REFUSED).await;
        let error = set_webhook_against(base).await.unwrap_err();
        assert!(matches!(error, WebhookError::NotApplied(_)), "{error}");
        assert!(error.platform_unchanged());
        assert_eq!(received.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn an_unreachable_platform_is_known_not_to_have_applied() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);
        let error = set_webhook_against(format!("http://{addr}"))
            .await
            .unwrap_err();
        assert!(matches!(error, WebhookError::NotApplied(_)), "{error}");
        assert!(error.platform_unchanged());
    }

    #[tokio::test]
    async fn a_lost_reply_leaves_the_outcome_unknown() {
        let (base, received) = fake_telegram(APPLIED_REPLY_LOST).await;
        let error = set_webhook_against(base).await.unwrap_err();
        assert!(matches!(error, WebhookError::PlatformError(_)), "{error}");
        assert!(!error.platform_unchanged());
        assert_eq!(received.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn a_reply_that_never_comes_leaves_the_outcome_unknown() {
        let addr = silent_platform().await;
        let error = set_webhook_against(format!("http://{addr}"))
            .await
            .unwrap_err();
        assert!(matches!(error, WebhookError::PlatformError(_)), "{error}");
        assert!(!error.platform_unchanged());
    }

    #[tokio::test]
    async fn an_accepted_set_webhook_succeeds() {
        let (base, _) = fake_telegram(ACCEPTED).await;
        set_webhook_against(base).await.unwrap();
    }

    #[test]
    fn a_stored_secret_is_reused_without_storing_again() {
        assert_eq!(
            choose_webhook_secret(Some("stored"), None),
            SecretChoice {
                secret: "stored".into(),
                store_first: false,
                generated: false,
            }
        );
    }

    #[test]
    fn a_new_secret_is_stored_before_the_platform_sees_it() {
        let choice = choose_webhook_secret(None, None);
        assert!(choice.store_first);
        assert!(choice.generated);
        assert_eq!(choice.secret.len(), 64);
        assert!(choice.secret.bytes().all(|b| b.is_ascii_hexdigit()));
    }

    #[test]
    fn a_preferred_secret_wins_and_is_stored_first_when_it_differs() {
        assert_eq!(
            choose_webhook_secret(Some("stored"), Some("preferred")),
            SecretChoice {
                secret: "preferred".into(),
                store_first: true,
                generated: false,
            }
        );
        assert_eq!(
            choose_webhook_secret(None, Some("preferred")),
            SecretChoice {
                secret: "preferred".into(),
                store_first: true,
                generated: false,
            }
        );
        assert_eq!(
            choose_webhook_secret(Some("same"), Some("same")),
            SecretChoice {
                secret: "same".into(),
                store_first: false,
                generated: false,
            }
        );
    }

    #[test]
    fn a_secret_telegram_would_refuse_is_never_sent() {
        assert!(usable_webhook_secret("aZ09_-"));
        assert!(usable_webhook_secret(&"a".repeat(256)));
        assert!(!usable_webhook_secret(""));
        assert!(!usable_webhook_secret(&"a".repeat(257)));
        assert!(!usable_webhook_secret("has space"));
        assert!(!usable_webhook_secret("dot."));

        // An unusable stored secret is replaced, an unusable preferred one ignored.
        let choice = choose_webhook_secret(Some("not usable!"), None);
        assert!(choice.generated && choice.store_first);
        assert_eq!(
            choose_webhook_secret(Some("stored"), Some("not usable!")),
            SecretChoice {
                secret: "stored".into(),
                store_first: false,
                generated: false,
            }
        );
    }

    #[test]
    fn an_empty_stored_secret_counts_as_none() {
        assert_eq!(
            stored_webhook_secret(&Some(json!({"webhook_secret": ""}))),
            None
        );
        assert_eq!(stored_webhook_secret(&Some(json!({}))), None);
        assert_eq!(
            stored_webhook_secret(&Some(json!({"webhook_secret": "s"}))),
            Some("s")
        );
    }
}

/// `register_trigger_webhook` against a real database and a fake Telegram.
/// The fake either applies every `setWebhook` and then fails the reply (the
/// outcome that used to strand Telegram on a secret the trigger never
/// stored) or refuses it outright.
#[cfg(all(test, feature = "db-integration-tests"))]
mod registration_tests {
    use super::tests::{
        ACCEPTED, APPLIED_REPLY_LOST, REFUSED, Received, TelegramCall, connections_facade,
        fake_telegram,
    };
    use super::*;
    use crate::api::dto::triggers::CreateInvocationTriggerRequest;
    use crate::api::repositories::workflows::WorkflowRepository;
    use axum::http::StatusCode;
    use uuid::Uuid;

    struct Harness {
        pool: PgPool,
        manager: WebhookManager,
        received: Received,
        tenant: String,
        connection_id: String,
    }

    impl Harness {
        async fn new(reply: (StatusCode, &'static str)) -> Self {
            let url = std::env::var("TEST_RUNTARA_SERVER_DATABASE_URL")
                .expect("db-integration-tests requires TEST_RUNTARA_SERVER_DATABASE_URL");
            let pool = PgPool::connect(&url).await.expect("server database");
            sqlx::migrate!("./migrations").run(&pool).await.unwrap();

            let tenant = format!("t-{}", Uuid::new_v4());
            let connection_id = Uuid::new_v4().to_string();
            sqlx::query(
                "INSERT INTO connection_data_entity (id, tenant_id, title, integration_id, connection_parameters, status)
                 VALUES ($1, $2, $3, 'telegram_bot', $4, 'ACTIVE')",
            )
            .bind(&connection_id)
            .bind(&tenant)
            .bind(format!("telegram {connection_id}"))
            .bind(json!({"bot_token": "000000:test-token"}))
            .execute(&pool)
            .await
            .unwrap();

            let (telegram_api_base, received) = fake_telegram(reply).await;
            let manager = WebhookManager::for_test(
                connections_facade(pool.clone()),
                Some("http://runtime.test".into()),
                telegram_api_base,
            );
            Self {
                pool,
                manager,
                received,
                tenant,
                connection_id,
            }
        }

        async fn channel_trigger(&self, webhook_secret: Option<&str>) -> InvocationTrigger {
            self.channel_trigger_created(webhook_secret, 0).await
        }

        /// An active Channel trigger on the harness's bot, for a live
        /// workflow, created `minutes_ago` so tests control which trigger is
        /// newest.
        async fn channel_trigger_created(
            &self,
            webhook_secret: Option<&str>,
            minutes_ago: i32,
        ) -> InvocationTrigger {
            let workflow_id = Uuid::new_v4().to_string();
            WorkflowRepository::new(self.pool.clone())
                .create(
                    &self.tenant,
                    &workflow_id,
                    None,
                    &format!("wf-{workflow_id}"),
                    "/",
                )
                .await
                .unwrap();

            let mut configuration = json!({"connection_id": self.connection_id});
            if let Some(secret) = webhook_secret {
                configuration["webhook_secret"] = json!(secret);
            }
            let request = CreateInvocationTriggerRequest {
                workflow_id,
                trigger_type: TriggerType::Channel,
                active: true,
                configuration: Some(configuration),
                remote_tenant_id: None,
                single_instance: false,
            };
            let trigger = TriggerRepository::new(self.pool.clone())
                .create(&request, Some(&self.tenant), None)
                .await
                .unwrap();
            sqlx::query_as::<_, InvocationTrigger>(
                r#"
                UPDATE invocation_trigger
                SET created_at = NOW() - make_interval(mins => $2)
                WHERE id = $1
                RETURNING id, tenant_id, workflow_id, trigger_type, active, configuration,
                          created_at, last_run, updated_at, remote_tenant_id, single_instance
                "#,
            )
            .bind(&trigger.id)
            .bind(minutes_ago)
            .fetch_one(&self.pool)
            .await
            .unwrap()
        }

        /// Switch `trigger` on or off, returning the updated row.
        async fn set_active(&self, trigger: &InvocationTrigger, active: bool) -> InvocationTrigger {
            sqlx::query_as::<_, InvocationTrigger>(
                r#"
                UPDATE invocation_trigger SET active = $2 WHERE id = $1
                RETURNING id, tenant_id, workflow_id, trigger_type, active, configuration,
                          created_at, last_run, updated_at, remote_tenant_id, single_instance
                "#,
            )
            .bind(&trigger.id)
            .bind(active)
            .fetch_one(&self.pool)
            .await
            .unwrap()
        }

        async fn delete(&self, trigger: &InvocationTrigger) {
            assert!(
                TriggerRepository::new(self.pool.clone())
                    .delete(&trigger.id, Some(&self.tenant))
                    .await
                    .unwrap()
            );
        }

        async fn activate(&self, trigger: &InvocationTrigger) {
            activate_channel_webhook(&self.pool, &self.manager, trigger, &self.tenant).await;
        }

        async fn reconcile(&self, removed: &InvocationTrigger) {
            reconcile_channel_webhooks(
                &self.pool,
                &self.manager,
                std::slice::from_ref(removed),
                &self.tenant,
            )
            .await;
        }

        async fn register(&self, trigger: &InvocationTrigger, preferred_secret: Option<&str>) {
            register_trigger_webhook(
                &self.pool,
                &self.manager,
                trigger,
                &self.tenant,
                preferred_secret,
            )
            .await;
        }

        async fn stored_secret(&self, trigger: &InvocationTrigger) -> Option<String> {
            let trigger = TriggerRepository::new(self.pool.clone())
                .get_by_id(&trigger.id, Some(&self.tenant))
                .await
                .unwrap()
                .expect("trigger exists");
            stored_webhook_secret(&trigger.configuration).map(str::to_string)
        }

        /// The secrets sent with `setWebhook`, in order.
        fn sent_secrets(&self) -> Vec<String> {
            self.calls()
                .into_iter()
                .filter_map(|call| match call {
                    TelegramCall::SetWebhook(secret) => Some(secret),
                    TelegramCall::DeleteWebhook => None,
                })
                .collect()
        }

        fn calls(&self) -> Vec<TelegramCall> {
            self.received.lock().unwrap().clone()
        }

        async fn cleanup(&self) {
            let _ = sqlx::query("DELETE FROM invocation_trigger WHERE tenant_id = $1")
                .bind(&self.tenant)
                .execute(&self.pool)
                .await;
            let _ = sqlx::query("DELETE FROM workflows WHERE tenant_id = $1")
                .bind(&self.tenant)
                .execute(&self.pool)
                .await;
            let _ = sqlx::query("DELETE FROM connection_data_entity WHERE id = $1")
                .bind(&self.connection_id)
                .execute(&self.pool)
                .await;
        }
    }

    #[tokio::test]
    async fn a_new_trigger_stores_the_secret_telegram_was_sent_even_when_the_call_fails() {
        let harness = Harness::new(APPLIED_REPLY_LOST).await;
        let trigger = harness.channel_trigger(None).await;

        harness.register(&trigger, None).await;

        let sent = harness.sent_secrets();
        assert_eq!(sent.len(), 1, "Telegram was asked once");
        assert_eq!(
            harness.stored_secret(&trigger).await.as_deref(),
            Some(sent[0].as_str())
        );
        harness.cleanup().await;
    }

    #[tokio::test]
    async fn re_registering_sends_the_stored_secret_instead_of_a_new_one() {
        let harness = Harness::new(APPLIED_REPLY_LOST).await;
        let trigger = harness.channel_trigger(Some("stored-secret")).await;

        harness.register(&trigger, None).await;

        assert_eq!(harness.sent_secrets(), vec!["stored-secret".to_string()]);
        assert_eq!(
            harness.stored_secret(&trigger).await.as_deref(),
            Some("stored-secret")
        );
        harness.cleanup().await;
    }

    #[tokio::test]
    async fn a_preferred_secret_is_adopted_before_telegram_is_asked() {
        let harness = Harness::new(APPLIED_REPLY_LOST).await;
        let trigger = harness.channel_trigger(Some("own-secret")).await;

        harness
            .register(&trigger, Some("secret-telegram-holds"))
            .await;

        assert_eq!(
            harness.sent_secrets(),
            vec!["secret-telegram-holds".to_string()]
        );
        assert_eq!(
            harness.stored_secret(&trigger).await.as_deref(),
            Some("secret-telegram-holds")
        );
        harness.cleanup().await;
    }

    #[tokio::test]
    async fn a_new_secret_telegram_refused_is_rolled_back() {
        let harness = Harness::new(REFUSED).await;
        let trigger = harness.channel_trigger(None).await;

        harness.register(&trigger, None).await;

        assert_eq!(harness.sent_secrets().len(), 1, "Telegram was asked once");
        assert_eq!(
            harness.stored_secret(&trigger).await,
            None,
            "Telegram cannot hold a secret it refused"
        );
        harness.cleanup().await;
    }

    #[tokio::test]
    async fn an_adopted_secret_is_kept_when_telegram_refuses() {
        let harness = Harness::new(REFUSED).await;
        let trigger = harness.channel_trigger(Some("own-secret")).await;

        harness
            .register(&trigger, Some("secret-telegram-holds"))
            .await;

        assert_eq!(
            harness.stored_secret(&trigger).await.as_deref(),
            Some("secret-telegram-holds"),
            "the adopted secret is the one Telegram already holds"
        );
        harness.cleanup().await;
    }

    #[tokio::test]
    async fn activating_a_trigger_older_than_a_live_one_leaves_the_webhook_alone() {
        let harness = Harness::new(ACCEPTED).await;
        let older = harness.channel_trigger_created(Some("older"), 20).await;
        let _newer = harness.channel_trigger_created(Some("newer"), 10).await;
        let older = harness.set_active(&older, false).await;

        let older = harness.set_active(&older, true).await;
        harness.activate(&older).await;

        assert_eq!(
            harness.calls(),
            Vec::<TelegramCall>::new(),
            "Telegram keeps the newer trigger's secret, the one validated"
        );
        assert_eq!(
            harness.stored_secret(&older).await.as_deref(),
            Some("older")
        );
        harness.cleanup().await;
    }

    #[tokio::test]
    async fn activating_the_newest_trigger_adopts_the_secret_telegram_holds() {
        // Even a refused call must leave the trigger now validated holding
        // the secret Telegram still has.
        let harness = Harness::new(REFUSED).await;
        let _older = harness.channel_trigger_created(Some("older"), 20).await;
        let newer = harness.channel_trigger_created(None, 10).await;

        harness.activate(&newer).await;

        assert_eq!(harness.sent_secrets(), vec!["older".to_string()]);
        assert_eq!(
            harness.stored_secret(&newer).await.as_deref(),
            Some("older")
        );
        harness.cleanup().await;
    }

    #[tokio::test]
    async fn activating_the_only_trigger_registers_its_own_secret() {
        let harness = Harness::new(ACCEPTED).await;
        let trigger = harness.channel_trigger(Some("own")).await;

        harness.activate(&trigger).await;

        assert_eq!(harness.sent_secrets(), vec!["own".to_string()]);
        assert_eq!(
            harness.stored_secret(&trigger).await.as_deref(),
            Some("own")
        );
        harness.cleanup().await;
    }

    #[tokio::test]
    async fn deactivating_the_newest_trigger_hands_its_secret_to_the_survivor() {
        let harness = Harness::new(ACCEPTED).await;
        let older = harness.channel_trigger_created(Some("older"), 20).await;
        let newer = harness.channel_trigger_created(Some("newer"), 10).await;

        harness.set_active(&newer, false).await;
        harness.reconcile(&newer).await;

        assert_eq!(
            harness.calls(),
            vec![TelegramCall::SetWebhook("newer".into())],
            "re-registered with the secret Telegram holds, not unregistered"
        );
        assert_eq!(
            harness.stored_secret(&older).await.as_deref(),
            Some("newer")
        );
        harness.cleanup().await;
    }

    #[tokio::test]
    async fn deleting_the_newest_trigger_hands_its_secret_to_the_survivor() {
        let harness = Harness::new(ACCEPTED).await;
        let older = harness.channel_trigger_created(Some("older"), 20).await;
        let newer = harness.channel_trigger_created(Some("newer"), 10).await;

        harness.delete(&newer).await;
        harness.reconcile(&newer).await;

        assert_eq!(
            harness.calls(),
            vec![TelegramCall::SetWebhook("newer".into())],
            "re-registered with the secret Telegram holds, not unregistered"
        );
        assert_eq!(
            harness.stored_secret(&older).await.as_deref(),
            Some("newer")
        );
        harness.cleanup().await;
    }

    #[tokio::test]
    async fn deactivating_an_older_trigger_leaves_the_webhook_alone() {
        let harness = Harness::new(ACCEPTED).await;
        let older = harness.channel_trigger_created(Some("older"), 20).await;
        let newer = harness.channel_trigger_created(Some("newer"), 10).await;

        harness.set_active(&older, false).await;
        harness.reconcile(&older).await;

        assert_eq!(
            harness.calls(),
            Vec::<TelegramCall>::new(),
            "the newer trigger still receives updates"
        );
        assert_eq!(
            harness.stored_secret(&newer).await.as_deref(),
            Some("newer")
        );
        harness.cleanup().await;
    }

    #[tokio::test]
    async fn deactivating_the_last_trigger_unregisters() {
        let harness = Harness::new(ACCEPTED).await;
        let trigger = harness.channel_trigger(Some("own")).await;

        harness.set_active(&trigger, false).await;
        harness.reconcile(&trigger).await;

        assert_eq!(harness.calls(), vec![TelegramCall::DeleteWebhook]);
        harness.cleanup().await;
    }
}
