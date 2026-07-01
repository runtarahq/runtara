use std::time::Duration;

use chrono::{DateTime, Utc};
use redis::aio::ConnectionManager;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::mpsc;
use tokio::sync::mpsc::error::TrySendError;
use uuid::Uuid;

use crate::auth::{AuthContext, AuthMethod};
use crate::shutdown::ShutdownSignal;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ActorType {
    User,
    ApiKey,
    System,
    Trigger,
}

impl ActorType {
    /// Wire string stored in the `actor_type` TEXT column.
    pub fn as_str(self) -> &'static str {
        match self {
            ActorType::User => "user",
            ActorType::ApiKey => "api_key",
            ActorType::System => "system",
            ActorType::Trigger => "trigger",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EventSource {
    Api,
    Ui,
    Mcp,
    Worker,
}

impl EventSource {
    /// Wire string stored in the `source` TEXT column.
    pub fn as_str(self) -> &'static str {
        match self {
            EventSource::Api => "api",
            EventSource::Ui => "ui",
            EventSource::Mcp => "mcp",
            EventSource::Worker => "worker",
        }
    }
}

/// Product-analytics event types. Each variant maps to a stable dotted string stored in
/// the `event_type` TEXT column via [`EventType::as_str`].
///
/// Deliberately conservative: only events with a clear, existing emission point in the
/// codebase are listed. Add a variant (and its `as_str` arm) when you wire its call-site —
/// the exhaustive match makes a missing string a compile error, not a runtime surprise.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EventType {
    // Account & access
    ApiKeyCreated,
    ApiKeyRevoked,
    // Workflow authoring
    WorkflowCreated,
    WorkflowUpdated,
    WorkflowDeleted,
    WorkflowCompiled,
    WorkflowVersionRegistered,
    // Execution
    ExecutionStarted,
    ExecutionCompleted,
    ExecutionFailed,
    ExecutionCancelled,
    // Triggers
    TriggerCreated,
    TriggerFired,
    // Agents & capabilities
    AgentCapabilityUsed,
    AgentCapabilityTested,
    // Connections (the integration funnel)
    ConnectionCreated,
    ConnectionDeleted,
    ConnectionOauthStarted,
    ConnectionOauthCompleted,
    ConnectionOauthFailed,
    ConnectionTokenRefreshed,
    // Billing-relevant (future-proofing)
    QuotaExceeded,
}

impl EventType {
    /// The stable wire string written to the `event_type` column. Exhaustive on purpose:
    /// adding a variant without a string fails to compile.
    pub fn as_str(self) -> &'static str {
        match self {
            EventType::ApiKeyCreated => "api_key.created",
            EventType::ApiKeyRevoked => "api_key.revoked",
            EventType::WorkflowCreated => "workflow.created",
            EventType::WorkflowUpdated => "workflow.updated",
            EventType::WorkflowDeleted => "workflow.deleted",
            EventType::WorkflowCompiled => "workflow.compiled",
            EventType::WorkflowVersionRegistered => "workflow.version_registered",
            EventType::ExecutionStarted => "execution.started",
            EventType::ExecutionCompleted => "execution.completed",
            EventType::ExecutionFailed => "execution.failed",
            EventType::ExecutionCancelled => "execution.cancelled",
            EventType::TriggerCreated => "trigger.created",
            EventType::TriggerFired => "trigger.fired",
            EventType::AgentCapabilityUsed => "agent.capability_used",
            EventType::AgentCapabilityTested => "agent.capability_tested",
            EventType::ConnectionCreated => "connection.created",
            EventType::ConnectionDeleted => "connection.deleted",
            EventType::ConnectionOauthStarted => "connection.oauth_started",
            EventType::ConnectionOauthCompleted => "connection.oauth_completed",
            EventType::ConnectionOauthFailed => "connection.oauth_failed",
            EventType::ConnectionTokenRefreshed => "connection.token_refreshed",
            EventType::QuotaExceeded => "quota.exceeded",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProductEvent {
    /// Generated here (not left to a DB default) because the event now crosses a process
    /// boundary via an at-least-once Valkey stream before it's inserted: smo-management's
    /// consumer dedupes retried deliveries with `ON CONFLICT (event_id) DO NOTHING`, which only
    /// works if the id is stable across redeliveries.
    pub event_id: Uuid,
    pub occurred_at: DateTime<Utc>, // stamped in ::new(), builder-overridable
    pub event_type: EventType,      // -> "workflow.created" etc. via as_str() at INSERT
    pub event_version: i16,         // default 1
    pub tenant_id: String,
    pub user_id: Option<String>,  //always shows user sub
    pub actor_id: Option<String>, //user sub for user, jti for API Key
    pub actor_type: Option<ActorType>,
    pub resource_id: Option<String>,
    pub resource_type: Option<String>,
    pub properties: serde_json::Value, // default {}
    pub session_id: Option<String>,
    pub request_id: Option<String>,
    pub source: Option<EventSource>,
}

impl ProductEvent {
    pub fn new(event_type: EventType) -> Self {
        Self {
            event_id: Uuid::new_v4(),
            occurred_at: Utc::now(),
            event_type,
            event_version: 1,
            tenant_id: crate::config::tenant_id().to_string(),
            user_id: None,
            actor_id: None,
            actor_type: None,
            resource_id: None,
            resource_type: None,
            properties: serde_json::json!({}),
            session_id: None,
            request_id: None,
            source: None,
        }
    }

    /// Build an event for an authenticated request, deriving tenant + actor from the
    /// caller's `AuthContext`. Tenant is `org_id`; `user_id` is always the human (for an
    /// API key that is the key's `issuing_user_id`, already resolved at auth time). When a
    /// key was used, `actor_type = ApiKey` and `actor_id` is its `jti`; otherwise the actor
    /// is the user themselves. Use this in HTTP handlers; use `new` + `no_user_actor` for
    /// worker/trigger events that have no caller.
    pub fn from_auth(event_type: EventType, ctx: &AuthContext) -> Self {
        let mut event = Self::new(event_type);
        event.user_id = Some(ctx.user_id.clone());
        match ctx.auth_method {
            AuthMethod::ApiKey => {
                event.actor_type = Some(ActorType::ApiKey);
                // The key's token id identifies the credential. Fall back to the user sub
                // in the (unexpected) case a key request carries no jti.
                event.actor_id = ctx.jti.clone().or_else(|| Some(ctx.user_id.clone()));
            }
            AuthMethod::Jwt | AuthMethod::Unauthenticated => {
                event.actor_type = Some(ActorType::User);
                event.actor_id = Some(ctx.user_id.clone());
            }
        }
        event
    }

    pub fn user_actor(
        mut self,
        user_id: impl Into<String>,
        actor_id: impl Into<String>,
        actor_type: ActorType,
    ) -> Self {
        self.user_id = Some(user_id.into());
        self.actor_id = Some(actor_id.into());
        self.actor_type = Some(actor_type);
        self
    }

    pub fn no_user_actor(mut self, actor_id: impl Into<String>, actor_type: ActorType) -> Self {
        self.actor_id = Some(actor_id.into());
        self.actor_type = Some(actor_type);
        self
    }

    pub fn resource(
        mut self,
        resource_id: impl Into<String>,
        resource_type: impl Into<String>,
    ) -> Self {
        self.resource_id = Some(resource_id.into());
        self.resource_type = Some(resource_type.into());
        self
    }

    pub fn properties(mut self, properties: Value) -> Self {
        self.properties = properties;
        self
    }

    pub fn trace(mut self, session_id: impl Into<String>, request_id: impl Into<String>) -> Self {
        self.session_id = Some(session_id.into());
        self.request_id = Some(request_id.into());
        self
    }

    pub fn source(mut self, source: EventSource) -> Self {
        self.source = Some(source);
        self
    }

    /// Wire JSON for the Valkey stream payload. Deliberately NOT `derive(Serialize)`: that
    /// derive serializes the enum fields by variant name (e.g. `"WorkflowCreated"`) for the
    /// unrelated internal round-trip through the compilation queue (see
    /// `valkey::compilation_queue::CompilationRequest::product_event`), whereas smo-management's
    /// consumer inserts straight into `TEXT` columns and expects the dotted wire strings
    /// (`"workflow.created"`) already used by `EventType::as_str()` elsewhere. This method is
    /// the one place that builds that external, dotted-string wire format.
    fn to_wire_json(&self) -> Value {
        serde_json::json!({
            "event_id": self.event_id,
            "occurred_at": self.occurred_at,
            "event_type": self.event_type.as_str(),
            "event_version": self.event_version,
            "tenant_id": self.tenant_id,
            "user_id": self.user_id,
            "actor_id": self.actor_id,
            "actor_type": self.actor_type.map(ActorType::as_str),
            "resource_id": self.resource_id,
            "resource_type": self.resource_type,
            "properties": self.properties,
            "session_id": self.session_id,
            "request_id": self.request_id,
            "source": self.source.map(EventSource::as_str),
        })
    }
}

/// Default in-memory channel capacity — events buffered before `emit` drops on backpressure.
const DEFAULT_CHANNEL_CAPACITY: usize = 10_000;
/// Default number of buffered rows that force a flush.
const DEFAULT_FLUSH_ROWS: usize = 200;
/// Default max time a partial batch waits before flushing.
const DEFAULT_FLUSH_INTERVAL_MS: u64 = 2_000;
/// Default approximate cap on the Valkey stream length (`XADD ... MAXLEN ~`). Bounds memory if
/// smo-management's consumer falls behind or a tenant's Valkey is unreachable — oldest entries
/// are trimmed first, so sustained backpressure silently drops old events rather than growing
/// the stream without bound, consistent with this pipeline's existing drop-on-backpressure stance.
const DEFAULT_STREAM_MAXLEN: usize = 200_000;

/// Valkey stream key product events are `XADD`ed to. Fixed, not tenant-prefixed: this runs
/// against runtara's local, single-tenant Valkey instance (like the compilation queue), and the
/// Valkey instance itself is the tenant boundary — see smo-management's `valkey::ValkeyWriter`
/// doc comment. smo-management's per-tenant consumer reads this same key from each tenant's
/// Valkey.
const STREAM_KEY: &str = "runtara:product_events:stream";

/// Tunables for the product-events pipeline. Every field is an independent, optional env
/// var; unset (or unparseable) means "use the default". Most tenants run entirely on
/// defaults — the `Option` fields exist so an operator can override one knob without
/// touching the others.
#[derive(Debug, Clone, Default)]
pub struct ProductEventConfig {
    /// `RUNTARA_PRODUCT_EVENTS_CHANNEL_CAPACITY` — bounded channel size.
    pub channel_capacity: Option<usize>,
    /// `RUNTARA_PRODUCT_EVENTS_FLUSH_ROWS` — flush once this many rows are buffered.
    pub flush_rows: Option<usize>,
    /// `RUNTARA_PRODUCT_EVENTS_FLUSH_INTERVAL_MS` — flush a partial batch after this long.
    pub flush_interval_ms: Option<u64>,
    /// `RUNTARA_PRODUCT_EVENTS_STREAM_MAXLEN` — approximate cap on the Valkey stream length.
    pub stream_maxlen: Option<usize>,
}

impl ProductEventConfig {
    /// Read each tunable from its own env var. Unset/unparseable → `None` (default applies).
    pub fn from_env() -> Self {
        Self {
            channel_capacity: parse_env("RUNTARA_PRODUCT_EVENTS_CHANNEL_CAPACITY"),
            flush_rows: parse_env("RUNTARA_PRODUCT_EVENTS_FLUSH_ROWS"),
            flush_interval_ms: parse_env("RUNTARA_PRODUCT_EVENTS_FLUSH_INTERVAL_MS"),
            stream_maxlen: parse_env("RUNTARA_PRODUCT_EVENTS_STREAM_MAXLEN"),
        }
    }

    /// Effective channel capacity.
    pub fn channel_capacity(&self) -> usize {
        self.channel_capacity.unwrap_or(DEFAULT_CHANNEL_CAPACITY)
    }

    /// Effective flush-row threshold.
    pub fn flush_rows(&self) -> usize {
        self.flush_rows.unwrap_or(DEFAULT_FLUSH_ROWS)
    }

    /// Effective flush interval.
    pub fn flush_interval(&self) -> Duration {
        Duration::from_millis(self.flush_interval_ms.unwrap_or(DEFAULT_FLUSH_INTERVAL_MS))
    }

    /// Effective approximate stream length cap.
    pub fn stream_maxlen(&self) -> usize {
        self.stream_maxlen.unwrap_or(DEFAULT_STREAM_MAXLEN)
    }
}

/// Read and parse a single env var, returning `None` if unset or unparseable.
fn parse_env<T: std::str::FromStr>(key: &str) -> Option<T> {
    std::env::var(key).ok().and_then(|v| v.parse().ok())
}

/// Single background consumer of the product-event channel. Owns the `Receiver`, accumulates
/// events into a batch, and ships each batch to Valkey as a pipelined `XADD` per event onto
/// [`STREAM_KEY`]. smo-management runs a per-tenant consumer group against that stream and is
/// the one that actually persists events — this drain no longer touches Postgres at all.
pub struct ProductEventDrain {
    /// Shared, non-blocking connection manager. `XADD` is non-blocking, so (unlike the
    /// consumer side) this can safely reuse the process-wide multiplexed manager.
    manager: ConnectionManager,
    rx: mpsc::Receiver<ProductEvent>,
    config: ProductEventConfig,
    shutdown: ShutdownSignal,
}

impl ProductEventDrain {
    pub fn new(
        manager: ConnectionManager,
        rx: mpsc::Receiver<ProductEvent>,
        config: ProductEventConfig,
        shutdown: ShutdownSignal,
    ) -> Self {
        Self {
            manager,
            rx,
            config,
            shutdown,
        }
    }

    /// Accumulate events and flush on size, time, shutdown, or channel-close. Consumes
    /// `self`; spawn with `tokio::spawn(drain.run())`.
    pub async fn run(mut self) {
        let flush_rows = self.config.flush_rows();
        let mut batch: Vec<ProductEvent> = Vec::with_capacity(flush_rows);
        let mut ticker = tokio::time::interval(self.config.flush_interval());

        loop {
            tokio::select! {
                biased;

                // Shutdown wins: drain what's already buffered, do a final flush, then exit.
                _ = self.shutdown.clone().wait() => {
                    while let Ok(ev) = self.rx.try_recv() {
                        batch.push(ev);
                    }
                    self.flush(&batch).await;
                    tracing::debug!("product event drain stopped on shutdown");
                    return;
                }

                // An event arrived, or every sender dropped (channel closed).
                maybe = self.rx.recv() => {
                    match maybe {
                        Some(ev) => {
                            batch.push(ev);
                            if batch.len() >= flush_rows {
                                self.flush(&batch).await;
                                batch.clear();
                            }
                        }
                        None => {
                            self.flush(&batch).await;
                            return;
                        }
                    }
                }

                // Time trigger: flush a partial batch so events don't languish.
                _ = ticker.tick() => {
                    if !batch.is_empty() {
                        self.flush(&batch).await;
                        batch.clear();
                    }
                }
            }
        }
    }

    /// Best-effort pipelined `XADD`, one entry per event, in a single round trip. A failed
    /// serialization drops just that event (logged); a failed pipeline exec drops the whole
    /// batch (logged) — never block or retry, a poisoned batch must not wedge the drain.
    async fn flush(&self, batch: &[ProductEvent]) {
        if batch.is_empty() {
            return;
        }

        let maxlen = redis::streams::StreamMaxlen::Approx(self.config.stream_maxlen());
        let mut pipe = redis::pipe();
        let mut queued = 0usize;
        for ev in batch {
            match serde_json::to_string(&ev.to_wire_json()) {
                Ok(payload) => {
                    pipe.xadd_maxlen(STREAM_KEY, maxlen, "*", &[("data", payload.as_str())])
                        .ignore();
                    queued += 1;
                }
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        event_type = ev.event_type.as_str(),
                        "product event dropped: failed to serialize"
                    );
                }
            }
        }

        if queued == 0 {
            return;
        }

        let mut conn = self.manager.clone();
        if let Err(e) = pipe.query_async::<()>(&mut conn).await {
            tracing::warn!(
                error = %e,
                rows = queued,
                "product event batch XADD failed; dropping batch"
            );
        }
    }
}

#[derive(Clone)]
pub struct ProductEventSink {
    tx: mpsc::Sender<ProductEvent>, // no Option
}

impl ProductEventSink {
    pub fn new(tx: mpsc::Sender<ProductEvent>) -> Self {
        Self { tx }
    }

    pub fn emit(&self, event: ProductEvent) {
        match self.tx.try_send(event) {
            Ok(()) => {}
            Err(TrySendError::Full(dropped)) => {
                tracing::warn!(
                    event_type = dropped.event_type.as_str(),
                    "product event dropped: channel full"
                );
            }
            Err(TrySendError::Closed(_)) => {
                tracing::warn!("product event dropped: channel closed");
            }
        }
    }
}

/// Bridges `runtara-connections` lifecycle events into product-analytics events.
///
/// The connections crate can't depend on this module (circular dependency), so it defines a
/// `ConnectionEventSink` trait and the host implements it. This is that implementation: it
/// translates each [`runtara_connections::events::ConnectionLifecycleEvent`] into a
/// [`ProductEvent`] and forwards it onto the one shared [`ProductEventSink`].
///
/// Connection events are **tenant-scoped and system-attributed** — the crate boundary only
/// exposes a `TenantId`, no caller identity, so they carry no `user_id` (`actor_type=system`,
/// `source` unset). The integration / outcome lives in `properties`.
#[derive(Clone)]
pub struct ConnectionEventBridge {
    sink: ProductEventSink,
}

impl ConnectionEventBridge {
    pub fn new(sink: ProductEventSink) -> Self {
        Self { sink }
    }
}

impl runtara_connections::events::ConnectionEventSink for ConnectionEventBridge {
    fn emit(&self, event: runtara_connections::events::ConnectionLifecycleEvent) {
        use runtara_connections::events::ConnectionLifecycleEvent as Lifecycle;

        let product_event = match event {
            Lifecycle::Created {
                connection_id,
                integration,
            } => ProductEvent::new(EventType::ConnectionCreated)
                .no_user_actor("connections", ActorType::System)
                .resource(connection_id, "connection")
                .properties(serde_json::json!({ "integration": integration })),
            Lifecycle::Deleted { connection_id } => ProductEvent::new(EventType::ConnectionDeleted)
                .no_user_actor("connections", ActorType::System)
                .resource(connection_id, "connection"),
            Lifecycle::OAuthStarted { connection_id } => {
                ProductEvent::new(EventType::ConnectionOauthStarted)
                    .no_user_actor("connections", ActorType::System)
                    .resource(connection_id, "connection")
            }
            Lifecycle::OAuthCompleted { connection_id } => {
                ProductEvent::new(EventType::ConnectionOauthCompleted)
                    .no_user_actor("connections", ActorType::System)
                    .resource(connection_id, "connection")
            }
            Lifecycle::OAuthFailed { reason } => {
                ProductEvent::new(EventType::ConnectionOauthFailed)
                    .no_user_actor("connections", ActorType::System)
                    .properties(serde_json::json!({ "reason": reason }))
            }
            Lifecycle::TokenRefreshed {
                connection_id,
                integration,
                success,
            } => ProductEvent::new(EventType::ConnectionTokenRefreshed)
                .no_user_actor("connections", ActorType::System)
                .source(EventSource::Worker)
                .resource(connection_id, "connection")
                .properties(serde_json::json!({ "integration": integration, "success": success })),
        };

        self.sink.emit(product_event);
    }
}

/// Emit `quota.exceeded` iff `denial` is a numeric tier-limit breach
/// (`EntitlementDenial::LimitExceeded`) — this is the one place that decides
/// which denial *kinds* count as a quota. Feature-gate denials
/// (`FeatureRequired`/`AgentNotEnabled`) are a toggle, not a quota (no
/// "current usage" ever approaches them), so callers pass every denial
/// through unconditionally and this silently no-ops for those.
///
/// `event` should already carry the caller's actor/tenant/source
/// attribution (via `ProductEvent::from_auth` in an HTTP handler, or
/// `ProductEvent::new(..).no_user_actor(..)` for an engine-internal gate like
/// `maxConcurrentExecutions`) — this function only adds the quota-specific
/// `properties` and emits.
pub fn emit_quota_exceeded(
    events: &ProductEventSink,
    event: ProductEvent,
    denial: &crate::entitlement_error::EntitlementDenial,
) {
    if let crate::entitlement_error::EntitlementDenial::LimitExceeded { limit, maximum } = denial {
        events.emit(event.properties(serde_json::json!({
            "quota_type": limit,
            "maximum": maximum,
        })));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `ProductEvent` built directly via struct literal, bypassing `new()` so the tests
    /// don't depend on the global config singleton (`new` reads `config::tenant_id()`).
    fn sample_event() -> ProductEvent {
        ProductEvent {
            event_id: Uuid::new_v4(),
            occurred_at: Utc::now(),
            event_type: EventType::WorkflowCreated,
            event_version: 1,
            tenant_id: "tenant-1".to_string(),
            user_id: None,
            actor_id: None,
            actor_type: None,
            resource_id: None,
            resource_type: None,
            properties: serde_json::json!({}),
            session_id: None,
            request_id: None,
            source: None,
        }
    }

    // ---- enum -> wire string mappings ----

    #[test]
    fn event_type_as_str_maps_every_variant() {
        assert_eq!(EventType::ApiKeyCreated.as_str(), "api_key.created");
        assert_eq!(EventType::ApiKeyRevoked.as_str(), "api_key.revoked");
        assert_eq!(EventType::WorkflowCreated.as_str(), "workflow.created");
        assert_eq!(EventType::WorkflowUpdated.as_str(), "workflow.updated");
        assert_eq!(EventType::WorkflowDeleted.as_str(), "workflow.deleted");
        assert_eq!(EventType::WorkflowCompiled.as_str(), "workflow.compiled");
        assert_eq!(
            EventType::WorkflowVersionRegistered.as_str(),
            "workflow.version_registered"
        );
        assert_eq!(EventType::ExecutionStarted.as_str(), "execution.started");
        assert_eq!(
            EventType::ExecutionCompleted.as_str(),
            "execution.completed"
        );
        assert_eq!(EventType::ExecutionFailed.as_str(), "execution.failed");
        assert_eq!(
            EventType::ExecutionCancelled.as_str(),
            "execution.cancelled"
        );
        assert_eq!(
            EventType::AgentCapabilityUsed.as_str(),
            "agent.capability_used"
        );
        assert_eq!(
            EventType::AgentCapabilityTested.as_str(),
            "agent.capability_tested"
        );
        assert_eq!(EventType::QuotaExceeded.as_str(), "quota.exceeded");
    }

    #[test]
    fn actor_type_as_str_maps_every_variant() {
        assert_eq!(ActorType::User.as_str(), "user");
        assert_eq!(ActorType::ApiKey.as_str(), "api_key");
        assert_eq!(ActorType::System.as_str(), "system");
        assert_eq!(ActorType::Trigger.as_str(), "trigger");
    }

    #[test]
    fn event_source_as_str_maps_every_variant() {
        assert_eq!(EventSource::Api.as_str(), "api");
        assert_eq!(EventSource::Ui.as_str(), "ui");
        assert_eq!(EventSource::Mcp.as_str(), "mcp");
        assert_eq!(EventSource::Worker.as_str(), "worker");
    }

    // ---- builder methods ----

    #[test]
    fn user_actor_sets_all_three_who_fields() {
        let e = sample_event().user_actor("user-1", "jti-9", ActorType::ApiKey);
        assert_eq!(e.user_id.as_deref(), Some("user-1"));
        assert_eq!(e.actor_id.as_deref(), Some("jti-9"));
        assert_eq!(e.actor_type, Some(ActorType::ApiKey));
    }

    #[test]
    fn no_user_actor_leaves_user_id_none() {
        let e = sample_event().no_user_actor("trig-1", ActorType::Trigger);
        assert_eq!(e.user_id, None);
        assert_eq!(e.actor_id.as_deref(), Some("trig-1"));
        assert_eq!(e.actor_type, Some(ActorType::Trigger));
    }

    #[test]
    fn resource_binds_id_then_type_in_struct_order() {
        let e = sample_event().resource("res-1", "workflow");
        assert_eq!(e.resource_id.as_deref(), Some("res-1"));
        assert_eq!(e.resource_type.as_deref(), Some("workflow"));
    }

    #[test]
    fn properties_overrides_default() {
        let e = sample_event().properties(serde_json::json!({ "duration_ms": 42 }));
        assert_eq!(e.properties, serde_json::json!({ "duration_ms": 42 }));
    }

    #[test]
    fn product_event_serde_round_trips() {
        // The "hand the worker a pre-built event via the compilation queue" design depends on
        // ProductEvent surviving a JSON round-trip with every field (enums, options, properties).
        let mut e = sample_event();
        e.event_type = EventType::ExecutionFailed;
        e.event_version = 2;
        e.user_id = Some("user-1".to_string());
        e.actor_id = Some("jti-9".to_string());
        e.actor_type = Some(ActorType::ApiKey);
        e.resource_id = Some("wf-1".to_string());
        e.resource_type = Some("workflow".to_string());
        e.properties = serde_json::json!({ "duration_ms": 42, "success": false });
        e.session_id = Some("sess".to_string());
        e.source = Some(EventSource::Mcp);

        let json = serde_json::to_value(&e).expect("serialize");
        let back: ProductEvent = serde_json::from_value(json.clone()).expect("deserialize");
        // ProductEvent has no PartialEq; compare via re-serialization.
        assert_eq!(serde_json::to_value(&back).unwrap(), json);
    }

    #[test]
    fn trace_sets_session_and_request() {
        let e = sample_event().trace("sess-1", "req-1");
        assert_eq!(e.session_id.as_deref(), Some("sess-1"));
        assert_eq!(e.request_id.as_deref(), Some("req-1"));
    }

    #[test]
    fn source_sets_source() {
        let e = sample_event().source(EventSource::Mcp);
        assert_eq!(e.source, Some(EventSource::Mcp));
    }

    // ---- emit_quota_exceeded: only fires for LimitExceeded ----

    #[test]
    fn emit_quota_exceeded_fires_for_limit_exceeded() {
        let (tx, mut rx) = mpsc::channel(4);
        let sink = ProductEventSink::new(tx);
        let denial = crate::entitlement_error::EntitlementDenial::LimitExceeded {
            limit: "maxWorkflows",
            maximum: 5,
        };
        emit_quota_exceeded(&sink, sample_event(), &denial);
        let emitted = rx.try_recv().expect("event emitted");
        assert_eq!(emitted.event_type, EventType::WorkflowCreated); // sample_event()'s base type
        assert_eq!(emitted.properties["quota_type"], "maxWorkflows");
        assert_eq!(emitted.properties["maximum"], 5);
    }

    #[test]
    fn emit_quota_exceeded_ignores_feature_required() {
        let (tx, mut rx) = mpsc::channel(4);
        let sink = ProductEventSink::new(tx);
        let denial = crate::entitlement_error::EntitlementDenial::FeatureRequired(
            crate::entitlements::FeatureKey::Reports,
        );
        emit_quota_exceeded(&sink, sample_event(), &denial);
        assert!(
            rx.try_recv().is_err(),
            "a feature-gate denial must not emit quota.exceeded"
        );
    }

    #[test]
    fn emit_quota_exceeded_ignores_agent_not_enabled() {
        let (tx, mut rx) = mpsc::channel(4);
        let sink = ProductEventSink::new(tx);
        let denial =
            crate::entitlement_error::EntitlementDenial::AgentNotEnabled("openai".to_string());
        emit_quota_exceeded(&sink, sample_event(), &denial);
        assert!(
            rx.try_recv().is_err(),
            "an agent-allowlist denial must not emit quota.exceeded"
        );
    }

    #[test]
    fn builders_chain_and_compose() {
        let e = sample_event()
            .user_actor("u", "u", ActorType::User)
            .resource("r", "workflow")
            .source(EventSource::Api);
        assert_eq!(e.actor_type, Some(ActorType::User));
        assert_eq!(e.resource_type.as_deref(), Some("workflow"));
        assert_eq!(e.source, Some(EventSource::Api));
    }

    // ---- config defaults & overrides (no env mutation) ----

    #[test]
    fn config_defaults_apply_when_unset() {
        let cfg = ProductEventConfig::default();
        assert_eq!(cfg.channel_capacity(), DEFAULT_CHANNEL_CAPACITY);
        assert_eq!(cfg.flush_rows(), DEFAULT_FLUSH_ROWS);
        assert_eq!(
            cfg.flush_interval(),
            Duration::from_millis(DEFAULT_FLUSH_INTERVAL_MS)
        );
        assert_eq!(cfg.stream_maxlen(), DEFAULT_STREAM_MAXLEN);
    }

    #[test]
    fn config_overrides_take_effect() {
        let cfg = ProductEventConfig {
            channel_capacity: Some(5),
            flush_rows: Some(7),
            flush_interval_ms: Some(50),
            stream_maxlen: Some(9),
        };
        assert_eq!(cfg.channel_capacity(), 5);
        assert_eq!(cfg.flush_rows(), 7);
        assert_eq!(cfg.flush_interval(), Duration::from_millis(50));
        assert_eq!(cfg.stream_maxlen(), 9);
    }

    // ---- wire JSON (Valkey stream payload) uses dotted strings, not enum variant names ----

    #[test]
    fn to_wire_json_uses_dotted_strings_not_variant_names() {
        let e = sample_event()
            .user_actor("user-1", "jti-9", ActorType::ApiKey)
            .source(EventSource::Api);
        let wire = e.to_wire_json();
        assert_eq!(wire["event_id"], serde_json::json!(e.event_id));
        assert_eq!(wire["event_type"], "workflow.created");
        assert_eq!(wire["actor_type"], "api_key");
        assert_eq!(wire["source"], "api");
    }

    #[test]
    fn to_wire_json_encodes_unset_optionals_as_null() {
        let wire = sample_event().to_wire_json();
        assert!(wire["actor_type"].is_null());
        assert!(wire["source"].is_null());
        assert!(wire["user_id"].is_null());
    }

    // ---- sink emit (non-blocking, drop-on-backpressure) ----

    #[test]
    fn emit_enqueues_event() {
        let (tx, mut rx) = mpsc::channel(4);
        let sink = ProductEventSink::new(tx);
        sink.emit(sample_event());
        assert!(rx.try_recv().is_ok());
    }

    #[test]
    fn emit_drops_when_full_without_panicking() {
        let (tx, mut rx) = mpsc::channel(1);
        let sink = ProductEventSink::new(tx);
        sink.emit(sample_event()); // fills the single slot
        sink.emit(sample_event()); // channel full -> dropped, must not panic
        assert!(rx.try_recv().is_ok()); // the first event
        assert!(rx.try_recv().is_err()); // the second was dropped
    }

    #[test]
    fn emit_drops_when_closed_without_panicking() {
        let (tx, rx) = mpsc::channel(4);
        let sink = ProductEventSink::new(tx);
        drop(rx); // close the channel
        sink.emit(sample_event()); // closed -> dropped, must not panic
    }

    // ---- drain graceful shutdown (empty batch never touches Valkey) ----

    #[tokio::test]
    async fn drain_exits_on_shutdown_without_touching_valkey() {
        // `ConnectionManager::new` connects eagerly (unlike `PgPool::connect_lazy`), so this
        // needs a live Valkey purely to construct the drain — the empty final flush below never
        // issues a command against it either way. Skips cleanly without VALKEY_HOST, mirroring
        // `valkey_auth.rs` / `middleware::auth`'s `manager_or_skip!`.
        let Some(cfg) = crate::valkey::ValkeyConfig::from_env() else {
            eprintln!("Skipping test: VALKEY_HOST not set");
            return;
        };
        let client = redis::Client::open(cfg.connection_url()).expect("open valkey client");
        let manager = ConnectionManager::new(client)
            .await
            .expect("connect valkey");

        let (tx, rx) = mpsc::channel(8);
        let shutdown = ShutdownSignal::new();
        shutdown.trigger(); // request shutdown up front

        let drain = ProductEventDrain::new(manager, rx, ProductEventConfig::default(), shutdown);

        tokio::time::timeout(Duration::from_secs(1), drain.run())
            .await
            .expect("drain should exit promptly on shutdown");
        drop(tx);
    }

    // ---- flush actually XADDs the dotted wire JSON (live Valkey round trip) ----

    #[tokio::test]
    async fn flush_xadds_dotted_wire_json_to_stream() {
        use redis::AsyncCommands;

        let Some(cfg) = crate::valkey::ValkeyConfig::from_env() else {
            eprintln!("Skipping test: VALKEY_HOST not set");
            return;
        };
        let client = redis::Client::open(cfg.connection_url()).expect("open valkey client");
        let manager = ConnectionManager::new(client)
            .await
            .expect("connect valkey");

        // A unique marker (rather than a unique key) lets this test share the real,
        // fixed-name production stream key without colliding with concurrent test runs.
        let marker = Uuid::new_v4().to_string();
        let mut ev = sample_event();
        ev.properties = serde_json::json!({ "test_marker": marker });

        let (_tx, rx) = mpsc::channel(8);
        let drain = ProductEventDrain::new(
            manager.clone(),
            rx,
            ProductEventConfig::default(),
            ShutdownSignal::new(),
        );
        drain.flush(std::slice::from_ref(&ev)).await;

        let mut conn = manager;
        let reply: redis::streams::StreamRangeReply = conn
            .xrevrange_count(STREAM_KEY, "+", "-", 50)
            .await
            .expect("xrevrange");
        let found = reply.ids.into_iter().find(|entry| {
            entry
                .get::<String>("data")
                .is_some_and(|data| data.contains(&marker))
        });
        let entry = found.expect("flushed event should appear in the stream");
        let data: String = entry.get("data").expect("data field");
        let parsed: Value = serde_json::from_str(&data).expect("data is JSON");
        assert_eq!(parsed["event_type"], "workflow.created");
        assert_eq!(parsed["event_id"], serde_json::json!(ev.event_id));
        assert!(parsed["actor_type"].is_null());

        // Don't leave test junk in the shared stream.
        let _: () = conn.xdel(STREAM_KEY, &[&entry.id]).await.unwrap();
    }
}
