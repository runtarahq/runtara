use serde_json::json;

/// A messaging channel that can send text to a conversation.
pub trait Channel: Send + Sync + 'static {
    fn send_text(
        &self,
        conversation_id: &str,
        text: &str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<()>> + Send + '_>>;
}

// ---------------------------------------------------------------------------
// Telegram
// ---------------------------------------------------------------------------

pub struct TelegramChannel {
    token: String,
    client: reqwest::Client,
}

impl TelegramChannel {
    pub fn new(token: String, client: reqwest::Client) -> Self {
        Self { token, client }
    }
}

impl Channel for TelegramChannel {
    fn send_text(
        &self,
        conversation_id: &str,
        text: &str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<()>> + Send + '_>> {
        let conversation_id = conversation_id.to_string();
        let text = text.to_string();
        Box::pin(async move {
            if text.is_empty() {
                return Ok(());
            }

            let url = format!("https://api.telegram.org/bot{}/sendMessage", self.token);

            // Telegram has a 4096 char limit per message. Split if needed.
            for chunk in split_message(&text, 4096) {
                let resp = self
                    .client
                    .post(&url)
                    .json(&json!({
                        "chat_id": conversation_id,
                        "text": chunk,
                    }))
                    .send()
                    .await?;

                if !resp.status().is_success() {
                    let body = resp.text().await.unwrap_or_default();
                    tracing::warn!(
                        chat_id = %conversation_id,
                        status = %body,
                        "Telegram sendMessage failed"
                    );
                }
            }

            Ok(())
        })
    }
}

// ---------------------------------------------------------------------------
// Slack
// ---------------------------------------------------------------------------

pub struct SlackChannel {
    token: String,
    client: reqwest::Client,
}

impl SlackChannel {
    pub fn new(token: String, client: reqwest::Client) -> Self {
        Self { token, client }
    }
}

impl Channel for SlackChannel {
    fn send_text(
        &self,
        conversation_id: &str,
        text: &str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<()>> + Send + '_>> {
        let conversation_id = conversation_id.to_string();
        let text = text.to_string();
        Box::pin(async move {
            if text.is_empty() {
                return Ok(());
            }

            // Slack has a 4000 char limit per message.
            for chunk in split_message(&text, 4000) {
                let resp = self
                    .client
                    .post("https://slack.com/api/chat.postMessage")
                    .header("Authorization", format!("Bearer {}", self.token))
                    .json(&json!({
                        "channel": conversation_id,
                        "text": chunk,
                    }))
                    .send()
                    .await?;

                let body: serde_json::Value = resp.json().await?;
                if body["ok"].as_bool() != Some(true) {
                    tracing::warn!(
                        channel = %conversation_id,
                        error = %body["error"],
                        "Slack chat.postMessage failed"
                    );
                }
            }

            Ok(())
        })
    }
}

// ---------------------------------------------------------------------------
// Microsoft Teams
// ---------------------------------------------------------------------------

/// Teams Bot Framework adapter with OAuth2 token caching.
///
/// The `service_url` is extracted from inbound activities and stored per
/// conversation. Outbound messages require a bearer token obtained by
/// exchanging `app_id` + `app_password` with Azure AD.
pub struct TeamsChannel {
    app_id: String,
    app_password: String,
    client: reqwest::Client,
    /// Cached bearer token + expiry.
    token_cache: tokio::sync::RwLock<Option<(String, std::time::Instant)>>,
    /// Per-conversation service URL (set from inbound activity).
    service_urls: dashmap::DashMap<String, String>,
}

impl TeamsChannel {
    pub fn new(app_id: String, app_password: String, client: reqwest::Client) -> Self {
        Self {
            app_id,
            app_password,
            client,
            token_cache: tokio::sync::RwLock::new(None),
            service_urls: dashmap::DashMap::new(),
        }
    }

    /// Store the service URL for a conversation (called from the webhook handler).
    pub fn set_service_url(&self, conversation_id: &str, service_url: &str) {
        self.service_urls
            .insert(conversation_id.to_string(), service_url.to_string());
    }

    /// Get a valid bearer token, refreshing if expired.
    async fn get_token(&self) -> anyhow::Result<String> {
        // Check cache.
        {
            let cache = self.token_cache.read().await;
            if let Some((ref token, expiry)) = *cache
                && std::time::Instant::now() < expiry
            {
                return Ok(token.clone());
            }
        }

        // Fetch new token.
        let token_url = "https://login.microsoftonline.com/botframework.com/oauth2/v2.0/token";

        let resp = self
            .client
            .post(token_url)
            .form(&[
                ("grant_type", "client_credentials"),
                ("client_id", &self.app_id),
                ("client_secret", &self.app_password),
                ("scope", "https://api.botframework.com/.default"),
            ])
            .send()
            .await?;

        let body: serde_json::Value = resp.json().await?;
        let access_token = body["access_token"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("No access_token in response: {}", body))?
            .to_string();
        let expires_in = body["expires_in"].as_u64().unwrap_or(3600);

        // Cache with 5-minute safety margin.
        let expiry = std::time::Instant::now()
            + std::time::Duration::from_secs(expires_in.saturating_sub(300));
        {
            let mut cache = self.token_cache.write().await;
            *cache = Some((access_token.clone(), expiry));
        }

        Ok(access_token)
    }
}

impl Channel for TeamsChannel {
    fn send_text(
        &self,
        conversation_id: &str,
        text: &str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<()>> + Send + '_>> {
        let conversation_id = conversation_id.to_string();
        let text = text.to_string();
        Box::pin(async move {
            if text.is_empty() {
                return Ok(());
            }

            let service_url = self
                .service_urls
                .get(&conversation_id)
                .map(|v| v.clone())
                .ok_or_else(|| {
                    anyhow::anyhow!("No service URL for conversation {}", conversation_id)
                })?;

            let token = self.get_token().await?;
            let url = format!(
                "{}/v3/conversations/{}/activities",
                service_url.trim_end_matches('/'),
                conversation_id
            );

            // Teams has a ~28KB message limit. Split at 4000 chars for safety.
            for chunk in split_message(&text, 4000) {
                let resp = self
                    .client
                    .post(&url)
                    .header("Authorization", format!("Bearer {}", token))
                    .json(&json!({
                        "type": "message",
                        "text": chunk,
                    }))
                    .send()
                    .await?;

                if !resp.status().is_success() {
                    let status = resp.status();
                    let body = resp.text().await.unwrap_or_default();
                    tracing::warn!(
                        conversation = %conversation_id,
                        status = %status,
                        body = %body,
                        "Teams send activity failed"
                    );
                }
            }

            Ok(())
        })
    }
}

// ---------------------------------------------------------------------------
// Mailgun (email channel)
// ---------------------------------------------------------------------------

/// Mailgun email channel adapter.
///
/// Sends reply emails via the Mailgun REST API. The `conv_id` is the
/// recipient's email address (the original sender of the inbound email).
pub struct MailgunChannel {
    api_key: String,
    domain: String,
    region: String,
    client: reqwest::Client,
}

impl MailgunChannel {
    pub fn new(api_key: String, domain: String, region: String, client: reqwest::Client) -> Self {
        Self {
            api_key,
            domain,
            region,
            client,
        }
    }
}

impl Channel for MailgunChannel {
    fn send_text(
        &self,
        conversation_id: &str,
        text: &str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<()>> + Send + '_>> {
        let recipient = conversation_id.to_string();
        let text = text.to_string();
        Box::pin(async move {
            if text.is_empty() {
                return Ok(());
            }

            let base_url = match self.region.as_str() {
                "eu" => format!("https://api.eu.mailgun.net/v3/{}/messages", self.domain),
                _ => format!("https://api.mailgun.net/v3/{}/messages", self.domain),
            };

            let from = format!("noreply@{}", self.domain);

            let resp = self
                .client
                .post(&base_url)
                .basic_auth("api", Some(&self.api_key))
                .form(&[
                    ("from", from.as_str()),
                    ("to", &recipient),
                    ("subject", "Re: Your message"),
                    ("text", &text),
                ])
                .send()
                .await?;

            let body: serde_json::Value = resp.json().await?;
            if body["id"].is_null() {
                tracing::warn!(
                    recipient = %recipient,
                    error = %body,
                    "Mailgun send failed"
                );
            }

            Ok(())
        })
    }
}

/// Find the last char boundary at or before `pos`.
fn floor_char_boundary(s: &str, pos: usize) -> usize {
    if pos >= s.len() {
        return s.len();
    }
    let mut i = pos;
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

fn split_message(text: &str, max_len: usize) -> Vec<&str> {
    if text.len() <= max_len {
        return vec![text];
    }

    let mut chunks = Vec::new();
    let mut remaining = text;

    while !remaining.is_empty() {
        if remaining.len() <= max_len {
            chunks.push(remaining);
            break;
        }

        // Find a safe byte boundary that doesn't split a UTF-8 codepoint.
        let safe_max = floor_char_boundary(remaining, max_len);
        let cut = remaining[..safe_max]
            .rfind('\n')
            .map(|p| p + 1)
            .unwrap_or(safe_max);

        chunks.push(&remaining[..cut]);
        remaining = &remaining[cut..];
    }

    chunks
}
