use serde_json::json;

/// A messaging channel that can send text to a conversation.
pub trait Channel: Send + Sync + 'static {
    fn send_text(
        &self,
        conversation_id: &str,
        text: &str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<()>> + Send + '_>>;
}

/// How a channel's replies carry the connection's credential.
pub enum ReplyAuth {
    /// A token this process holds, sent as `Authorization: Bearer`.
    Bearer(String),
    /// A password this process holds, sent with basic auth.
    Basic { username: String, password: String },
    /// A token this process holds, which the provider takes in the URL's
    /// path: the caller writes it there.
    InPath(String),
    /// An `Authorization` header this process holds, sent as it is.
    Header(String),
    /// The connection's secret is sealed: seal attaches it and sends the
    /// reply, and refuses any destination the secret's policy does not allow.
    Sealed {
        connections: std::sync::Arc<runtara_connections::ConnectionsFacade>,
        tenant_id: String,
        connection_id: String,
        integration_id: String,
        parameters: serde_json::Value,
    },
}

/// The largest reply acknowledgement read from a provider through seal.
const MAX_REPLY_RESPONSE_BYTES: usize = 1024 * 1024;

impl ReplyAuth {
    /// The credential for a connection's replies: sealed if the connection
    /// holds `field` sealed, else `plain` of its value.
    pub fn of(
        connections: &std::sync::Arc<runtara_connections::ConnectionsFacade>,
        tenant_id: &str,
        connection_id: &str,
        integration_id: &str,
        parameters: &serde_json::Value,
        field: &str,
        plain: impl FnOnce(String) -> Self,
    ) -> anyhow::Result<Self> {
        if runtara_connections::sealed::is_sealed(&parameters[field]) {
            return Ok(Self::Sealed {
                connections: connections.clone(),
                tenant_id: tenant_id.to_string(),
                connection_id: connection_id.to_string(),
                integration_id: integration_id.to_string(),
                parameters: parameters.clone(),
            });
        }
        parameters[field]
            .as_str()
            .map(|value| plain(value.to_string()))
            .ok_or_else(|| anyhow::anyhow!("Missing {field} in connection"))
    }

    /// POSTs `body` and returns the response's status and JSON.
    async fn post_json(
        &self,
        client: &reqwest::Client,
        url: &str,
        content_type: &str,
        body: Vec<u8>,
    ) -> anyhow::Result<(u16, serde_json::Value)> {
        let (status, bytes) = self.post(client, url, content_type, body).await?;
        Ok((status, serde_json::from_slice(&bytes)?))
    }

    /// POSTs `body` and returns the response's status and body.
    async fn post(
        &self,
        client: &reqwest::Client,
        url: &str,
        content_type: &str,
        body: Vec<u8>,
    ) -> anyhow::Result<(u16, Vec<u8>)> {
        let (status, bytes) = match self {
            Self::Sealed {
                connections,
                tenant_id,
                connection_id,
                integration_id,
                parameters,
            } => {
                let sealed = connections.sealed().ok_or_else(|| {
                    anyhow::anyhow!(
                        "the connection's credentials are sealed, and this deployment has no seal"
                    )
                })?;
                let response = sealed
                    .send_bounded(
                        tenant_id,
                        connection_id,
                        integration_id,
                        parameters,
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
                (response.status, response.body)
            }
            plain => {
                let request = client
                    .post(url)
                    .header("content-type", content_type)
                    .body(body);
                let request = match plain {
                    Self::Bearer(token) => request.bearer_auth(token),
                    Self::Basic { username, password } => {
                        request.basic_auth(username, Some(password))
                    }
                    Self::InPath(_) => request,
                    Self::Header(authorization) => request.header("Authorization", authorization),
                    Self::Sealed { .. } => unreachable!("handled above"),
                };
                // The URL may hold the token, and reqwest's errors name it.
                let response = request.send().await.map_err(|e| e.without_url())?;
                let status = response.status().as_u16();
                let bytes = response.bytes().await.map_err(|e| e.without_url())?;
                (status, bytes.to_vec())
            }
        };
        Ok((status, bytes))
    }
}

// ---------------------------------------------------------------------------
// Telegram
// ---------------------------------------------------------------------------

pub struct TelegramChannel {
    auth: ReplyAuth,
    api_base: String,
    client: reqwest::Client,
}

impl TelegramChannel {
    pub fn new(auth: ReplyAuth, client: reqwest::Client) -> Self {
        Self::with_api_base(
            auth,
            runtara_connections::sealed::policy::TELEGRAM_API_BASE.to_string(),
            client,
        )
    }

    /// A channel whose Bot API is at `api_base`.
    pub fn with_api_base(auth: ReplyAuth, api_base: String, client: reqwest::Client) -> Self {
        Self {
            auth,
            api_base,
            client,
        }
    }
}

/// The URL of a Bot API method. Telegram takes the token in the path; seal
/// puts a sealed one there itself, so the URL then names the method only.
pub fn telegram_method_url(api_base: &str, auth: &ReplyAuth, method: &str) -> String {
    let api_base = api_base.trim_end_matches('/');
    match auth {
        ReplyAuth::InPath(token) => format!("{api_base}/bot{token}/{method}"),
        _ => format!("{api_base}/{method}"),
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

            let url = telegram_method_url(&self.api_base, &self.auth, "sendMessage");

            // Telegram has a 4096 char limit per message. Split if needed.
            for chunk in split_message(&text, 4096) {
                let request = json!({
                    "chat_id": conversation_id,
                    "text": chunk,
                });
                let (status, body) = self
                    .auth
                    .post_json(
                        &self.client,
                        &url,
                        "application/json",
                        serde_json::to_vec(&request)?,
                    )
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
    client: reqwest::Client,
}

impl SlackChannel {
    pub fn new(auth: ReplyAuth, client: reqwest::Client) -> Self {
        Self { auth, client }
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
                        &self.client,
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
    /// Hardened egress client (no redirects + DNS guard).
    client: reqwest::Client,
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
        client: reqwest::Client,
        service_urls: std::sync::Arc<dashmap::DashMap<(String, String), String>>,
    ) -> Self {
        Self {
            tenant_id,
            connection_id,
            params,
            facade,
            client,
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
            // A sealed app password gets a sealed token: seal attaches it and
            // sends the reply. Otherwise the token is this process's to send.
            let auth =
                match resolved.sealed_parameters {
                    Some(parameters) => ReplyAuth::Sealed {
                        connections: self.facade.clone(),
                        tenant_id: self.tenant_id.clone(),
                        connection_id: self.connection_id.clone(),
                        integration_id: "teams_bot".to_string(),
                        parameters,
                    },
                    None => ReplyAuth::Header(headers.get("Authorization").cloned().ok_or_else(
                        || anyhow::anyhow!("Teams auth did not yield a bearer token"),
                    )?),
                };

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
                    .post(
                        &self.client,
                        &url,
                        "application/json",
                        serde_json::to_vec(&activity)?,
                    )
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
    client: reqwest::Client,
}

impl MailgunChannel {
    pub fn new(auth: ReplyAuth, domain: String, region: String, client: reqwest::Client) -> Self {
        Self {
            auth,
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

            let form = url::form_urlencoded::Serializer::new(String::new())
                .append_pair("from", &from)
                .append_pair("to", &recipient)
                .append_pair("subject", "Re: Your message")
                .append_pair("text", &text)
                .finish();
            let (_, body) = self
                .auth
                .post_json(
                    &self.client,
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
                sealed,
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
            &reqwest::Client::new(),
            &format!("{}/api/reply", server.uri()),
            "application/json",
            b"{\"text\":\"hi\"}".to_vec(),
        )
        .await
    }

    use serde_json::Value;

    #[tokio::test]
    async fn a_reply_carries_a_readable_credential_as_it_always_did() {
        let server = provider("Bearer xoxb-token").await;
        let (status, body) = reply(&ReplyAuth::Bearer("xoxb-token".into()), &server)
            .await
            .unwrap();
        assert_eq!((status, body["ok"].as_bool()), (200, Some(true)));

        // "api:key-1", as Mailgun takes it.
        let server = provider("Basic YXBpOmtleS0x").await;
        let auth = ReplyAuth::Basic {
            username: "api".into(),
            password: "key-1".into(),
        };
        assert_eq!(reply(&auth, &server).await.unwrap().0, 200);
    }

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
                ReplyAuth::Bearer,
            )
            .unwrap()
        };

        let auth = of(&facade(Some(sealed)));
        assert!(matches!(auth, ReplyAuth::Sealed { .. }));
        let (status, body) = reply(&auth, &server).await.unwrap();
        assert_eq!((status, body["ok"].as_bool()), (200, Some(true)));

        // Without seal the reply is not sent, with or without a credential.
        let error = reply(&of(&facade(None)), &server).await.unwrap_err();
        assert!(error.to_string().contains("sealed"), "{error}");
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
    async fn a_telegram_reply_names_the_bot_in_the_path() {
        let server = telegram("1:readable").await;
        let channel = TelegramChannel::with_api_base(
            ReplyAuth::InPath("1:readable".into()),
            server.uri(),
            reqwest::Client::new(),
        );
        channel.send_text("42", "hi").await.unwrap();
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
            ReplyAuth::InPath,
        )
        .unwrap();
        assert!(matches!(auth, ReplyAuth::Sealed { .. }));
        // The channel writes no token: the URL names the method only.
        assert_eq!(
            telegram_method_url(&server.uri(), &auth, "sendMessage"),
            format!("{}/sendMessage", server.uri())
        );
        let channel = TelegramChannel::with_api_base(auth, server.uri(), reqwest::Client::new());
        channel.send_text("42", "hi").await.unwrap();
    }

    #[tokio::test]
    async fn a_connection_without_the_credential_has_no_reply_auth() {
        let missing = ReplyAuth::of(
            &facade(None),
            "acme",
            "c-1",
            "slack_bot",
            &json!({}),
            "bot_token",
            ReplyAuth::Bearer,
        );
        assert!(missing.is_err());
        let readable = ReplyAuth::of(
            &facade(None),
            "acme",
            "c-1",
            "slack_bot",
            &json!({"bot_token": "xoxb"}),
            "bot_token",
            ReplyAuth::Bearer,
        );
        assert!(matches!(readable, Ok(ReplyAuth::Bearer(token)) if token == "xoxb"));
    }
}
