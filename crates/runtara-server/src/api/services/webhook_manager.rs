//! Webhook registration/unregistration for channel connections.
//!
//! When a Channel trigger is created, updated, or deleted, the external
//! platform (Telegram, Slack, etc.) needs to be told where to send events.
//! This module handles that lifecycle.

use runtara_connections::ConnectionsFacade;
use serde_json::{Value, json};
use sqlx::PgPool;
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
fn platform_error(error: reqwest::Error) -> WebhookError {
    WebhookError::PlatformError(error.without_url().to_string())
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

    #[cfg(all(test, feature = "db-integration-tests"))]
    fn for_test(
        facade: Arc<ConnectionsFacade>,
        base_url: Option<String>,
        telegram_api_base: String,
    ) -> Self {
        Self {
            facade,
            http_client: platform_http_client(PLATFORM_CONNECT_TIMEOUT, PLATFORM_REQUEST_TIMEOUT),
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
            return Err(WebhookError::PlatformError(format!(
                "Telegram setWebhook failed: {}",
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

/// The secret a registration sends, and whether the trigger does not hold it
/// yet and so must store it before the platform is contacted.
#[derive(Debug, PartialEq)]
struct SecretChoice {
    secret: String,
    store_first: bool,
}

/// Pick the secret to register: `preferred` when given, else the trigger's
/// `stored` secret, else a new one. Reusing the stored secret keeps
/// re-registration idempotent, so a failed call cannot strand the platform
/// on a secret the trigger never kept.
fn choose_webhook_secret(stored: Option<&str>, preferred: Option<&str>) -> SecretChoice {
    match (preferred, stored) {
        (Some(preferred), stored) => SecretChoice {
            secret: preferred.to_string(),
            store_first: stored != Some(preferred),
        },
        (None, Some(stored)) => SecretChoice {
            secret: stored.to_string(),
            store_first: false,
        },
        (None, None) => SecretChoice {
            secret: generate_webhook_secret(),
            store_first: true,
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
        let mut config = trigger.configuration.clone().unwrap_or_else(|| json!({}));
        let choice = choose_webhook_secret(
            stored_webhook_secret(&trigger.configuration),
            preferred_secret,
        );

        if choice.store_first && manager.auto_registers() {
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
    /// Platform API call failed.
    PlatformError(String),
    /// Database error.
    DatabaseError(String),
}

impl std::fmt::Display for WebhookError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotConfigured(msg) => write!(f, "Not configured: {}", msg),
            Self::InvalidConnection(msg) => write!(f, "Invalid connection: {}", msg),
            Self::PlatformError(msg) => write!(f, "Platform error: {}", msg),
            Self::DatabaseError(msg) => write!(f, "Database error: {}", msg),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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

    #[test]
    fn a_stored_secret_is_reused_without_storing_again() {
        assert_eq!(
            choose_webhook_secret(Some("stored"), None),
            SecretChoice {
                secret: "stored".into(),
                store_first: false,
            }
        );
    }

    #[test]
    fn a_new_secret_is_stored_before_the_platform_sees_it() {
        let choice = choose_webhook_secret(None, None);
        assert!(choice.store_first);
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
            }
        );
        assert_eq!(
            choose_webhook_secret(None, Some("preferred")),
            SecretChoice {
                secret: "preferred".into(),
                store_first: true,
            }
        );
        assert_eq!(
            choose_webhook_secret(Some("same"), Some("same")),
            SecretChoice {
                secret: "same".into(),
                store_first: false,
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

/// `register_trigger_webhook` against a real database and a fake Telegram
/// that applies every `setWebhook` and then answers with an error, which is
/// the outcome that used to strand Telegram on a secret the trigger never
/// stored.
#[cfg(all(test, feature = "db-integration-tests"))]
mod registration_tests {
    use super::*;
    use crate::api::dto::triggers::CreateInvocationTriggerRequest;
    use axum::{Json, Router, extract::State, http::StatusCode, routing::post};
    use runtara_connections::{
        ConnectionsConfig, ConnectionsState, crypto::noop::NoOpCipher,
        integration_compatibility::IntegrationCompatibility,
    };
    use std::sync::Mutex;
    use uuid::Uuid;

    type Received = Arc<Mutex<Vec<String>>>;

    /// A Telegram Bot API that records each `setWebhook` secret_token as
    /// applied, then fails the call.
    async fn applying_but_failing_telegram() -> (String, Received) {
        async fn set_webhook(
            State(received): State<Received>,
            Json(body): Json<Value>,
        ) -> (StatusCode, &'static str) {
            let secret = body["secret_token"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            received.lock().unwrap().push(secret);
            (
                StatusCode::BAD_GATEWAY,
                "applied, but the reply never made it",
            )
        }

        let received = Received::default();
        let app = Router::new()
            .route("/{bot}/setWebhook", post(set_webhook))
            .with_state(received.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (format!("http://{addr}"), received)
    }

    struct Harness {
        pool: PgPool,
        manager: WebhookManager,
        received: Received,
        tenant: String,
        connection_id: String,
    }

    impl Harness {
        async fn new() -> Self {
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

            let facade = Arc::new(ConnectionsFacade::new(ConnectionsState::from_config(
                ConnectionsConfig {
                    db_pool: pool.clone(),
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
            )));
            let (telegram_api_base, received) = applying_but_failing_telegram().await;
            let manager = WebhookManager::for_test(
                facade,
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
            let mut configuration = json!({"connection_id": self.connection_id});
            if let Some(secret) = webhook_secret {
                configuration["webhook_secret"] = json!(secret);
            }
            let request = CreateInvocationTriggerRequest {
                workflow_id: Uuid::new_v4().to_string(),
                trigger_type: TriggerType::Channel,
                active: true,
                configuration: Some(configuration),
                remote_tenant_id: None,
                single_instance: false,
            };
            TriggerRepository::new(self.pool.clone())
                .create(&request, Some(&self.tenant), None)
                .await
                .unwrap()
        }

        async fn stored_secret(&self, trigger: &InvocationTrigger) -> Option<String> {
            let trigger = TriggerRepository::new(self.pool.clone())
                .get_by_id(&trigger.id, Some(&self.tenant))
                .await
                .unwrap()
                .expect("trigger exists");
            stored_webhook_secret(&trigger.configuration).map(str::to_string)
        }

        fn sent_secrets(&self) -> Vec<String> {
            self.received.lock().unwrap().clone()
        }

        async fn cleanup(&self) {
            let _ = sqlx::query("DELETE FROM invocation_trigger WHERE tenant_id = $1")
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
        let harness = Harness::new().await;
        let trigger = harness.channel_trigger(None).await;

        register_trigger_webhook(
            &harness.pool,
            &harness.manager,
            &trigger,
            &harness.tenant,
            None,
        )
        .await;

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
        let harness = Harness::new().await;
        let trigger = harness.channel_trigger(Some("stored-secret")).await;

        register_trigger_webhook(
            &harness.pool,
            &harness.manager,
            &trigger,
            &harness.tenant,
            None,
        )
        .await;

        assert_eq!(harness.sent_secrets(), vec!["stored-secret".to_string()]);
        assert_eq!(
            harness.stored_secret(&trigger).await.as_deref(),
            Some("stored-secret")
        );
        harness.cleanup().await;
    }

    #[tokio::test]
    async fn a_preferred_secret_is_adopted_before_telegram_is_asked() {
        let harness = Harness::new().await;
        let trigger = harness.channel_trigger(Some("own-secret")).await;

        register_trigger_webhook(
            &harness.pool,
            &harness.manager,
            &trigger,
            &harness.tenant,
            Some("secret-telegram-holds"),
        )
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
}
