use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::PgPool;
use tokio::sync::mpsc;
use tokio::sync::mpsc::error::TrySendError;

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
    // Execution
    ExecutionStarted,
    ExecutionCompleted,
    ExecutionFailed,
    // Triggers
    TriggerCreated,
    TriggerFired,
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
            EventType::ExecutionStarted => "execution.started",
            EventType::ExecutionCompleted => "execution.completed",
            EventType::ExecutionFailed => "execution.failed",
            EventType::TriggerCreated => "trigger.created",
            EventType::TriggerFired => "trigger.fired",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProductEvent {
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
}

/// Default in-memory channel capacity — events buffered before `emit` drops on backpressure.
const DEFAULT_CHANNEL_CAPACITY: usize = 10_000;
/// Default number of buffered rows that force a flush.
const DEFAULT_FLUSH_ROWS: usize = 200;
/// Default max time a partial batch waits before flushing.
const DEFAULT_FLUSH_INTERVAL_MS: u64 = 2_000;

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
}

impl ProductEventConfig {
    /// Read each tunable from its own env var. Unset/unparseable → `None` (default applies).
    pub fn from_env() -> Self {
        Self {
            channel_capacity: parse_env("RUNTARA_PRODUCT_EVENTS_CHANNEL_CAPACITY"),
            flush_rows: parse_env("RUNTARA_PRODUCT_EVENTS_FLUSH_ROWS"),
            flush_interval_ms: parse_env("RUNTARA_PRODUCT_EVENTS_FLUSH_INTERVAL_MS"),
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
}

/// Read and parse a single env var, returning `None` if unset or unparseable.
fn parse_env<T: std::str::FromStr>(key: &str) -> Option<T> {
    std::env::var(key).ok().and_then(|v| v.parse().ok())
}

/// Single background consumer of the product-event channel. Owns the `Receiver`, accumulates
/// events into a batch, and writes each batch to Postgres in one multi-row INSERT.
pub struct ProductEventDrain {
    pool: PgPool,
    rx: mpsc::Receiver<ProductEvent>,
    config: ProductEventConfig,
    shutdown: ShutdownSignal,
}

impl ProductEventDrain {
    pub fn new(
        pool: PgPool,
        rx: mpsc::Receiver<ProductEvent>,
        config: ProductEventConfig,
        shutdown: ShutdownSignal,
    ) -> Self {
        Self {
            pool,
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

    /// Best-effort multi-row INSERT. `event_id`/`ingested_at` are left to DB defaults. On
    /// error, log the dropped count and move on — never block or retry (a poisoned batch must
    /// not wedge the drain).
    async fn flush(&self, batch: &[ProductEvent]) {
        if batch.is_empty() {
            return;
        }

        let mut qb = sqlx::QueryBuilder::new(
            "INSERT INTO product_events \
             (occurred_at, event_type, event_version, tenant_id, user_id, actor_id, actor_type, \
              resource_id, resource_type, properties, session_id, request_id, source) ",
        );
        qb.push_values(batch, |mut b, ev| {
            b.push_bind(ev.occurred_at)
                .push_bind(ev.event_type.as_str())
                .push_bind(ev.event_version)
                .push_bind(ev.tenant_id.as_str())
                .push_bind(ev.user_id.as_deref())
                .push_bind(ev.actor_id.as_deref())
                .push_bind(ev.actor_type.map(|a| a.as_str()))
                .push_bind(ev.resource_id.as_deref())
                .push_bind(ev.resource_type.as_deref())
                .push_bind(ev.properties.clone())
                .push_bind(ev.session_id.as_deref())
                .push_bind(ev.request_id.as_deref())
                .push_bind(ev.source.map(|s| s.as_str()));
        });

        if let Err(e) = qb.build().execute(&self.pool).await {
            tracing::warn!(
                error = %e,
                rows = batch.len(),
                "product event batch insert failed; dropping batch"
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A `ProductEvent` built directly via struct literal, bypassing `new()` so the tests
    /// don't depend on the global config singleton (`new` reads `config::tenant_id()`).
    fn sample_event() -> ProductEvent {
        ProductEvent {
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
        assert_eq!(EventType::ExecutionStarted.as_str(), "execution.started");
        assert_eq!(
            EventType::ExecutionCompleted.as_str(),
            "execution.completed"
        );
        assert_eq!(EventType::ExecutionFailed.as_str(), "execution.failed");
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
    }

    #[test]
    fn config_overrides_take_effect() {
        let cfg = ProductEventConfig {
            channel_capacity: Some(5),
            flush_rows: Some(7),
            flush_interval_ms: Some(50),
        };
        assert_eq!(cfg.channel_capacity(), 5);
        assert_eq!(cfg.flush_rows(), 7);
        assert_eq!(cfg.flush_interval(), Duration::from_millis(50));
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

    // ---- drain graceful shutdown (empty batch never touches the pool) ----

    #[tokio::test]
    async fn drain_exits_on_shutdown_without_touching_db() {
        // Lazy pool never connects; an empty final flush returns before using it.
        let pool = PgPool::connect_lazy("postgres://localhost/dummy").unwrap();
        let (tx, rx) = mpsc::channel(8);
        let shutdown = ShutdownSignal::new();
        shutdown.trigger(); // request shutdown up front

        let drain = ProductEventDrain::new(pool, rx, ProductEventConfig::default(), shutdown);

        tokio::time::timeout(Duration::from_secs(1), drain.run())
            .await
            .expect("drain should exit promptly on shutdown");
        drop(tx);
    }
}
