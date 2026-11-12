use serde_json::json;

/// A messaging channel that can send text to a conversation.
pub trait Channel: Send + Sync + 'static {
    fn send_text(
        &self,
        conversation_id: &str,
        text: &str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<()>> + Send + '_>>;
}

/// How a channel's replies carry the connection's credential: seal attaches
/// it and sends the reply, and refuses any destination the credential's
/// policy does not allow. This process holds the credential sealed only.
pub struct ReplyAuth {
    connections: std::sync::Arc<runtara_connections::ConnectionsFacade>,
    tenant_id: String,
    connection_id: String,
    integration_id: String,
    parameters: serde_json::Value,
}

/// The largest reply acknowledgement read from a provider through seal.
const MAX_REPLY_RESPONSE_BYTES: usize = 1024 * 1024;

impl ReplyAuth {
    /// The credential for a connection's replies, which it holds sealed in
    /// `field`.
    pub fn of(
        connections: &std::sync::Arc<runtara_connections::ConnectionsFacade>,
        tenant_id: &str,
        connection_id: &str,
        integration_id: &str,
        parameters: &serde_json::Value,
        field: &str,
    ) -> anyhow::Result<Self> {
        if !runtara_connections::sealed::is_sealed(&parameters[field]) {
            anyhow::bail!("Missing {field} in connection");
        }
        Ok(Self::presenting(
            connections,
            tenant_id,
            connection_id,
            integration_id,
            parameters.clone(),
        ))
    }

    /// The credential of `parameters`, which present what the connection's
    /// requests carry: its own sealed secret, or a sealed token got with it.
    pub fn presenting(
        connections: &std::sync::Arc<runtara_connections::ConnectionsFacade>,
        tenant_id: &str,
        connection_id: &str,
        integration_id: &str,
        parameters: serde_json::Value,
    ) -> Self {
        Self {
            connections: connections.clone(),
            tenant_id: tenant_id.to_string(),
            connection_id: connection_id.to_string(),
            integration_id: integration_id.to_string(),
            parameters,
        }
    }

    /// POSTs `body` and returns the response's status and JSON.
    async fn post_json(
        &self,
        url: &str,
        content_type: &str,
        body: Vec<u8>,
    ) -> anyhow::Result<(u16, serde_json::Value)> {
        let (status, bytes) = self.post(url, content_type, body).await?;
        Ok((status, serde_json::from_slice(&bytes)?))
    }

    /// POSTs `body` and returns the response's status and body.
    async fn post(
        &self,
        url: &str,
        content_type: &str,
        body: Vec<u8>,
    ) -> anyhow::Result<(u16, Vec<u8>)> {
        let response = self
            .connections
            .sealed()
            .send_bounded(
                &self.tenant_id,
                &self.connection_id,
                &self.integration_id,
                &self.parameters,
                "POST",
                url,
                [("content-type".to_string(), content_type.to_string())],
                Some(body),
                MAX_REPLY_RESPONSE_BYTES,
            )
            .await
            .map_err(|error| match error {
                // seal's detail has identifiers and static text only.
                runtara_connections::sealed::SendError::Seal(seal) => anyhow::anyhow!(
                    "seal did not send the reply: {}: {}",
                    seal.code.as_str(),
                    seal.detail
                ),
                other => anyhow::anyhow!("the reply was not sent: {other:?}"),
            })?;
        Ok((response.status, response.body))
    }
}

// ---------------------------------------------------------------------------
// Telegram
// ---------------------------------------------------------------------------

pub struct TelegramChannel {
    auth: ReplyAuth,
    api_base: String,
}

impl TelegramChannel {
    pub fn new(auth: ReplyAuth) -> Self {
        Self::with_api_base(
            auth,
            runtara_connections::sealed::policy::TELEGRAM_API_BASE.to_string(),
        )
    }

    /// A channel whose Bot API is at `api_base`.
    pub fn with_api_base(auth: ReplyAuth, api_base: String) -> Self {
        Self { auth, api_base }
    }
}

/// The URL of a Bot API method. Telegram takes the token in the path, and
/// seal puts it there: the URL names the method only.
pub fn telegram_method_url(api_base: &str, method: &str) -> String {
    format!("{}/{method}", api_base.trim_end_matches('/'))
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

            let url = telegram_method_url(&self.api_base, "sendMessage");

            // Telegram has a 4096 char limit per message. Split if needed.
            for chunk in split_message(&text, 4096) {
                let request = json!({
                    "chat_id": conversation_id,
                    "text": chunk,
                });
                let (status, body) = self
                    .auth
                    .post_json(&url, "application/json", serde_json::to_vec(&request)?)
                    .await?;

                if !(200..300).contains(&status) {
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
    auth: ReplyAuth,
}

impl SlackChannel {
    pub fn new(auth: ReplyAuth) -> Self {
        Self { auth }
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

                let (_, body) = self
                    .auth
                    .post_json(
                        "https://slack.com/api/chat.postMessage",
                        "application/json",
                        serde_json::to_vec(&request)?,
                    )
                    .await?;
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
    /// SHARED, live `(connection_id, conversation_id) → serviceUrl` map owned by
    /// the router. Sharing (not snapshotting) closes the reply-loss window where
    /// a serviceUrl arrives — or a new conversation appears — after the session
    /// has already started. Keyed by connection too so conversations from
    /// different connections cannot alias.
    service_urls: std::sync::Arc<dashmap::DashMap<(String, String), String>>,
}

impl TeamsChannel {
    pub fn new(
        tenant_id: String,
        connection_id: String,
        params: serde_json::Value,
        facade: std::sync::Arc<runtara_connections::ConnectionsFacade>,
        service_urls: std::sync::Arc<dashmap::DashMap<(String, String), String>>,
    ) -> Self {
        Self {
            tenant_id,
            connection_id,
            params,
            facade,
            service_urls,
        }
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
                .get(&(self.connection_id.clone(), conversation_id.clone()))
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
            let resolved = self
                .facade
                .resolve_connection_auth(
                    &self.connection_id,
                    &self.tenant_id,
                    "teams_bot",
                    &self.params,
                    &mut headers,
                )
                .await
                .map_err(|e| anyhow::anyhow!("Teams auth resolution failed: {e}"))?;
            // The sealed app password gets a sealed token: seal attaches it
            // and sends the reply.
            let auth = ReplyAuth::presenting(
                &self.facade,
                &self.tenant_id,
                &self.connection_id,
                "teams_bot",
                resolved
                    .sealed_parameters
                    .ok_or_else(|| anyhow::anyhow!("Teams auth did not yield a token"))?,
            );

            let url = format!(
                "{}/v3/conversations/{}/activities",
                service_url.trim_end_matches('/'),
                urlencoding::encode(&conversation_id)
            );

            // Teams has a ~28KB message limit. Split at 4000 chars for safety.
            for chunk in split_message(&text, 4000) {
                let activity = json!({
                    "type": "message",
                    "text": chunk,
                    // Keep the Teams compose box enabled after replies.
                    "inputHint": "acceptingInput",
                });
                let (status, body) = auth
                    .post(&url, "application/json", serde_json::to_vec(&activity)?)
                    .await?;

                if !(200..300).contains(&status) {
                    let body = String::from_utf8_lossy(&body).into_owned();
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
    auth: ReplyAuth,
    domain: String,
    region: String,
}

impl MailgunChannel {
    pub fn new(auth: ReplyAuth, domain: String, region: String) -> Self {
        Self {
            auth,
            domain,
            region,
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

            let form = url::form_urlencoded::Serializer::new(String::new())
                .append_pair("from", &from)
                .append_pair("to", &recipient)
                .append_pair("subject", "Re: Your message")
                .append_pair("text", &text)
                .finish();
            let (_, body) = self
                .auth
                .post_json(
                    &base_url,
                    "application/x-www-form-urlencoded",
                    form.into_bytes(),
                )
                .await?;
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

    use std::sync::Arc;

    use runtara_connections::sealed::{Allowances, MemoryKeySource, SealedCredentials};
    use runtara_connections::{ConnectionsConfig, ConnectionsFacade, ConnectionsState};
    use wiremock::matchers::{body_string, header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// A facade over a database it never reaches: a reply reads only the
    /// parameters it was given.
    fn facade(sealed: Option<Arc<SealedCredentials>>) -> Arc<ConnectionsFacade> {
        Arc::new(ConnectionsFacade::new(ConnectionsState::from_config(
            ConnectionsConfig {
                sealed: sealed.unwrap_or_else(SealedCredentials::for_tests),
                agent_access: Arc::new(runtara_connections::EveryAgent),
                db_pool: sqlx::PgPool::connect_lazy("postgres://unused@127.0.0.1:1/unused")
                    .unwrap(),
                redis_manager: None,
                public_base_url: "http://localhost".into(),
                oauth_complete_url: None,
                http_client: reqwest::Client::new(),
                cipher: Arc::new(runtara_connections::crypto::noop::NoOpCipher),
                compatibility: Arc::new(runtara_connections::IntegrationCompatibility::new(
                    Default::default(),
                )),
                agent_catalog: Arc::new(runtara_dsl::agent_meta::AgentCatalog::from_agents(
                    Vec::new(),
                )),
                connection_events: None,
                oauth: Default::default(),
            },
        )))
    }

    async fn provider(authorization: &str) -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/reply"))
            .and(header("authorization", authorization))
            .and(header("content-type", "application/json"))
            .and(body_string("{\"text\":\"hi\"}"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true})))
            .expect(1)
            .mount(&server)
            .await;
        server
    }

    async fn reply(auth: &ReplyAuth, server: &MockServer) -> anyhow::Result<(u16, Value)> {
        auth.post_json(
            &format!("{}/api/reply", server.uri()),
            "application/json",
            b"{\"text\":\"hi\"}".to_vec(),
        )
        .await
    }

    use serde_json::Value;

    #[tokio::test]
    async fn a_reply_with_a_sealed_credential_is_sent_by_seal() {
        let server = provider("Bearer sealed-CANARY").await;
        let address = server.address();
        let sealed = Arc::new(
            SealedCredentials::in_process(
                Arc::new(MemoryKeySource::default()),
                &[7_u8; 32],
                Allowances {
                    private_hosts: vec![format!("{}:{}", address.ip(), address.port())],
                    plain_http_hosts: vec![address.ip().to_string()],
                    ..Default::default()
                },
            )
            .unwrap(),
        );
        // A type whose destination a test can point at a local server; the
        // channels' own are fixed to their providers.
        let parameters = sealed
            .seal_parameters(
                "acme",
                "c-1",
                "http_bearer",
                &json!({"token": "sealed-CANARY", "base_url": format!("{}/api", server.uri())}),
                None,
            )
            .await
            .unwrap()
            .unwrap();
        let of = |connections: &Arc<ConnectionsFacade>| {
            ReplyAuth::of(
                connections,
                "acme",
                "c-1",
                "http_bearer",
                &parameters,
                "token",
            )
            .unwrap()
        };

        let auth = of(&facade(Some(sealed)));
        let (status, body) = reply(&auth, &server).await.unwrap();
        assert_eq!((status, body["ok"].as_bool()), (200, Some(true)));

        // A seal without this tenant's keys cannot send it.
        let error = reply(&of(&facade(None)), &server).await.unwrap_err();
        assert!(error.to_string().contains("KEYSET_UNKNOWN"), "{error}");
    }

    /// A Bot API that takes `sendMessage` for one bot only.
    async fn telegram(token: &str) -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(format!("/bot{token}/sendMessage")))
            .and(body_string("{\"chat_id\":\"42\",\"text\":\"hi\"}"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true})))
            .expect(1)
            .mount(&server)
            .await;
        server
    }

    #[tokio::test]
    async fn a_telegram_reply_with_a_sealed_token_is_sent_by_seal() {
        let server = telegram("1:sealed-CANARY").await;
        let address = server.address();
        let sealed = Arc::new(
            SealedCredentials::in_process(
                Arc::new(MemoryKeySource::default()),
                &[7_u8; 32],
                Allowances {
                    private_hosts: vec![format!("{}:{}", address.ip(), address.port())],
                    plain_http_hosts: vec![address.ip().to_string()],
                    api_bases: [("telegram_bot".to_string(), server.uri())].into(),
                    ..Default::default()
                },
            )
            .unwrap(),
        );
        let parameters = sealed
            .seal_parameters(
                "acme",
                "c-1",
                "telegram_bot",
                &json!({"bot_token": "1:sealed-CANARY"}),
                None,
            )
            .await
            .unwrap()
            .unwrap();
        let auth = ReplyAuth::of(
            &facade(Some(sealed)),
            "acme",
            "c-1",
            "telegram_bot",
            &parameters,
            "bot_token",
        )
        .unwrap();
        // The channel writes no token: the URL names the method only.
        assert_eq!(
            telegram_method_url(&server.uri(), "sendMessage"),
            format!("{}/sendMessage", server.uri())
        );
        let channel = TelegramChannel::with_api_base(auth, server.uri());
        channel.send_text("42", "hi").await.unwrap();
    }

    #[tokio::test]
    async fn a_connection_without_a_sealed_credential_has_no_reply_auth() {
        // None entered, or one this process could read: neither is sent.
        for parameters in [json!({}), json!({"bot_token": "xoxb"})] {
            let auth = ReplyAuth::of(
                &facade(None),
                "acme",
                "c-1",
                "slack_bot",
                &parameters,
                "bot_token",
            );
            assert!(auth.is_err(), "{parameters}");
        }
    }
}
