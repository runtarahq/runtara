//! Webhook registration/unregistration for channel connections.
//!
//! When a Channel trigger is created, updated, or deleted, the external
//! platform (Telegram, Slack, etc.) needs to be told where to send events.
//! This module handles that lifecycle.

use runtara_connections::ConnectionsFacade;
use serde_json::{Value, json};
use std::sync::Arc;
use tracing::{info, warn};

/// Manages webhook registration with external platforms.
///
/// Called by the trigger service when Channel triggers are created/updated/deleted.
pub struct WebhookManager {
    facade: Arc<ConnectionsFacade>,
    http_client: reqwest::Client,
    /// Public base URL of this runtime instance (e.g. "https://runtime.example.com")
    base_url: Option<String>,
}

/// Result of a webhook registration, including any secrets to store.
pub struct WebhookRegistration {
    /// Secret token to store in the trigger configuration for request validation.
    pub webhook_secret: String,
    /// Platform identifier for URL construction (e.g. "telegram", "slack").
    pub platform: String,
}

impl WebhookManager {
    pub fn new(facade: Arc<ConnectionsFacade>) -> Self {
        let base_url = std::env::var("WEBHOOK_BASE_URL").ok();
        Self {
            facade,
            http_client: reqwest::Client::new(),
            base_url,
        }
    }

    /// Register a webhook for a Channel trigger.
    ///
    /// Returns a `WebhookRegistration` containing the secret token that should
    /// be stored in the trigger's configuration for validating incoming requests.
    pub async fn register(
        &self,
        connection_id: &str,
        tenant_id: &str,
    ) -> Result<WebhookRegistration, WebhookError> {
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

        // Generate a random webhook secret (used for all platform types).
        let webhook_secret = generate_webhook_secret();

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
                self.telegram_set_webhook(bot_token, &webhook_url, &webhook_secret)
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

        Ok(WebhookRegistration {
            webhook_secret,
            platform,
        })
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

        if integration_id == "telegram_bot" {
            let bot_token = params["bot_token"]
                .as_str()
                .ok_or_else(|| WebhookError::InvalidConnection("Missing bot_token".into()))?;

            self.telegram_delete_webhook(bot_token).await?;
            info!(connection_id = %connection_id, "Telegram webhook unregistered");
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
        let url = format!("https://api.telegram.org/bot{}/setWebhook", bot_token);
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
            .map_err(|e| WebhookError::PlatformError(e.to_string()))?;

        let body: Value = resp
            .json()
            .await
            .map_err(|e| WebhookError::PlatformError(e.to_string()))?;

        if body["ok"].as_bool() != Some(true) {
            return Err(WebhookError::PlatformError(format!(
                "Telegram setWebhook failed: {}",
                body
            )));
        }

        Ok(())
    }

    async fn telegram_delete_webhook(&self, bot_token: &str) -> Result<(), WebhookError> {
        let url = format!("https://api.telegram.org/bot{}/deleteWebhook", bot_token);
        let resp = self
            .http_client
            .post(&url)
            .send()
            .await
            .map_err(|e| WebhookError::PlatformError(e.to_string()))?;

        let body: Value = resp
            .json()
            .await
            .map_err(|e| WebhookError::PlatformError(e.to_string()))?;

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
