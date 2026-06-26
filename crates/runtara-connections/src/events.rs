//! Connection lifecycle events for product analytics.
//!
//! This crate cannot depend on the host's product-events pipeline (that would be a circular
//! dependency: the host depends on this crate). So it defines this thin observer interface
//! instead. The host (e.g. `runtara-server`) implements [`ConnectionEventSink`], translating
//! these into its own analytics events, and injects it via
//! [`crate::config::ConnectionsConfig::connection_events`]. The crate calls the sink at
//! connection lifecycle points.
//!
//! Dependency inversion: the low-level crate owns the *interface*, the host owns the
//! *implementation*. No `ProductEvent`/`ProductEventSink` types cross the crate boundary.

use std::sync::Arc;

/// A connection lifecycle event, expressed in this crate's own vocabulary. The host maps
/// these onto its product-analytics events.
#[derive(Debug, Clone)]
pub enum ConnectionLifecycleEvent {
    /// A connection was created. `integration` is the integration / connection-type id (the
    /// "which integrations users reach for" dimension).
    Created {
        connection_id: String,
        integration: Option<String>,
    },
    /// A connection was deleted (integration churn).
    Deleted { connection_id: String },
    /// A user began the OAuth authorization flow (funnel: started).
    OAuthStarted { connection_id: String },
    /// The OAuth callback completed successfully (funnel: completed).
    OAuthCompleted { connection_id: String },
    /// The OAuth flow failed (funnel: drop-off). `reason` is a short, non-sensitive description.
    OAuthFailed { reason: String },
    /// A connection's OAuth access token was refreshed via the refresh-token grant (long-term
    /// connection health). Fires only on an actual refresh (cache miss), not on every use.
    TokenRefreshed {
        connection_id: String,
        integration: String,
        success: bool,
    },
}

/// Sink the host implements to receive connection lifecycle events.
///
/// Implementations MUST be non-blocking and best-effort — emitting an analytics event must
/// never fail or slow the connection operation it observes.
pub trait ConnectionEventSink: Send + Sync {
    fn emit(&self, event: ConnectionLifecycleEvent);
}

/// Optional, cloneable handle stored in the connections config/state. `None` disables
/// connection analytics (e.g. in tests or hosts that don't wire it).
pub type ConnectionEvents = Option<Arc<dyn ConnectionEventSink>>;

/// Emit `event` through `events` if a sink is configured; a no-op otherwise.
pub fn emit(events: &ConnectionEvents, event: ConnectionLifecycleEvent) {
    if let Some(sink) = events {
        sink.emit(event);
    }
}
