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

            // Slack conversation ids include the thread timestamp after the
            // first colon. This is also how reaction callbacks target replies
            // at the message that was reacted to.
            let (channel_id, thread_ts) = split_slack_conversation_id(&conversation_id);

            // Slack has a 4000 char limit per message.
            for chunk in split_message(&text, 4000) {
                let mut request = json!({
                    "channel": channel_id,
                    "text": chunk,
                });
                if let Some(thread_ts) = thread_ts {
                    request["thread_ts"] = json!(thread_ts);
                }

                let resp = self
                    .client
                    .post("https://slack.com/api/chat.postMessage")
                    .header("Authorization", format!("Bearer {}", self.token))
                    .json(&request)
                    .send()
                    .await?;

                let body: serde_json::Value = resp.json().await?;
                if body["ok"].as_bool() != Some(true) {
                    tracing::warn!(
                        channel = %channel_id,
                        error = %body["error"],
                        "Slack chat.postMessage failed"
                    );
                }
            }

            Ok(())
        })
    }
}

fn split_slack_conversation_id(conversation_id: &str) -> (&str, Option<&str>) {
    match conversation_id.split_once(':') {
        Some((channel_id, thread_ts)) if !channel_id.is_empty() && !thread_ts.is_empty() => {
            (channel_id, Some(thread_ts))
        }
        _ => (conversation_id, None),
    }
}

// ---------------------------------------------------------------------------
// Microsoft Teams
// ---------------------------------------------------------------------------

/// Teams Bot Framework adapter for session replies.
///
/// Unlike the prototype, the bearer token is minted through the shared
/// connection-auth path (`ConnectionsFacade::resolve_connection_auth`, the
/// `teams_bot` arm), which uses the correct single-tenant authority and the
/// process-wide token cache — no hand-rolled multi-tenant token endpoint. The
/// activity POST goes through a hardened client (no redirects, DNS-guarded),
/// and the inbound-derived `serviceUrl` is re-validated (https, non-private)
/// before a token-bearing request is sent to it. Non-success responses
/// propagate as errors.
pub struct TeamsChannel {
    tenant_id: String,
    connection_id: String,
    /// Connection parameters (app_id/app_password/azure_tenant_id/app_type),
    /// consumed by the facade to mint the Bot Connector token.
    params: serde_json::Value,
    facade: std::sync::Arc<runtara_connections::ConnectionsFacade>,
    /// Hardened egress client (no redirects + DNS guard).
    client: reqwest::Client,
    /// Per-conversation service URL (set from authenticated inbound activities).
    service_urls: dashmap::DashMap<String, String>,
}

impl TeamsChannel {
    pub fn new(
        tenant_id: String,
        connection_id: String,
        params: serde_json::Value,
        facade: std::sync::Arc<runtara_connections::ConnectionsFacade>,
        client: reqwest::Client,
    ) -> Self {
        Self {
            tenant_id,
            connection_id,
            params,
            facade,
            client,
            service_urls: dashmap::DashMap::new(),
        }
    }

    /// Store the service URL for a conversation (called from the webhook handler).
    pub fn set_service_url(&self, conversation_id: &str, service_url: &str) {
        self.service_urls
            .insert(conversation_id.to_string(), service_url.to_string());
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

            // Defense in depth: the serviceUrl arrived in an authenticated
            // activity, but re-validate before sending a bearer token to it.
            runtara_connections::net::validate_public_url(
                &service_url,
                runtara_connections::net::host_is_allowlisted_for_egress,
            )
            .map_err(|e| anyhow::anyhow!("Refusing to send to serviceUrl: {e}"))?;

            // Mint the Bot Connector token through the shared connection-auth
            // path (correct single-tenant authority + process token cache).
            let mut headers = std::collections::HashMap::new();
            self.facade
                .resolve_connection_auth(
                    &self.connection_id,
                    &self.tenant_id,
                    "teams_bot",
                    &self.params,
                    &mut headers,
                )
                .await
                .map_err(|e| anyhow::anyhow!("Teams auth resolution failed: {e}"))?;
            let authorization = headers
                .get("Authorization")
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("Teams auth did not yield a bearer token"))?;

            let url = format!(
                "{}/v3/conversations/{}/activities",
                service_url.trim_end_matches('/'),
                urlencoding::encode(&conversation_id)
            );

            // Teams has a ~28KB message limit. Split at 4000 chars for safety.
            for chunk in split_message(&text, 4000) {
                let resp = self
                    .client
                    .post(&url)
                    .header("Authorization", &authorization)
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
                    // Propagate the failure instead of reporting success: a
                    // 401/403/404/429/5xx from the Bot Connector must not be
                    // swallowed (the session loop can then surface it).
                    anyhow::bail!("Teams send activity failed ({status}): {body}");
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_slack_thread_conversation_id() {
        assert_eq!(
            split_slack_conversation_id("C123:1712345678.000100"),
            ("C123", Some("1712345678.000100"))
        );
        assert_eq!(split_slack_conversation_id("C123"), ("C123", None));
    }

    #[test]
    fn split_message_respects_utf8_boundaries() {
        assert_eq!(split_message("a😀b", 5), vec!["a😀", "b"]);
    }
}
