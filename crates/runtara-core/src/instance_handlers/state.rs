// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Shared state for instance handlers.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::persistence::Persistence;

/// Notified as guest events cross the host boundary, for a host that counts them.
///
/// Exists because runtara-core cannot depend on the server that aggregates
/// these — the same dependency inversion the connections crate uses for its
/// lifecycle events. Core defines the shape; the host implements it.
///
/// The subtype is handed over verbatim and this crate reads nothing into it:
/// deciding which subtypes are worth counting needs the producer's vocabulary,
/// which belongs to the implementer, not to a durable-execution kernel.
///
/// Implementations run on the event path of every event of every run, so they
/// must be cheap and non-blocking. An atomic add is the intended cost; a lock
/// or any I/O here is a bug.
pub trait InstanceEventObserver: Send + Sync {
    /// An event was persisted, carrying the producer's subtype if it set one.
    fn on_event_persisted(&self, subtype: Option<&str>);
}

/// Shared state for instance handlers.
///
/// Contains the persistence implementation shared across all handlers.
pub struct InstanceHandlerState {
    /// Persistence implementation.
    pub persistence: Arc<dyn Persistence>,
    /// Max concurrent instances allowed (enforced at register time).
    /// 0 disables the check.
    pub max_concurrent_instances: u32,
    /// When set, new-instance registration is refused with
    /// `ERROR_SERVER_DRAINING`. In-flight handlers (checkpoint, event, signal
    /// ack) continue to serve so running instances can suspend cleanly.
    pub draining: Arc<AtomicBool>,
    /// Counts guest events for whoever is watching, if anyone is.
    ///
    /// `None` in every context that has no aggregator — tests, the SDK's
    /// embedded backend — so nothing is forced to supply one.
    pub event_observer: Option<Arc<dyn InstanceEventObserver>>,
}

impl InstanceHandlerState {
    /// Create a new instance handler state with the given persistence backend.
    ///
    /// Uses a disabled concurrency cap (0) — prefer `with_limits` for production.
    pub fn new(persistence: Arc<dyn Persistence>) -> Self {
        Self {
            persistence,
            max_concurrent_instances: 0,
            draining: Arc::new(AtomicBool::new(false)),
            event_observer: None,
        }
    }

    /// Create a new instance handler state with a concurrency cap.
    pub fn with_limits(persistence: Arc<dyn Persistence>, max_concurrent_instances: u32) -> Self {
        Self {
            persistence,
            max_concurrent_instances,
            draining: Arc::new(AtomicBool::new(false)),
            event_observer: None,
        }
    }

    /// Attach an observer that counts guest events as they arrive.
    pub fn with_event_observer(mut self, observer: Arc<dyn InstanceEventObserver>) -> Self {
        self.event_observer = Some(observer);
        self
    }

    /// Handle to the draining flag so external coordinators (server, environment)
    /// can request drain.
    pub fn draining_handle(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.draining)
    }

    /// Returns `true` when registration of NEW instances is being refused.
    pub fn is_draining(&self) -> bool {
        self.draining.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::instance_handlers::mock_persistence::MockPersistence;

    #[test]
    fn new_disables_the_concurrency_cap_and_starts_undrained() {
        let state = InstanceHandlerState::new(Arc::new(MockPersistence::new()));

        // 0 is the documented "no cap" value — a nonzero default here would
        // silently start refusing registrations.
        assert_eq!(state.max_concurrent_instances, 0);
        assert!(!state.is_draining());
    }

    #[test]
    fn with_limits_carries_the_concurrency_cap() {
        let state = InstanceHandlerState::with_limits(Arc::new(MockPersistence::new()), 12);

        assert_eq!(state.max_concurrent_instances, 12);
        assert!(!state.is_draining());
    }

    #[test]
    fn draining_handle_aliases_the_flag_the_state_reads() {
        let state = InstanceHandlerState::new(Arc::new(MockPersistence::new()));
        let handle = state.draining_handle();

        // The handle exists so an external coordinator can request drain; if it
        // were a copy rather than an alias, drain would be silently ignored.
        assert!(!state.is_draining());
        handle.store(true, Ordering::SeqCst);
        assert!(state.is_draining());

        handle.store(false, Ordering::SeqCst);
        assert!(!state.is_draining());
    }
}
