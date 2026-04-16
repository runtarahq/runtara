// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Shared state for instance handlers.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::persistence::Persistence;

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
        }
    }

    /// Create a new instance handler state with a concurrency cap.
    pub fn with_limits(persistence: Arc<dyn Persistence>, max_concurrent_instances: u32) -> Self {
        Self {
            persistence,
            max_concurrent_instances,
            draining: Arc::new(AtomicBool::new(false)),
        }
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
    fn test_instance_handler_state_new() {
        let persistence = Arc::new(MockPersistence::new());
        let state = InstanceHandlerState::new(persistence);
        // Just verify it compiles and persistence is accessible
        let _ = &state.persistence;
    }
}
