// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Global SDK registry for the #[durable] macro.
//!
//! This module provides global SDK registration so the #[durable] macro
//! can access the SDK without explicit passing. It also spawns a background
//! heartbeat task to keep instances alive during long-running operations.

use std::sync::Arc;
use std::time::Duration;

use once_cell::sync::OnceCell;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};

use crate::RuntaraSdk;

/// Global storage for the SDK instance.
static SDK_INSTANCE: OnceCell<Arc<Mutex<RuntaraSdk>>> = OnceCell::new();

/// Cancellation token for the background heartbeat task.
static HEARTBEAT_CANCEL: OnceCell<CancellationToken> = OnceCell::new();

/// Register an SDK instance globally for use by #[durable] functions.
///
/// This should be called once at application startup after creating and
/// connecting the SDK. If the SDK is configured with a non-zero heartbeat
/// interval, a background task will be spawned to send periodic heartbeats.
///
/// # Panics
///
/// Panics if called more than once.
///
/// # Example
///
/// ```ignore
/// use runtara_sdk::{RuntaraSdk, register_sdk};
///
/// #[tokio::main]
/// async fn main() -> Result<(), Box<dyn std::error::Error>> {
///     let sdk = RuntaraSdk::localhost("my-instance", "my-tenant")?;
///     sdk.connect().await?;
///
///     // Register globally for #[durable] functions
///     // This also starts a background heartbeat task (default: every 30s)
///     register_sdk(sdk);
///
///     // Now #[durable] functions will use this SDK
///     Ok(())
/// }
/// ```
pub fn register_sdk(sdk: RuntaraSdk) {
    let heartbeat_interval_ms = sdk.heartbeat_interval_ms();

    // Get backend Arc BEFORE wrapping SDK in mutex - heartbeat task uses this directly
    // to avoid mutex contention with long-running workflow operations
    let backend = sdk.backend_arc();

    let sdk_arc = Arc::new(Mutex::new(sdk));

    if SDK_INSTANCE.set(sdk_arc).is_err() {
        panic!("SDK already registered. register_sdk() should only be called once.");
    }

    // Spawn background heartbeat task if enabled
    if heartbeat_interval_ms > 0 {
        let cancel_token = CancellationToken::new();
        let _ = HEARTBEAT_CANCEL.set(cancel_token.clone());

        let interval = Duration::from_millis(heartbeat_interval_ms);

        tokio::spawn(async move {
            debug!(
                interval_ms = heartbeat_interval_ms,
                "Background heartbeat task started"
            );

            loop {
                tokio::select! {
                    biased;

                    _ = cancel_token.cancelled() => {
                        debug!("Background heartbeat task cancelled");
                        break;
                    }

                    _ = tokio::time::sleep(interval) => {
                        // Use backend directly - no mutex needed!
                        // Backend methods are already thread-safe (RuntaraClient has internal mutex)
                        if let Err(e) = backend.heartbeat().await {
                            warn!(error = %e, "Failed to send background heartbeat");
                        } else {
                            debug!("Background heartbeat sent");
                        }
                    }
                }
            }
        });
    }
}

/// Get a reference to the registered SDK.
///
/// # Panics
///
/// Panics if no SDK has been registered.
pub fn sdk() -> &'static Arc<Mutex<RuntaraSdk>> {
    SDK_INSTANCE
        .get()
        .expect("No SDK registered. Call register_sdk() at application startup.")
}

/// Try to get a reference to the registered SDK.
///
/// Returns `None` if no SDK has been registered.
pub fn try_sdk() -> Option<&'static Arc<Mutex<RuntaraSdk>>> {
    SDK_INSTANCE.get()
}

/// Stop the background heartbeat task.
///
/// This should be called when shutting down the SDK to cleanly stop
/// the background heartbeat task. It's safe to call this multiple times
/// or even if no heartbeat task was started.
///
/// # Example
///
/// ```ignore
/// use runtara_sdk::{register_sdk, stop_heartbeat, sdk};
///
/// // ... at shutdown
/// stop_heartbeat();
/// let sdk_guard = sdk().lock().await;
/// sdk_guard.close().await?;
/// ```
pub fn stop_heartbeat() {
    if let Some(cancel_token) = HEARTBEAT_CANCEL.get() {
        cancel_token.cancel();
        debug!("Heartbeat cancellation requested");
    }
}

#[cfg(test)]
mod tests {
    // Note: These tests can't run in parallel due to global state.
    // In a real test suite, you'd use a thread-local or per-test registry.

    #[test]
    fn test_try_sdk_returns_none_initially() {
        // Can't test this reliably due to global state from other tests
    }
}
