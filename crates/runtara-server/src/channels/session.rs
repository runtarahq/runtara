use std::sync::Arc;

use crate::runtime_types::ListEventsOptions;
use axum::http::StatusCode;
use dashmap::{DashMap, mapref::entry::Entry};
use redis::aio::ConnectionManager;
use serde_json::{Value, json};
use sqlx::PgPool;
use tokio::sync::mpsc;
use tokio::time::{Duration, sleep};
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use crate::api::dto::triggers::TriggerType;
use crate::api::handlers::chat::{ChatEvent, chat_event_type, parse_debug_event};
use crate::api::repositories::triggers::TriggerRepository;
use crate::api::services::session_queue;
use crate::api::services::session_queue::managed::{self, InputTarget, QueueScope};
use crate::runtime_client::RuntimeClient;
use crate::workers::execution_engine::{ExecutionEngine, QueueRequest, TriggerSource};
use runtara_connections::ConnectionsFacade;

use super::channel::{Channel, TelegramChannel};
use super::collector;
use super::intake::{Accepted, IntakeStore, PinnedWorkflow, ReplyBinding};

mod inputs;
use inputs::{InputProgress, ManagedChannelInputs, ReplyTarget, UNDELIVERED_NOTICE};

/// How often pending channel messages are retried and old ones purged.
const INTAKE_SWEEP_INTERVAL: Duration = Duration::from_secs(30);

const AMBIGUOUS_NOTICE: &str =
    "Several inputs are waiting. Answer the intended one in the workflow view.";

/// A normalized inbound message from any channel. Serialized into the durable
/// intake row, so a message accepted before a crash can be dispatched again.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
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
    /// Provider activity/message id, the durable dedup identity of the
    /// delivery. `None` when unavailable (the payload hash is used instead).
    pub activity_id: Option<String>,
    /// The durable intake row this message was accepted as. Launches use it as
    /// their instance id; handling it marks the row processed.
    #[serde(skip)]
    pub intake_id: Option<Uuid>,
    /// The workflow version current when the message was accepted. Launches
    /// use it even if a newer version is published before they run.
    #[serde(skip)]
    pub workflow: Option<PinnedWorkflow>,
}

/// A normalized attachment from any channel.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
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
    /// Provider file identifier, when supplied (e.g. a Slack file ID).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
}

/// Session key: (connection_id, trigger_id, discriminator).
/// The discriminator varies based on session_mode:
/// - per_sender: sender identity (chat_id, user_id, email)
/// - per_trigger: "shared" (everyone shares one session)
/// - per_message: random UUID (no session continuity)
type SessionKey = (String, String, String);

/// The active Channel trigger and connection a message is routed through.
struct Route {
    tenant_id: String,
    trigger_id: String,
    workflow_id: String,
    workflow_version: i32,
    session_mode: String,
    integration_id: String,
    params: Value,
}

enum RouteError {
    /// Configuration cannot route this connection; retrying will not help.
    Unroutable(anyhow::Error),
    /// Storage was unavailable; the provider should retry.
    Unavailable(anyhow::Error),
}

impl std::fmt::Display for RouteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unroutable(e) | Self::Unavailable(e) => write!(f, "{e}"),
        }
    }
}

/// Routes incoming channel messages to the right session.
///
/// Every verified message is first stored in durable intake; only then is the
/// webhook acknowledged. Each active conversation gets its own session actor.
pub struct ChannelRouter {
    sessions: Arc<DashMap<SessionKey, mpsc::Sender<InboundMessage>>>,
    client: Arc<RuntimeClient>,
    pool: PgPool,
    connections: Arc<ConnectionsFacade>,
    engine: Arc<ExecutionEngine>,
    valkey: ConnectionManager,
    intake: IntakeStore,
    /// Rows accepted before this instant belong to an earlier process and are
    /// dispatched again by `recover_pending`.
    started_at: chrono::DateTime<chrono::Utc>,
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
            intake: IntakeStore::new(pool.clone()),
            started_at: chrono::Utc::now(),
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

    /// Look up the connection and its active Channel trigger.
    async fn resolve_route(&self, connection_id: &str) -> Result<Route, RouteError> {
        let conn = self
            .connections
            .get_channel_connection(connection_id)
            .await
            .map_err(|e| RouteError::Unavailable(anyhow::anyhow!("DB error: {}", e)))?
            .ok_or_else(|| {
                RouteError::Unroutable(anyhow::anyhow!("Connection not found: {}", connection_id))
            })?;

        let tenant_id = conn.tenant_id.ok_or_else(|| {
            RouteError::Unroutable(anyhow::anyhow!("Connection has no tenant_id"))
        })?;
        if tenant_id != crate::config::tenant_id() {
            return Err(RouteError::Unroutable(anyhow::anyhow!(
                "Connection tenant mismatch"
            )));
        }

        let triggers = TriggerRepository::new(self.pool.clone())
            .list(Some(&tenant_id))
            .await
            .map_err(|e| RouteError::Unavailable(anyhow::anyhow!("DB error: {}", e)))?;
        let trigger = triggers
            .into_iter()
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
                RouteError::Unroutable(anyhow::anyhow!(
                    "No active Channel trigger found for connection {}",
                    connection_id
                ))
            })?;

        let session_mode = trigger
            .configuration
            .as_ref()
            .and_then(|c| c.get("session_mode"))
            .and_then(|v| v.as_str())
            .unwrap_or("per_sender")
            .to_string();
        let params = conn.connection_parameters.ok_or_else(|| {
            RouteError::Unroutable(anyhow::anyhow!("Connection has no parameters"))
        })?;
        let workflow_version =
            crate::api::repositories::workflows::WorkflowRepository::new(self.pool.clone())
                .get_current_or_latest_version(&tenant_id, &trigger.workflow_id)
                .await
                .map_err(|e| RouteError::Unavailable(anyhow::anyhow!("DB error: {}", e)))?
                .filter(|version| *version > 0)
                .ok_or_else(|| {
                    RouteError::Unroutable(anyhow::anyhow!(
                        "Workflow {} has no versions",
                        trigger.workflow_id
                    ))
                })?;

        Ok(Route {
            tenant_id,
            trigger_id: trigger.id,
            workflow_version,
            workflow_id: trigger.workflow_id,
            session_mode,
            integration_id: conn.integration_id.unwrap_or_default(),
            params,
        })
    }

    /// Receive a verified inbound message: store it durably, then hand it to
    /// its session. The returned status is the webhook response. It is 2xx
    /// only once the message is stored (or already was, for a redelivery), so
    /// a provider retries anything this process could lose.
    ///
    /// `detach` hands off in a background task, for providers with a short
    /// acknowledgement deadline (Teams). Storage already happened, so a
    /// failure after the ack is retried by the intake sweep rather than lost.
    pub async fn receive(
        self: &Arc<Self>,
        connection_id: &str,
        mut msg: InboundMessage,
        detach: bool,
    ) -> StatusCode {
        let route = match self.resolve_route(connection_id).await {
            Ok(route) => route,
            Err(RouteError::Unroutable(e)) => {
                // Not retryable: the provider would redeliver into the same
                // configuration. Acknowledge and drop, as before.
                warn!(connection_id = %connection_id, error = %e, "Dropping unroutable channel message");
                return StatusCode::OK;
            }
            Err(RouteError::Unavailable(e)) => {
                warn!(connection_id = %connection_id, error = %e, "Channel routing unavailable");
                return StatusCode::SERVICE_UNAVAILABLE;
            }
        };

        let workflow = PinnedWorkflow {
            id: route.workflow_id.clone(),
            version: route.workflow_version,
        };
        match self
            .intake
            .accept(
                &route.tenant_id,
                connection_id,
                &route.trigger_id,
                &workflow,
                &msg,
            )
            .await
        {
            Ok(Accepted::New(intake_id)) => {
                msg.intake_id = Some(intake_id);
                msg.workflow = Some(workflow);
            }
            Ok(Accepted::Duplicate) => {
                debug!(connection_id = %connection_id, "Dropping duplicate channel delivery");
                return StatusCode::OK;
            }
            Err(e) => {
                warn!(connection_id = %connection_id, error = %e, "Unable to store channel message");
                return StatusCode::SERVICE_UNAVAILABLE;
            }
        }

        if detach {
            let router = self.clone();
            let connection_id = connection_id.to_string();
            tokio::spawn(async move {
                router.dispatch_logged(&connection_id, route, msg).await;
            });
        } else {
            self.dispatch_logged(connection_id, route, msg).await;
        }
        StatusCode::OK
    }

    /// Background intake worker: dispatch the previous process's pending
    /// messages at once, then keep retrying due pending messages and deleting
    /// handled ones past retention. Runs for the life of the process.
    pub async fn run_intake_worker(self: Arc<Self>) {
        self.recover_pending().await;
        let mut tick = tokio::time::interval(INTAKE_SWEEP_INTERVAL);
        tick.tick().await;
        loop {
            tick.tick().await;
            self.sweep_intake().await;
            match self
                .intake
                .purge(crate::config::tenant_id(), super::intake::RETENTION, 1000)
                .await
            {
                Ok(0) => {}
                Ok(deleted) => debug!(deleted, "Purged handled channel intake"),
                Err(e) => warn!(error = %e, "Unable to purge channel intake"),
            }
        }
    }

    /// Make messages a previous process accepted but did not finish due now,
    /// and dispatch them.
    pub async fn recover_pending(self: &Arc<Self>) {
        match self
            .intake
            .release_orphans(crate::config::tenant_id(), self.started_at)
            .await
        {
            Ok(0) => {}
            Ok(count) => info!(count, "Recovering pending channel messages"),
            Err(e) => warn!(error = %e, "Unable to release pending channel intake"),
        }
        self.sweep_intake().await;
    }

    /// Dispatch every pending message whose next attempt is due. A message
    /// whose launch failed transiently is retried here, with backoff.
    pub async fn sweep_intake(self: &Arc<Self>) {
        const BATCH: i64 = 100;
        loop {
            let claimed = match self
                .intake
                .claim_due(crate::config::tenant_id(), BATCH)
                .await
            {
                Ok(claimed) => claimed,
                Err(e) => {
                    warn!(error = %e, "Unable to claim pending channel intake");
                    return;
                }
            };
            let full = claimed.len() as i64 == BATCH;
            for row in claimed {
                if let Some(binding) = &row.reply {
                    self.recover_reply(row.intake_id, binding).await;
                    continue;
                }
                match self.resolve_route(&row.connection_id).await {
                    Ok(route) => {
                        self.dispatch_logged(&row.connection_id, route, row.message)
                            .await
                    }
                    Err(RouteError::Unroutable(e)) => {
                        if let Err(e) = self.intake.mark_failed(row.intake_id, &e.to_string()).await
                        {
                            warn!(intake_id = %row.intake_id, error = %e, "Unable to fail unroutable channel intake");
                        }
                    }
                    // Left pending; claimed again after its backoff.
                    Err(RouteError::Unavailable(e)) => {
                        warn!(intake_id = %row.intake_id, error = %e, "Channel intake left pending");
                    }
                }
            }
            if !full {
                return;
            }
        }
    }

    /// Finish a reply whose session ended before it reached the managed queue.
    /// It goes to the request it was bound to, exactly as the session would
    /// have handed it off, and never starts a run. Errors leave it pending.
    async fn recover_reply(&self, intake_id: Uuid, binding: &ReplyBinding) {
        let outcome = async {
            let tenant = crate::config::tenant_id();
            let scope = QueueScope::new(tenant, &binding.session_id)?;
            let message_id = intake_id.to_string();
            let mut valkey = self.valkey.clone();
            // Already handed off before the session ended.
            match managed::get(&mut valkey, &scope, &message_id).await {
                Ok(_) => return anyhow::Ok("reply"),
                Err(managed::QueueError::NotFound) => {}
                Err(e) => return Err(e.into()),
            }
            let page = self
                .client
                .list_input_requests(
                    tenant,
                    std::slice::from_ref(&binding.instance_id),
                    0,
                    u32::MAX,
                )
                .await?;
            let Some(request) = page
                .requests
                .iter()
                .find(|request| request.request_id == binding.request_id)
            else {
                // Its request closed; never re-aim a reply at another one.
                return Ok("undelivered");
            };
            if inputs::is_structured(request) {
                // Field collection lived in the ended session; the reply alone
                // is not a response to the structured request.
                return Ok("interrupted");
            }
            managed::enqueue_targeted(
                &mut valkey,
                &scope,
                &message_id,
                &message_id,
                &binding.payload,
                &InputTarget {
                    instance_id: binding.instance_id.clone(),
                    request_id: binding.request_id.clone(),
                },
            )
            .await?;
            Ok("reply")
        }
        .await;
        match outcome {
            Ok(outcome) => {
                self.intake
                    .settle(Some(intake_id), outcome, Some(&binding.instance_id))
                    .await
            }
            Err(e) => warn!(%intake_id, error = %e, "Channel reply recovery left pending"),
        }
    }

    async fn dispatch_logged(
        self: &Arc<Self>,
        connection_id: &str,
        route: Route,
        msg: InboundMessage,
    ) {
        let intake_id = msg.intake_id;
        if let Err(e) = self.dispatch(connection_id, route, msg).await {
            warn!(connection_id = %connection_id, error = %e, "Failed to handle channel message");
            if let Some(intake_id) = intake_id
                && let Err(e) = self.intake.mark_failed(intake_id, &e.to_string()).await
            {
                warn!(%intake_id, error = %e, "Unable to fail channel intake");
            }
        }
    }

    /// Route a stored message to its conversation's session, starting one if
    /// needed. Errors are permanent for this message (bad connection data).
    async fn dispatch(
        self: &Arc<Self>,
        connection_id: &str,
        route: Route,
        mut msg: InboundMessage,
    ) -> anyhow::Result<()> {
        // Teams replies need the serviceUrl of the conversation. It is part of
        // the verified activity, so a recovered message restores it too.
        if route.integration_id == "teams_bot"
            && let Some(url) = msg
                .original_message
                .get("serviceUrl")
                .and_then(Value::as_str)
        {
            self.set_teams_service_url(connection_id, &msg.conv_id, url);
        }

        let discriminator = match route.session_mode.as_str() {
            "per_trigger" => "shared".to_string(),
            "per_message" => Uuid::new_v4().to_string(),
            _ => msg.sender_id.clone(), // per_sender (default)
        };
        let key = (
            connection_id.to_string(),
            route.trigger_id.clone(),
            discriminator,
        );

        // Built up front: a session must never be registered and then abandoned.
        let channel = self.channel_adapter(connection_id, &route)?;

        let (tx, rx) = loop {
            let existing = self.sessions.get(&key).map(|tx| tx.clone());
            if let Some(tx) = existing {
                match tx.send(msg).await {
                    Ok(()) => return Ok(()),
                    Err(mpsc::error::SendError(returned)) => {
                        // The actor ended. Replace it, unless another message
                        // already did.
                        msg = returned;
                        self.sessions.remove_if(&key, |_, v| v.same_channel(&tx));
                        continue;
                    }
                }
            }
            // Atomic check-and-insert: two concurrent first messages from one
            // sender must share a session, not start two.
            let (tx, rx) = mpsc::channel::<InboundMessage>(32);
            match self.sessions.entry(key.clone()) {
                Entry::Occupied(_) => continue,
                Entry::Vacant(slot) => {
                    slot.insert(tx.clone());
                    break (tx, rx);
                }
            }
        };

        // The first message goes into the execution inputs, not the mpsc
        // channel, which carries only subsequent messages in the session.
        let router = self.clone();
        let conv_id = msg.conv_id.clone();
        tokio::spawn(async move {
            let mut rx = rx;
            info!(
                conv_id = %conv_id,
                workflow_id = %route.workflow_id,
                session_mode = %route.session_mode,
                "Channel session starting"
            );
            if let Err(e) = session_loop(
                channel,
                &conv_id,
                msg,
                &mut rx,
                router.client.clone(),
                router.engine.clone(),
                router.valkey.clone(),
                router.intake.clone(),
                &route.tenant_id,
                &route.workflow_id,
                &route.session_mode,
                &key.0,
            )
            .await
            {
                warn!(conv_id = %conv_id, error = %e, "Channel session ended with error");
            } else {
                info!(conv_id = %conv_id, "Channel session ended normally");
            }
            router.sessions.remove_if(&key, |_, v| v.same_channel(&tx));
            drop(tx);
            // Messages routed here after the loop stopped reading were never
            // handled. Route them again rather than dropping them.
            rx.close();
            while let Ok(pending) = rx.try_recv() {
                router.redispatch(key.0.clone(), pending);
            }
        });

        Ok(())
    }

    fn redispatch(self: &Arc<Self>, connection_id: String, msg: InboundMessage) {
        let router = self.clone();
        tokio::spawn(async move {
            match router.resolve_route(&connection_id).await {
                Ok(route) => {
                    let dispatch: std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> =
                        Box::pin(router.dispatch_logged(&connection_id, route, msg));
                    dispatch.await
                }
                // Left pending: the next startup dispatches it again.
                Err(e) => {
                    warn!(connection_id = %connection_id, error = %e, "Unable to reroute channel message")
                }
            }
        });
    }

    /// Build the reply adapter from connection credentials.
    fn channel_adapter(
        &self,
        connection_id: &str,
        route: &Route,
    ) -> anyhow::Result<Arc<dyn Channel>> {
        let params = &route.params;
        Ok(match route.integration_id.as_str() {
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
                    route.tenant_id.clone(),
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
        })
    }
}

// ===========================================================================
// Session loop: diagnostic history and authoritative input state are separate
// ===========================================================================

#[allow(clippy::too_many_arguments)]
async fn session_loop(
    channel: Arc<dyn Channel>,
    conv_id: &str,
    initial_message: InboundMessage,
    user_rx: &mut mpsc::Receiver<InboundMessage>,
    client: Arc<RuntimeClient>,
    engine: Arc<ExecutionEngine>,
    mut valkey: ConnectionManager,
    intake: IntakeStore,
    org_id: &str,
    workflow_id: &str,
    session_mode: &str,
    source_connection_id: &str,
) -> anyhow::Result<()> {
    // conv_id tracks the current conversation target (channel/thread).
    // Updated when subsequent messages arrive from a different channel,
    // so responses always go where the sender is currently messaging.
    let mut conv_id = conv_id.to_string();

    let session_id = Uuid::new_v4().to_string();

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
        "sourceConnectionId": source_connection_id,
        "originalMessage": &initial_message.original_message,
    });
    if let Some(target) = &initial_message.target {
        data["target"] = target.clone();
    }
    let inputs = json!({ "data": data, "variables": {} });

    // The intake id is the launch identity: dispatching the same stored
    // message again (after a crash) is deduplicated by the execution engine.
    // The workflow version was fixed when the message was accepted.
    let (launch_workflow, launch_version) = match &initial_message.workflow {
        Some(pinned) => (pinned.id.as_str(), Some(pinned.version)),
        None => (workflow_id, None),
    };
    let result = match engine
        .queue(QueueRequest {
            run_label: None,
            tenant_id: org_id,
            workflow_id: launch_workflow,
            version: launch_version,
            inputs,
            debug: false,
            correlation_id: None,
            idempotency_key: None,
            trigger_source: TriggerSource::Webhook,
            instance_id: initial_message.intake_id,
        })
        .await
    {
        Ok(result) => result,
        Err(e) => {
            fail_launch(&intake, initial_message.intake_id, &e).await;
            anyhow::bail!("Failed to queue execution: {:?}", e);
        }
    };

    let mut instance_id = result.instance_id.to_string();
    intake
        .settle(initial_message.intake_id, "launched", Some(&instance_id))
        .await;

    let _ = session_queue::set_session_meta(
        &mut valkey,
        org_id,
        &session_id,
        &instance_id,
        launch_workflow,
    )
    .await;

    let mut managed_inputs = ManagedChannelInputs {
        client: client.clone(),
        conn: valkey.clone(),
        scope: QueueScope::new(org_id, &session_id)?,
        prompted: Default::default(),
        intake: Some(intake.clone()),
    };

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
    while !session_ended && start_time.elapsed() < max_duration {
        // === INSTANCE LOOP ===
        let mut event_offset: u32 = 0;
        let mut instance_done = false;
        let mut last_input_notice: Option<&str> = None;
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
                            // Replies received during this execution remain responses.
                            // They must not fall through to idle startup handling.
                            if !foreign {
                                match managed_inputs.finish_instance(&instance_id, &channel, &conv_id).await {
                                    Ok(true) => {},
                                    Ok(false) => continue,
                                    Err(error) => {
                                        warn!(error = %error, "Unable to retain terminal-session replies");
                                        continue;
                                    }
                                }
                            }
                            // A foreign-owned instance was already flushed by its
                            // owning session; re-flushing from offset 0 would
                            // re-send the entire reply transcript. Suppress both
                            // the flush and the Failed notice, and drop to the
                            // idle phase so per_sender/per_trigger sessions stay
                            // alive for genuinely new turns.
                            if !foreign {
                                flush_events(
                                    &client, &channel, &conv_id, &instance_id,
                                    &mut event_offset,
                                ).await;

                                if let crate::runtime_types::InstanceStatus::Failed = info.status {
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
                        sort_order: Some(crate::runtime_types::EventSortOrder::Asc),
                        limit: Some(100),
                        offset: Some(event_offset),
                        ..Default::default()
                    };

                    if let Ok(result) = client.list_events(&instance_id, Some(options)).await {
                        for event in result.events {
                            if let Some(payload) = &event.payload {
                                dispatch_event(event.subtype.as_deref(), payload, &channel, &conv_id).await;
                            }
                            event_offset += 1;
                        }
                    }
                    // Input discovery is mandatory even with no debug events or
                    // when event history retrieval fails.
                    let notice = match managed_inputs.poll(&instance_id, &channel, &conv_id, user_rx).await {
                        Ok(InputProgress::Ambiguous) => Some(AMBIGUOUS_NOTICE),
                        // Each dropped reply is reported, even if it repeats.
                        Ok(InputProgress::Undelivered) => {
                            let _ = channel.send_text(&conv_id, UNDELIVERED_NOTICE).await;
                            None
                        }
                        Ok(_) => None,
                        Err(error) => {
                            warn!(error = %error, "Channel input processing unavailable or collection ended");
                            Some("Unable to confirm input delivery. Check the workflow before sending another reply.")
                        }
                    };
                    if notice != last_input_notice {
                        if let Some(message) = notice { let _ = channel.send_text(&conv_id, message).await; }
                        last_input_notice = notice;
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
                        "sourceConnectionId": source_connection_id,
                        "originalMessage": inbound.original_message,
                    });
                    if let Some(target) = &inbound.target {
                        event["target"] = target.clone();
                    }
                    // Bind the reply now, to the request its sender was shown.
                    // Anything else is reported undelivered, never retained for
                    // whichever request happens to open next.
                    let refusal = match managed_inputs.reply_target(&instance_id).await {
                        Ok(ReplyTarget::Request(request)) => {
                            // Record the binding first: after a crash, recovery
                            // delivers this reply to the same request instead of
                            // launching a run from it. The row stays pending
                            // until the reply reaches the managed queue.
                            let bound = match inbound.intake_id {
                                Some(intake_id) => intake.bind_reply(intake_id, &ReplyBinding {
                                    session_id: session_id.clone(),
                                    instance_id: instance_id.clone(),
                                    request_id: request.clone(),
                                    payload: event.clone(),
                                }).await.map_err(anyhow::Error::from),
                                None => Ok(()),
                            };
                            let buffered = match bound {
                                Ok(()) => session_queue::push_event(&mut valkey, org_id, &session_id, Some(&instance_id), Some(&request), inbound.intake_id, &event).await.map_err(anyhow::Error::from),
                                Err(e) => Err(e),
                            };
                            match buffered {
                                Ok(()) => None,
                                Err(e) => {
                                    warn!(error = %e, "Failed to buffer channel reply");
                                    Some("Your reply could not be saved. Please send it again.")
                                }
                            }
                        }
                        Ok(ReplyTarget::NotWaiting) => Some("Nothing is waiting for a reply right now, so your message was not delivered."),
                        Ok(ReplyTarget::Ambiguous) => Some("Several inputs are waiting. Answer in the workflow view; your message was not delivered."),
                        Err(error) => {
                            warn!(error = %error, "Channel input discovery unavailable");
                            Some("Unable to confirm which input your reply answers, so it was not delivered. Please try again.")
                        }
                    };
                    if let Some(message) = refusal {
                        let _ = channel.send_text(&conv_id, message).await;
                        // The sender was told; the message is handled.
                        intake.settle(inbound.intake_id, "refused", Some(&instance_id)).await;
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
                        match managed::has_unresolved(&mut valkey, &managed_inputs.scope).await {
                            Ok(false) => {},
                            Ok(true) => {
                                // Deliver or explicitly fail earlier replies before
                                // a new run can start; never leave them to block.
                                match managed_inputs.settle_queue(&channel, &conv_id).await {
                                    Ok(true) if last_input_notice != Some("pending_response") => {
                                        let _ = channel.send_text(&conv_id, "A previous reply is still being delivered. Please wait a moment.").await;
                                        last_input_notice = Some("pending_response");
                                    }
                                    Ok(_) => {}
                                    Err(error) => warn!(error = %error, "Unable to settle channel replies"),
                                }
                                continue;
                            }
                            Err(error) => {
                                warn!(error = %error, "Unable to verify outstanding channel replies");
                                continue;
                            }
                        }
                        if let Ok(true) = session_queue::has_events(&mut valkey, org_id, &session_id).await {
                            // Only a fresh idle message can start a run. A reply
                            // from an earlier execution cannot be consumed here.
                            let queued_msg = match session_queue::take_startup_event(&mut valkey, org_id, &session_id).await {
                                Ok(Some(message)) => Some(message),
                                Ok(None) => continue,
                                Err(error) => {
                                    warn!(error = %error, "Channel startup buffer unavailable");
                                    continue;
                                }
                            };
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
                            let queued_intake = queued_msg.as_ref()
                                .and_then(|m| m.get("intakeId"))
                                .and_then(Value::as_str)
                                .and_then(|id| Uuid::parse_str(id).ok());
                            let queued_workflow = queued_msg.as_ref()
                                .and_then(|m| Some(PinnedWorkflow {
                                    id: m.get("workflowId")?.as_str()?.to_string(),
                                    version: i32::try_from(m.get("workflowVersion")?.as_i64()?).ok()?,
                                }));
                            let (launch_workflow, launch_version) = match &queued_workflow {
                                Some(pinned) => (pinned.id.as_str(), Some(pinned.version)),
                                None => (workflow_id, None),
                            };
                            let mut requeue_data = json!({
                                "sessionId": &session_id,
                                "channel": &initial_message.channel,
                                "userMessage": user_message,
                                "attachments": queued_attachments,
                                "sourceConnectionId": source_connection_id,
                                "originalMessage": queued_original,
                            });
                            if let Some(target) = queued_target {
                                requeue_data["target"] = target;
                            }
                            let inputs = json!({ "data": requeue_data, "variables": {} });
                            match engine.queue(QueueRequest {
                                run_label: None,
                                tenant_id: org_id,
                                workflow_id: launch_workflow,
                                version: launch_version,
                                inputs,
                                debug: false,
                                correlation_id: None,
                                idempotency_key: None,
                                trigger_source: TriggerSource::Webhook,
                                instance_id: queued_intake,
                            }).await {
                                Ok(result) => {
                                    instance_id = result.instance_id.to_string();
                                    intake.settle(queued_intake, "launched", Some(&instance_id)).await;
                                    let _ = session_queue::set_session_meta(
                                        &mut valkey, org_id, &session_id, &instance_id, launch_workflow,
                                    ).await;
                                    info!(instance_id = %instance_id, "New instance for channel session");
                                    sleep(Duration::from_millis(500)).await;
                                    break;
                                }
                                Err(e) => {
                                    error!(error = ?e, "Failed to start new instance");
                                    fail_launch(&intake, queued_intake, &e).await;
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
                            "sourceConnectionId": source_connection_id,
                            "originalMessage": inbound.original_message,
                        });
                        if let Some(target) = &inbound.target {
                            event["target"] = target.clone();
                        }
                        // The row stays pending until the run it starts is
                        // queued; a crash before then dispatches it again.
                        if let Some(intake_id) = inbound.intake_id {
                            event["intakeId"] = json!(intake_id);
                        }
                        if let Some(pinned) = &inbound.workflow {
                            event["workflowId"] = json!(pinned.id);
                            event["workflowVersion"] = json!(pinned.version);
                        }
                        if let Err(error) = session_queue::push_event(&mut valkey, org_id, &session_id, None, None, None, &event).await {
                            warn!(error = %error, "Unable to buffer channel startup message");
                        }
                    }
                }
            }
        }
    }

    if start_time.elapsed() >= max_duration {
        debug!(conv_id = %conv_id, "Channel session timed out");
    }

    // Replies buffered for the execution this actor is leaving are settled
    // now: bound ones are handed to the managed queue, the rest are reported.
    // Nothing is left for a later actor to bind to a different request.
    for _ in 0..100 {
        match managed_inputs
            .finish_instance(&instance_id, &channel, &conv_id)
            .await
        {
            Ok(false) => continue,
            Ok(true) => break,
            Err(error) => {
                warn!(error = %error, "Unable to settle buffered channel replies on exit");
                break;
            }
        }
    }

    Ok(())
}

/// A launch that can never succeed (invalid input, missing workflow) must not
/// be dispatched again at every startup. Anything else stays pending.
async fn fail_launch(
    intake: &IntakeStore,
    intake_id: Option<Uuid>,
    error: &crate::workers::execution_engine::ExecutionError,
) {
    use crate::workers::execution_engine::ExecutionError as E;
    let permanent = matches!(
        error,
        E::ValidationError(_)
            | E::WorkflowValidationError { .. }
            | E::NotFound(_)
            | E::WorkflowNotFound(_)
            | E::WorkflowNotRunnable { .. }
    );
    if let (true, Some(intake_id)) = (permanent, intake_id)
        && let Err(e) = intake.mark_failed(intake_id, &format!("{error:?}")).await
    {
        warn!(%intake_id, error = %e, "Unable to fail channel intake");
    }
}

async fn flush_events(
    client: &Arc<RuntimeClient>,
    channel: &Arc<dyn Channel>,
    conv_id: &str,
    instance_id: &str,
    event_offset: &mut u32,
) {
    let options = ListEventsOptions {
        event_type: Some("custom".to_string()),
        sort_order: Some(crate::runtime_types::EventSortOrder::Asc),
        limit: Some(100),
        offset: Some(*event_offset),
        ..Default::default()
    };

    if let Ok(result) = client.list_events(instance_id, Some(options)).await {
        for event in result.events {
            if let Some(payload) = &event.payload {
                dispatch_event(event.subtype.as_deref(), payload, channel, conv_id).await;
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
) {
    let chat_events = parse_debug_event(subtype, payload);

    for chat_event in chat_events {
        match &chat_event {
            ChatEvent::Message { content, .. } if !content.is_empty() => {
                if let Err(e) = channel.send_text(conv_id, content).await {
                    warn!(conv_id = %conv_id, error = %e, "Failed to send channel reply");
                }
            }

            // Historical input events describe the transcript, never an action.
            ChatEvent::WaitingForInput { .. } => {}

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

#[cfg(all(test, feature = "valkey-integration-tests"))]
mod input_tests;
