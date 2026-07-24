use std::sync::Arc;

use dashmap::DashMap;
use redis::aio::ConnectionManager;
use runtara_management_sdk::ListEventsOptions;
use serde_json::{Value, json};
use sqlx::PgPool;
use tokio::sync::mpsc;
use tokio::time::{Duration, sleep};
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use crate::api::dto::triggers::TriggerType;
use crate::api::handlers::chat::{ChatEvent, chat_event_type, parse_debug_event};
use crate::api::repositories::triggers::TriggerRepository;
use crate::api::services::{session_queue, session_token};
use crate::runtime_client::RuntimeClient;
use crate::workers::execution_engine::{ExecutionEngine, QueueRequest, TriggerSource};
use runtara_connections::ConnectionsFacade;

use super::channel::{Channel, TelegramChannel};
use super::collector;

/// A normalized inbound message from any channel.
#[derive(Debug, Clone)]
pub struct InboundMessage {
    /// Plain text content (used for WaitForSignal delivery and session queue).
    pub text: String,
    /// Sender identity (used for session keying in per_sender mode).
    pub sender_id: String,
    /// Platform conversation ID (used for sending replies).
    pub conv_id: String,
    /// Channel platform identifier (e.g. "telegram", "slack", "mailgun").
    pub channel: String,
    /// Normalized attachments.
    pub attachments: Vec<Attachment>,
    /// Raw platform-specific payload (email headers, Slack event, etc.).
    pub original_message: Value,
    /// Curated, credential-free reply target exposed to the workflow as
    /// `data.target` (Teams: opaque endpoint ref + conversation identifiers).
    /// `None` for channels that don't produce one.
    pub target: Option<Value>,
    /// Provider activity/message id, used as the idempotency key for the
    /// deterministic instance id (redelivery dedup). `None` when unavailable.
    pub activity_id: Option<String>,
}

/// A normalized attachment from any channel.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Attachment {
    /// Filename (e.g. "invoice.pdf").
    pub name: String,
    /// MIME type (e.g. "application/pdf").
    #[serde(rename = "type")]
    pub content_type: String,
    /// Size in bytes.
    pub size: u64,
    /// URL to download the attachment (platform-specific).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Base64-encoded content (for small inline attachments).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<String>,
    /// Internal S3 storage bucket (set when attachment is persisted to tenant storage).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub storage_bucket: Option<String>,
    /// Internal S3 storage key (set when attachment is persisted to tenant storage).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub storage_key: Option<String>,
}

/// Session key: (connection_id, trigger_id, discriminator).
/// The discriminator varies based on session_mode:
/// - per_sender: sender identity (chat_id, user_id, email)
/// - per_trigger: "shared" (everyone shares one session)
/// - per_message: random UUID (no session continuity)
type SessionKey = (String, String, String);

/// Routes incoming channel messages to the right session.
///
/// Looks up connection + trigger from DB to determine org_id, workflow_id,
/// and bot credentials. Each active conversation gets its own session actor.
pub struct ChannelRouter {
    sessions: Arc<DashMap<SessionKey, mpsc::Sender<InboundMessage>>>,
    client: Arc<RuntimeClient>,
    pool: PgPool,
    connections: Arc<ConnectionsFacade>,
    engine: Arc<ExecutionEngine>,
    valkey: ConnectionManager,
    http_client: reqwest::Client,
    /// Hardened egress client (no redirects + DNS guard) for credentialed
    /// channel replies (Teams).
    hardened_client: reqwest::Client,
    /// Shared, live service-URL map for Teams, keyed by
    /// `(connection_id, conversation_id)`. Handed to every `TeamsChannel` by
    /// Arc clone (not snapshot) so replies can always resolve a serviceUrl that
    /// arrived after the session started.
    teams_service_urls: Arc<DashMap<(String, String), String>>,
}

impl ChannelRouter {
    pub fn new(
        client: Arc<RuntimeClient>,
        pool: PgPool,
        connections: Arc<ConnectionsFacade>,
        engine: Arc<ExecutionEngine>,
        valkey: ConnectionManager,
    ) -> Self {
        Self {
            sessions: Arc::new(DashMap::new()),
            client,
            pool,
            connections,
            engine,
            valkey,
            http_client: reqwest::Client::new(),
            hardened_client: runtara_connections::net::build_hardened_client(),
            teams_service_urls: Arc::new(DashMap::new()),
        }
    }

    /// Access the database pool (used by platform-specific webhook handlers).
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// Access the connections facade.
    pub fn connections(&self) -> &Arc<ConnectionsFacade> {
        &self.connections
    }

    /// Access the shared HTTP client (used for downloading external resources).
    pub fn http_client(&self) -> &reqwest::Client {
        &self.http_client
    }

    /// Store a Teams service URL for a `(connection, conversation)`.
    pub fn set_teams_service_url(
        &self,
        connection_id: &str,
        conversation_id: &str,
        service_url: &str,
    ) {
        self.teams_service_urls.insert(
            (connection_id.to_string(), conversation_id.to_string()),
            service_url.to_string(),
        );
    }

    /// Drop a Teams service URL, e.g. when the bot is uninstalled/removed from a
    /// conversation so a stale reference is not left behind.
    pub fn remove_teams_service_url(&self, connection_id: &str, conversation_id: &str) {
        self.teams_service_urls
            .remove(&(connection_id.to_string(), conversation_id.to_string()));
    }

    /// Reserve an inbound activity for processing, deduplicating at-least-once
    /// redeliveries. Returns `true` when this delivery is fresh and should be
    /// processed, `false` when it is a duplicate. Fails open on Valkey errors.
    pub async fn reserve_activity(&self, connection_id: &str, activity_id: &str) -> bool {
        // 4-hour window: covers Teams' ~15s retry, a valid Bot Framework token's
        // ~1h life, AND slower out-of-band redeliveries. The key is one small
        // Valkey entry per activity, so a generous TTL is cheap insurance
        // against a duplicate session; it comfortably outlives any legitimate
        // redelivery of the same activity id.
        const DEDUP_TTL_SECS: i64 = 4 * 3600;
        let identity = Self::activity_identity(connection_id, activity_id);
        let mut valkey = self.valkey.clone();
        session_queue::reserve_activity_dedup(&mut valkey, &identity, DEDUP_TTL_SECS).await
    }

    /// Release a dedup reservation after processing FAILED, so a redelivery of
    /// the same activity is not silently dropped as a duplicate. Best-effort.
    pub async fn release_activity(&self, connection_id: &str, activity_id: &str) {
        let identity = Self::activity_identity(connection_id, activity_id);
        let mut valkey = self.valkey.clone();
        session_queue::release_activity_dedup(&mut valkey, &identity).await;
    }

    /// The dedup identity for an inbound activity. Reserve and release MUST
    /// agree on this format.
    fn activity_identity(connection_id: &str, activity_id: &str) -> String {
        format!(
            "{}:{connection_id}:{activity_id}",
            crate::config::tenant_id()
        )
    }

    /// Validate the webhook secret from the request header against the
    /// secret stored in the trigger's configuration.
    pub async fn validate_webhook_secret(
        &self,
        connection_id: &str,
        secret_header: Option<&str>,
    ) -> anyhow::Result<()> {
        let expected_tenant = crate::config::tenant_id();
        let trigger_repo = TriggerRepository::new(self.pool.clone());
        let triggers = trigger_repo
            .list(Some(expected_tenant))
            .await
            .map_err(|e| anyhow::anyhow!("DB error: {}", e))?;

        let trigger = triggers.iter().find(|t| {
            t.trigger_type == TriggerType::Channel
                && t.active
                && t.configuration
                    .as_ref()
                    .and_then(|c| c.get("connection_id"))
                    .and_then(|v| v.as_str())
                    == Some(connection_id)
        });

        let Some(trigger) = trigger else {
            anyhow::bail!("No active Channel trigger for connection {}", connection_id);
        };

        let stored_secret = trigger
            .configuration
            .as_ref()
            .and_then(|c| c.get("webhook_secret"))
            .and_then(|v| v.as_str());

        match (stored_secret, secret_header) {
            (Some(stored), Some(header)) if stored == header => Ok(()),
            (Some(_), Some(_)) => anyhow::bail!("Invalid webhook secret"),
            (Some(_), None) => anyhow::bail!("Missing webhook secret header"),
            // No secret stored (legacy trigger) — allow for backward compatibility.
            (None, _) => Ok(()),
        }
    }

    /// Handle an inbound message from a platform conversation.
    ///
    /// Looks up the connection to get org_id + bot token, then finds
    /// the Channel trigger to get workflow_id. Creates or routes to
    /// an existing session.
    pub async fn handle_message(
        &self,
        connection_id: &str,
        msg: &InboundMessage,
    ) -> anyhow::Result<()> {
        let conv_id = &msg.conv_id;
        let sender_id = &msg.sender_id;
        // Look up connection from DB.
        let conn = self
            .connections
            .get_channel_connection(connection_id)
            .await
            .map_err(|e| anyhow::anyhow!("DB error: {}", e))?
            .ok_or_else(|| anyhow::anyhow!("Connection not found: {}", connection_id))?;

        let tenant_id = conn
            .tenant_id
            .ok_or_else(|| anyhow::anyhow!("Connection has no tenant_id"))?;

        let expected_tenant = crate::config::tenant_id();
        if tenant_id != expected_tenant {
            anyhow::bail!("Connection tenant mismatch");
        }

        // Find the Channel trigger for this connection.
        let trigger_repo = TriggerRepository::new(self.pool.clone());
        let triggers = trigger_repo
            .list(Some(&tenant_id))
            .await
            .map_err(|e| anyhow::anyhow!("DB error: {}", e))?;

        let trigger = triggers
            .iter()
            .find(|t| {
                t.trigger_type == TriggerType::Channel
                    && t.active
                    && t.configuration
                        .as_ref()
                        .and_then(|c| c.get("connection_id"))
                        .and_then(|v| v.as_str())
                        == Some(connection_id)
            })
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "No active Channel trigger found for connection {}",
                    connection_id
                )
            })?;

        let trigger_id = trigger.id.clone();
        let workflow_id = trigger.workflow_id.clone();

        // Determine session mode from trigger configuration.
        let session_mode = trigger
            .configuration
            .as_ref()
            .and_then(|c| c.get("session_mode"))
            .and_then(|v| v.as_str())
            .unwrap_or("per_sender")
            .to_string();

        let discriminator = match session_mode.as_str() {
            "per_trigger" => "shared".to_string(),
            "per_message" => Uuid::new_v4().to_string(),
            _ => sender_id.to_string(), // per_sender (default)
        };

        let key = (connection_id.to_string(), trigger_id, discriminator);

        // Try sending to an existing session (not applicable for per_message).
        if session_mode != "per_message"
            && let Some(tx) = self.sessions.get(&key)
        {
            if tx.send(msg.clone()).await.is_ok() {
                return Ok(());
            }
            drop(tx);
            self.sessions.remove(&key);
        }

        // Build the channel adapter from connection credentials.
        let integration_id = conn.integration_id.as_deref().unwrap_or("");
        let params = conn
            .connection_parameters
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Connection has no parameters"))?;

        let channel: Arc<dyn Channel> = match integration_id {
            "telegram_bot" => {
                let bot_token = params["bot_token"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("Missing bot_token in connection"))?;
                Arc::new(TelegramChannel::new(
                    bot_token.to_string(),
                    self.http_client.clone(),
                ))
            }
            "slack_bot" => {
                let bot_token = params["bot_token"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("Missing bot_token in connection"))?;
                Arc::new(super::channel::SlackChannel::new(
                    bot_token.to_string(),
                    self.http_client.clone(),
                ))
            }
            "teams_bot" => {
                // The adapter mints its token through the facade (correct
                // single-tenant authority + shared token cache) and egresses
                // via the hardened client — no raw secret handling here. It gets
                // an Arc clone of the LIVE serviceUrl map (not a snapshot) so a
                // serviceUrl that arrives after session start still resolves.
                Arc::new(super::channel::TeamsChannel::new(
                    tenant_id.clone(),
                    connection_id.to_string(),
                    params.clone(),
                    self.connections.clone(),
                    self.hardened_client.clone(),
                    self.teams_service_urls.clone(),
                ))
            }
            "mailgun" => {
                let api_key = params["api_key"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("Missing api_key in connection"))?;
                let domain = params["domain"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("Missing domain in connection"))?;
                let region = params["region"].as_str().unwrap_or("us");
                Arc::new(super::channel::MailgunChannel::new(
                    api_key.to_string(),
                    domain.to_string(),
                    region.to_string(),
                    self.http_client.clone(),
                ))
            }
            other => anyhow::bail!("Unsupported channel connection type: {}", other),
        };

        // Create session. Don't push the first message to the mpsc channel —
        // it's already included in the execution inputs via initial_message.
        // The mpsc channel is only for subsequent messages in the session.
        let (tx, rx) = mpsc::channel::<InboundMessage>(32);
        self.sessions.insert(key.clone(), tx);

        let client = self.client.clone();
        let engine = self.engine.clone();
        let valkey = self.valkey.clone();
        let sessions = self.sessions.clone();
        let conv_id = conv_id.to_string();
        let initial_message = msg.clone();

        tokio::spawn(async move {
            info!(
                conv_id = %conv_id,
                workflow_id = %workflow_id,
                session_mode = %session_mode,
                "Channel session starting"
            );
            if let Err(e) = session_loop(
                channel,
                &conv_id,
                initial_message,
                rx,
                client,
                engine,
                valkey,
                &tenant_id,
                &workflow_id,
                &session_mode,
            )
            .await
            {
                warn!(conv_id = %conv_id, error = %e, "Channel session ended with error");
            } else {
                info!(conv_id = %conv_id, "Channel session ended normally");
            }
            sessions.remove(&key);
        });

        Ok(())
    }
}

// ===========================================================================
// Session loop (unchanged logic, now connection-driven)
// ===========================================================================

#[allow(clippy::too_many_arguments)]
async fn session_loop(
    channel: Arc<dyn Channel>,
    conv_id: &str,
    initial_message: InboundMessage,
    mut user_rx: mpsc::Receiver<InboundMessage>,
    client: Arc<RuntimeClient>,
    engine: Arc<ExecutionEngine>,
    mut valkey: ConnectionManager,
    org_id: &str,
    workflow_id: &str,
    session_mode: &str,
) -> anyhow::Result<()> {
    // conv_id tracks the current conversation target (channel/thread).
    // Updated when subsequent messages arrive from a different channel,
    // so responses always go where the sender is currently messaging.
    let mut conv_id = conv_id.to_string();

    let session_id = Uuid::new_v4().to_string();

    let _token = session_token::sign(org_id, workflow_id, &session_id)
        .map_err(|e| anyhow::anyhow!("Failed to sign session token: {}", e))?;

    // Queue first execution with the full inbound message data.
    let attachments_json: Vec<Value> = initial_message
        .attachments
        .iter()
        .map(|a| serde_json::to_value(a).unwrap_or_default())
        .collect();

    let mut data = json!({
        "sessionId": &session_id,
        "channel": &initial_message.channel,
        "userMessage": &initial_message.text,
        "attachments": attachments_json,
        "originalMessage": &initial_message.original_message,
    });
    if let Some(target) = &initial_message.target {
        data["target"] = target.clone();
    }
    let inputs = json!({ "data": data, "variables": {} });

    // Deterministic instance id from the provider activity id: if the same
    // activity is redelivered after the Valkey dedup window is lost, the
    // environment dedups the start by instance id and won't double-fire. The
    // namespace is parameterized by channel so activity ids from different
    // channels can never collide into the same instance id.
    let deterministic_instance_id = initial_message.activity_id.as_deref().map(|activity_id| {
        Uuid::new_v5(
            &Uuid::NAMESPACE_URL,
            format!(
                "channel-activity:{}:{org_id}:{activity_id}",
                initial_message.channel
            )
            .as_bytes(),
        )
    });

    let result = engine
        .queue(QueueRequest {
            tenant_id: org_id,
            workflow_id,
            version: None,
            inputs,
            debug: false,
            correlation_id: None,
            trigger_source: TriggerSource::Webhook,
            instance_id: deterministic_instance_id,
        })
        .await
        .map_err(|e| anyhow::anyhow!("Failed to queue execution: {:?}", e))?;

    let mut instance_id = result.instance_id.to_string();

    let _ = session_queue::set_session_meta(
        &mut valkey,
        org_id,
        &session_id,
        &instance_id,
        workflow_id,
    )
    .await;

    info!(
        conv_id = %conv_id,
        session_id = %session_id,
        instance_id = %instance_id,
        "Channel session created"
    );

    sleep(Duration::from_millis(500)).await;

    let poll_interval = Duration::from_millis(300);
    let idle_poll_interval = Duration::from_millis(500);
    let max_duration = Duration::from_secs(600);
    let start_time = std::time::Instant::now();
    let mut session_ended = false;
    // The pending signal payload for the current instance. For the first
    // instance this is built from initial_message. For subsequent instances
    // (started via idle phase), it is built from the queued message.
    let mut pending_signal_payload: Value = json!({
        "message": &initial_message.text,
        "attachments": &attachments_json,
        "originalMessage": &initial_message.original_message,
    });
    if let Some(target) = &initial_message.target {
        pending_signal_payload["target"] = target.clone();
    }

    while !session_ended && start_time.elapsed() < max_duration {
        // === INSTANCE LOOP ===
        let mut event_offset: u32 = 0;
        let mut instance_done = false;
        let mut waiting_for_input = false;
        let mut first_signal_handled = false;
        // Whether THIS session owns the instance it is polling. Decided once, on
        // the first poll that carries the instance's persisted input, by
        // comparing its `data.sessionId` to ours (see `classify_ownership`).
        // Reset per instance-loop iteration so an idle-requeued instance (which
        // reuses our session_id) re-derives ownership. A foreign owner means a
        // duplicate session landed on a redelivered activity's instance; it must
        // suppress dispatch instead of re-flushing the owner's transcript.
        let mut owns_instance: Option<bool> = None;

        while !instance_done && !session_ended && start_time.elapsed() < max_duration {
            tokio::select! {
                _ = sleep(poll_interval) => {
                    let info_result = client.get_instance_info(&instance_id).await;

                    // Decide ownership once, on the first poll that carries the
                    // instance's persisted input. Never decide on the
                    // pre-registration NotFound ticks (no input, no events yet).
                    if owns_instance.is_none()
                        && let Ok(info) = &info_result
                        && let Some(input) = info.input.as_ref()
                    {
                        let owned = classify_ownership(Some(input), &session_id);
                        owns_instance = Some(owned);
                        if !owned {
                            debug!(
                                instance_id = %instance_id,
                                session_id = %session_id,
                                "Foreign-owned instance (redelivery); suppressing channel dispatch"
                            );
                        }
                    }
                    let foreign = owns_instance == Some(false);

                    match info_result {
                        Ok(info) if info.status.is_terminal() => {
                            // A foreign-owned instance was already flushed by its
                            // owning session; re-flushing from offset 0 would
                            // re-send the entire reply transcript. Suppress both
                            // the flush and the Failed notice, and drop to the
                            // idle phase so per_sender/per_trigger sessions stay
                            // alive for genuinely new turns.
                            if !foreign {
                                flush_events(
                                    &client, &channel, &conv_id, &instance_id,
                                    &mut event_offset, &mut user_rx,
                                ).await;

                                if let runtara_management_sdk::InstanceStatus::Failed = info.status {
                                    let msg = info.error.or(info.stderr)
                                        .unwrap_or_else(|| "Execution failed".to_string());
                                    warn!(conv_id = %conv_id, error = %msg, "Instance failed");
                                    let _ = channel.send_text(&conv_id, "Sorry, something went wrong. Please try again.").await;
                                }
                            }

                            instance_done = true;
                            continue;
                        }
                        Err(e) if start_time.elapsed() > Duration::from_secs(30) => {
                            error!(error = %e, "Instance polling failed");
                            let _ = channel.send_text(&conv_id, "Error: lost connection to runtime").await;
                            session_ended = true;
                            continue;
                        }
                        _ => {}
                    }

                    // Running/streaming dispatch only when we OWN the instance.
                    // Undecided (input not yet readable) or foreign ⇒ skip this
                    // tick rather than streaming a foreign instance's events (or
                    // dispatching from offset 0 before ownership is known).
                    if owns_instance != Some(true) {
                        continue;
                    }

                    let options = ListEventsOptions {
                        event_type: Some("custom".to_string()),
                        sort_order: Some(runtara_management_sdk::EventSortOrder::Asc),
                        limit: Some(100),
                        offset: Some(event_offset),
                        ..Default::default()
                    };

                    if let Ok(result) = client.list_events(&instance_id, Some(options)).await {
                        for event in result.events {
                            if let Some(payload) = &event.payload {
                                let subtype = event.subtype.as_deref();

                                if subtype == Some("external_input_requested") {
                                    let has_complex_schema = payload.get("response_schema")
                                        .map(|v| !v.is_null() && !is_simple_schema(v))
                                        .unwrap_or(false);

                                    if has_complex_schema {
                                        waiting_for_input = true;
                                        dispatch_event(
                                            subtype, payload, &channel, &conv_id,
                                            &instance_id, &mut user_rx, &client,
                                        ).await;
                                    } else {
                                        let signal_id = payload.get("signal_id")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or("")
                                            .to_string();

                                        match session_queue::pop_event(&mut valkey, org_id, &session_id).await {
                                            Ok(Some(queued)) => {
                                                let bytes = serde_json::to_vec(&queued).unwrap_or_default();
                                                if client.send_custom_signal(&instance_id, &signal_id, Some(&bytes)).await.is_ok() {
                                                    waiting_for_input = false;
                                                } else {
                                                    waiting_for_input = true;
                                                    dispatch_event(subtype, payload, &channel, &conv_id, &instance_id, &mut user_rx, &client).await;
                                                }
                                            }
                                            _ if !first_signal_handled => {
                                                // First WaitForSignal (no schema) with empty queue:
                                                // use the pending signal payload (set from initial_message
                                                // or from the idle-phase queued message for subsequent instances).
                                                debug!(
                                                    instance_id = %instance_id,
                                                    signal_id = %signal_id,
                                                    "Delivering pending message via signal (first WaitForSignal, empty queue)"
                                                );
                                                first_signal_handled = true;
                                                let bytes = serde_json::to_vec(&pending_signal_payload).unwrap_or_default();
                                                if client.send_custom_signal(&instance_id, &signal_id, Some(&bytes)).await.is_ok() {
                                                    waiting_for_input = false;
                                                } else {
                                                    waiting_for_input = true;
                                                    dispatch_event(subtype, payload, &channel, &conv_id, &instance_id, &mut user_rx, &client).await;
                                                }
                                            }
                                            _ => {
                                                waiting_for_input = true;
                                                dispatch_event(subtype, payload, &channel, &conv_id, &instance_id, &mut user_rx, &client).await;
                                            }
                                        }
                                    }
                                } else {
                                    dispatch_event(subtype, payload, &channel, &conv_id, &instance_id, &mut user_rx, &client).await;
                                }
                            }
                            event_offset += 1;
                        }

                        if waiting_for_input
                            && let Ok(Some(queued)) = session_queue::pop_event(&mut valkey, org_id, &session_id).await
                            && let Some(sig) = find_pending_signal_id(&client, &instance_id).await
                        {
                            let bytes = serde_json::to_vec(&queued).unwrap_or_default();
                            if client.send_custom_signal(&instance_id, &sig, Some(&bytes)).await.is_ok() {
                                waiting_for_input = false;
                            }
                        }
                    }
                }

                Some(inbound) = user_rx.recv() => {
                    // Update conv_id so responses go where the sender is now.
                    conv_id = inbound.conv_id.clone();
                    let attachments_json: Vec<Value> = inbound.attachments.iter()
                        .map(|a| serde_json::to_value(a).unwrap_or_default())
                        .collect();
                    let mut event = json!({
                        "message": inbound.text,
                        "attachments": attachments_json,
                        "originalMessage": inbound.original_message,
                    });
                    if let Some(target) = &inbound.target {
                        event["target"] = target.clone();
                    }
                    if let Err(e) = session_queue::push_event(&mut valkey, org_id, &session_id, &event).await {
                        warn!(error = %e, "Failed to push user message to queue");
                    }
                }
            }
        }

        // === IDLE PHASE ===
        // For per_message mode, skip idle — one instance per message, then exit.
        if instance_done && !session_ended && session_mode == "per_message" {
            session_ended = true;
        }

        if instance_done && !session_ended {
            debug!(session_id = %session_id, "Instance done, waiting for next message");

            loop {
                if start_time.elapsed() >= max_duration {
                    session_ended = true;
                    break;
                }

                tokio::select! {
                    _ = sleep(idle_poll_interval) => {
                        if let Ok(true) = session_queue::has_events(&mut valkey, org_id, &session_id).await {
                            // Pop the message from the queue so it's not re-processed.
                            // The message content will be delivered to the new instance
                            // via the queue-drain bridge if it has WaitForSignal.
                            // For workflows without WaitForSignal, the message was already
                            // handled by the webhook handler.
                            let queued_msg = session_queue::pop_event(&mut valkey, org_id, &session_id).await.ok().flatten();
                            let user_message = queued_msg.as_ref()
                                .and_then(|m| m.get("message"))
                                .and_then(|m| m.as_str())
                                .unwrap_or("");
                            let queued_attachments = queued_msg.as_ref()
                                .and_then(|m| m.get("attachments"))
                                .cloned()
                                .unwrap_or(json!([]));
                            let queued_original = queued_msg.as_ref()
                                .and_then(|m| m.get("originalMessage"))
                                .cloned()
                                .unwrap_or(Value::Null);
                            let queued_target = queued_msg.as_ref()
                                .and_then(|m| m.get("target"))
                                .cloned();
                            let mut requeue_data = json!({
                                "sessionId": &session_id,
                                "channel": &initial_message.channel,
                                "userMessage": user_message,
                                "attachments": queued_attachments,
                                "originalMessage": queued_original,
                            });
                            if let Some(target) = queued_target {
                                requeue_data["target"] = target;
                            }
                            let inputs = json!({ "data": requeue_data, "variables": {} });
                            match engine.queue(QueueRequest {
                                tenant_id: org_id,
                                workflow_id,
                                version: None,
                                inputs,
                                debug: false,
                                correlation_id: None,
                                trigger_source: TriggerSource::Webhook,
                                instance_id: None,
                            }).await {
                                Ok(result) => {
                                    instance_id = result.instance_id.to_string();
                                    let _ = session_queue::set_session_meta(
                                        &mut valkey, org_id, &session_id, &instance_id, workflow_id,
                                    ).await;
                                    // Update pending signal payload for the new instance
                                    // so the WaitForSignal handler delivers this message.
                                    if let Some(msg) = queued_msg {
                                        pending_signal_payload = msg;
                                    }
                                    info!(instance_id = %instance_id, "New instance for channel session");
                                    sleep(Duration::from_millis(500)).await;
                                    break;
                                }
                                Err(e) => {
                                    error!(error = ?e, "Failed to start new instance");
                                    let _ = channel.send_text(&conv_id, "Error: failed to start new conversation instance").await;
                                    session_ended = true;
                                    break;
                                }
                            }
                        }
                    }

                    Some(inbound) = user_rx.recv() => {
                        conv_id = inbound.conv_id.clone();
                        let attachments_json: Vec<Value> = inbound.attachments.iter()
                            .map(|a| serde_json::to_value(a).unwrap_or_default())
                            .collect();
                        let mut event = json!({
                            "message": inbound.text,
                            "attachments": attachments_json,
                            "originalMessage": inbound.original_message,
                        });
                        if let Some(target) = &inbound.target {
                            event["target"] = target.clone();
                        }
                        let _ = session_queue::push_event(&mut valkey, org_id, &session_id, &event).await;
                    }
                }
            }
        }
    }

    if start_time.elapsed() >= max_duration {
        debug!(conv_id = %conv_id, "Channel session timed out");
    }

    Ok(())
}

async fn flush_events(
    client: &Arc<RuntimeClient>,
    channel: &Arc<dyn Channel>,
    conv_id: &str,
    instance_id: &str,
    event_offset: &mut u32,
    user_rx: &mut mpsc::Receiver<InboundMessage>,
) {
    let options = ListEventsOptions {
        event_type: Some("custom".to_string()),
        sort_order: Some(runtara_management_sdk::EventSortOrder::Asc),
        limit: Some(100),
        offset: Some(*event_offset),
        ..Default::default()
    };

    if let Ok(result) = client.list_events(instance_id, Some(options)).await {
        for event in result.events {
            if let Some(payload) = &event.payload {
                dispatch_event(
                    event.subtype.as_deref(),
                    payload,
                    channel,
                    conv_id,
                    instance_id,
                    user_rx,
                    client,
                )
                .await;
            }
            *event_offset += 1;
        }
    }
}

async fn dispatch_event(
    subtype: Option<&str>,
    payload: &Value,
    channel: &Arc<dyn Channel>,
    conv_id: &str,
    instance_id: &str,
    user_rx: &mut mpsc::Receiver<InboundMessage>,
    client: &Arc<RuntimeClient>,
) {
    let chat_events = parse_debug_event(subtype, payload);

    for chat_event in chat_events {
        match &chat_event {
            ChatEvent::Message { content, .. } if !content.is_empty() => {
                if let Err(e) = channel.send_text(conv_id, content).await {
                    warn!(conv_id = %conv_id, error = %e, "Failed to send channel reply");
                }
            }

            ChatEvent::WaitingForInput {
                signal_id,
                message,
                response_schema,
                ..
            } => {
                let needs_prompting = response_schema
                    .as_ref()
                    .map(|s| !s.is_null() && !is_simple_schema(s))
                    .unwrap_or(false);

                // Only send the prompt message for structured schemas.
                // For simple/null schemas, the user's message is auto-delivered
                // from the queue — no need to prompt.
                if needs_prompting && let Err(e) = channel.send_text(conv_id, message).await {
                    warn!(conv_id = %conv_id, error = %e, "Failed to send input prompt");
                }

                if let Some(schema) = response_schema
                    && !schema.is_null()
                    && !is_simple_schema(schema)
                {
                    match collector::collect_fields(schema, channel.as_ref(), conv_id, user_rx)
                        .await
                    {
                        Ok(payload) => {
                            if let Err(e) = client
                                .send_custom_signal(
                                    instance_id,
                                    signal_id,
                                    Some(&serde_json::to_vec(&payload).unwrap_or_default()),
                                )
                                .await
                            {
                                warn!(error = %e, "Failed to submit signal");
                            }
                        }
                        Err(e) => {
                            warn!(error = %e, "Field collection failed");
                            let _ = client
                                .send_custom_signal(
                                    instance_id,
                                    signal_id,
                                    Some(&serde_json::to_vec(&json!({})).unwrap_or_default()),
                                )
                                .await;
                        }
                    }
                }
            }

            ChatEvent::Error { message } => {
                warn!(conv_id = %conv_id, error = %message, "Workflow error");
                if let Err(e) = channel
                    .send_text(conv_id, "Sorry, something went wrong. Please try again.")
                    .await
                {
                    warn!(conv_id = %conv_id, error = %e, "Failed to send error notice");
                }
            }

            _ => {
                debug!(event_type = %chat_event_type(&chat_event), "Channel: ignoring internal event");
            }
        }
    }
}

async fn find_pending_signal_id(client: &Arc<RuntimeClient>, instance_id: &str) -> Option<String> {
    let options = ListEventsOptions::new()
        .with_limit(10)
        .with_event_type("custom")
        .with_subtype("external_input_requested")
        .with_sort_order(runtara_management_sdk::EventSortOrder::Desc);

    let result = client.list_events(instance_id, Some(options)).await.ok()?;
    result
        .events
        .first()
        .and_then(|ev| ev.payload.as_ref())
        .and_then(|p| p.get("signal_id"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

fn is_simple_schema(schema: &Value) -> bool {
    if schema.is_null() {
        return true;
    }
    let Some(obj) = schema.as_object() else {
        return true;
    };
    if obj.is_empty() {
        return true;
    }
    if obj.len() == 1
        && let Some(field) = obj.get("message")
    {
        return field.get("type").and_then(|t| t.as_str()) == Some("string")
            && field.get("enum").is_none()
            && field.get("format").is_none();
    }
    false
}

/// Decide whether THIS session owns the instance it is polling, by comparing the
/// instance's persisted start input against the session's own id.
///
/// A channel session embeds `data.sessionId` into its workflow start inputs, and
/// the runtime persists the input of the ONE session that won the
/// (deterministic-instance-id) start-or-attach race. A duplicate session — one
/// that spawned for a redelivered activity after the Valkey dedup was lost —
/// lands on that same instance and reads a *foreign* `data.sessionId`; it must
/// suppress dispatch instead of re-flushing the owner's whole reply transcript.
///
/// Returns `true` (own → dispatch) when the input carries no `data.sessionId`
/// (fail-open — never drop a legitimate first reply on malformed/absent input)
/// or one that equals `session_id`; `false` (foreign → suppress) only when a
/// *differing* non-empty string `sessionId` is present.
fn classify_ownership(input: Option<&Value>, session_id: &str) -> bool {
    let owner = input
        .and_then(|i| i.get("data"))
        .and_then(|d| d.get("sessionId"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty());
    match owner {
        Some(owner) => owner == session_id,
        None => true, // fail-open: no owner recorded ⇒ treat as ours
    }
}

#[cfg(test)]
mod tests {
    use super::classify_ownership;
    use serde_json::json;

    const SID: &str = "11111111-1111-1111-1111-111111111111";

    #[test]
    fn owns_when_session_id_matches() {
        let input = json!({ "data": { "sessionId": SID, "userMessage": "hi" } });
        assert!(classify_ownership(Some(&input), SID));
    }

    #[test]
    fn foreign_when_session_id_differs() {
        let input = json!({ "data": { "sessionId": "22222222-different" } });
        assert!(!classify_ownership(Some(&input), SID));
    }

    #[test]
    fn fail_open_when_owner_absent_or_malformed() {
        // No sessionId key.
        assert!(classify_ownership(
            Some(&json!({ "data": { "userMessage": "hi" } })),
            SID
        ));
        // Empty-string sessionId is treated as absent (fail-open own).
        assert!(classify_ownership(
            Some(&json!({ "data": { "sessionId": "" } })),
            SID
        ));
        // sessionId present but not a string.
        assert!(classify_ownership(
            Some(&json!({ "data": { "sessionId": 42 } })),
            SID
        ));
        // No `data` envelope at all.
        assert!(classify_ownership(Some(&json!({ "sessionId": SID })), SID));
        // `data` is not an object.
        assert!(classify_ownership(Some(&json!({ "data": "nope" })), SID));
        // No input at all (pre-registration / unreadable).
        assert!(classify_ownership(None, SID));
    }
}
