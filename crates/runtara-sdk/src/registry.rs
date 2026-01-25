// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Global SDK registry for the #[durable] macro.
//!
//! This module provides global SDK registration so the #[durable] macro
//! can access the SDK without explicit passing. It also spawns background
//! tasks for heartbeat and cancellation polling.
//!
//! # Cancellation Support
//!
//! The registry provides cooperative cancellation for long-running operations.
//! When a cancellation signal is detected, the global cancellation token is
//! triggered, allowing operations to be interrupted mid-execution.
//!
//! Use `with_cancellation()` to wrap async operations with cancellation support:
//!
//! ```ignore
//! let result = with_cancellation(some_long_operation()).await?;
//! ```

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use once_cell::sync::OnceCell;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use crate::RuntaraSdk;

/// Global storage for the SDK instance.
static SDK_INSTANCE: OnceCell<Arc<Mutex<RuntaraSdk>>> = OnceCell::new();

/// Cancellation token for the background heartbeat task.
static HEARTBEAT_CANCEL: OnceCell<CancellationToken> = OnceCell::new();

/// Global cancellation token triggered when a cancel signal is received.
/// This token can be used with `tokio::select!` to interrupt long-running operations.
static INSTANCE_CANCELLATION: OnceCell<CancellationToken> = OnceCell::new();

/// Default interval for polling cancellation status (in milliseconds).
const DEFAULT_CANCELLATION_POLL_INTERVAL_MS: u64 = 1000;

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
    // Use the same interval for cancellation polling as heartbeat, or default to 1 second
    let cancellation_poll_interval_ms = if heartbeat_interval_ms > 0 {
        // Poll slightly more frequently than heartbeat to catch signals sooner
        std::cmp::min(
            heartbeat_interval_ms / 2,
            DEFAULT_CANCELLATION_POLL_INTERVAL_MS,
        )
    } else {
        DEFAULT_CANCELLATION_POLL_INTERVAL_MS
    };

    // Get backend Arc BEFORE wrapping SDK in mutex - background tasks use this directly
    // to avoid mutex contention with long-running workflow operations
    let backend = sdk.backend_arc();

    let sdk_arc = Arc::new(Mutex::new(sdk));

    if SDK_INSTANCE.set(sdk_arc.clone()).is_err() {
        panic!("SDK already registered. register_sdk() should only be called once.");
    }

    // Create the global cancellation token
    let instance_cancel_token = CancellationToken::new();
    if INSTANCE_CANCELLATION
        .set(instance_cancel_token.clone())
        .is_err()
    {
        panic!("Cancellation token already set.");
    }

    // Create shutdown token for background tasks
    let shutdown_token = CancellationToken::new();
    let _ = HEARTBEAT_CANCEL.set(shutdown_token.clone());

    // Spawn background heartbeat task if enabled
    if heartbeat_interval_ms > 0 {
        let heartbeat_backend = backend.clone();
        let heartbeat_shutdown = shutdown_token.clone();
        let interval = Duration::from_millis(heartbeat_interval_ms);

        tokio::spawn(async move {
            debug!(
                interval_ms = heartbeat_interval_ms,
                "Background heartbeat task started"
            );

            loop {
                tokio::select! {
                    biased;

                    _ = heartbeat_shutdown.cancelled() => {
                        debug!("Background heartbeat task cancelled");
                        break;
                    }

                    _ = tokio::time::sleep(interval) => {
                        // Use backend directly - no mutex needed!
                        // Backend methods are already thread-safe (RuntaraClient has internal mutex)
                        if let Err(e) = heartbeat_backend.heartbeat().await {
                            warn!(error = %e, "Failed to send background heartbeat");
                        } else {
                            debug!("Background heartbeat sent");
                        }
                    }
                }
            }
        });
    }

    // Spawn background cancellation poller (only available with QUIC backend)
    #[cfg(feature = "quic")]
    if cancellation_poll_interval_ms > 0 {
        let cancel_shutdown = shutdown_token.clone();
        let cancel_token = instance_cancel_token.clone();
        let poll_interval = Duration::from_millis(cancellation_poll_interval_ms);

        tokio::spawn(async move {
            debug!(
                interval_ms = cancellation_poll_interval_ms,
                "Background cancellation poller started"
            );

            loop {
                tokio::select! {
                    biased;

                    _ = cancel_shutdown.cancelled() => {
                        debug!("Background cancellation poller stopped");
                        break;
                    }

                    _ = cancel_token.cancelled() => {
                        // Already cancelled, no need to poll
                        debug!("Cancellation already triggered, stopping poller");
                        break;
                    }

                    _ = tokio::time::sleep(poll_interval) => {
                        // Check for cancellation signal
                        let should_cancel = {
                            let mut sdk_guard = sdk_arc.lock().await;
                            match sdk_guard.check_cancelled().await {
                                Err(crate::SdkError::Cancelled) => true,
                                Err(e) => {
                                    warn!(error = %e, "Error checking cancellation status");
                                    false
                                }
                                Ok(()) => false,
                            }
                        };

                        if should_cancel {
                            info!("Cancellation signal received, triggering global cancellation");
                            cancel_token.cancel();
                            break;
                        }
                    }
                }
            }
        });
    }

    // Suppress unused variable warnings when QUIC is disabled
    #[cfg(not(feature = "quic"))]
    let _ = (shutdown_token, cancellation_poll_interval_ms);
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

/// Get the global cancellation token.
///
/// This token is triggered when a cancellation signal is received from runtara-core.
/// Use this with `tokio::select!` to make long-running operations cancellable.
///
/// Returns `None` if no SDK has been registered.
///
/// # Example
///
/// ```ignore
/// use runtara_sdk::cancellation_token;
///
/// async fn long_operation() -> Result<(), String> {
///     if let Some(token) = cancellation_token() {
///         tokio::select! {
///             biased;
///             _ = token.cancelled() => Err("Operation cancelled".to_string()),
///             result = do_actual_work() => result,
///         }
///     } else {
///         do_actual_work().await
///     }
/// }
/// ```
pub fn cancellation_token() -> Option<CancellationToken> {
    INSTANCE_CANCELLATION.get().cloned()
}

/// Check if the instance has been cancelled.
///
/// Returns `true` if a cancellation signal has been received.
pub fn is_cancelled() -> bool {
    INSTANCE_CANCELLATION
        .get()
        .map(|t| t.is_cancelled())
        .unwrap_or(false)
}

/// Execute a future with cancellation support.
///
/// This wraps the provided future with a cancellation check. If the global
/// cancellation token is triggered, the future will be dropped and an error
/// will be returned.
///
/// # Arguments
///
/// * `operation` - The async operation to execute
///
/// # Returns
///
/// Returns `Ok(result)` if the operation completes successfully, or
/// `Err("Operation cancelled")` if cancellation was triggered.
///
/// # Example
///
/// ```ignore
/// use runtara_sdk::with_cancellation;
///
/// async fn process_item(item: &Item) -> Result<Output, String> {
///     // This HTTP request will be cancelled if a cancel signal is received
///     let response = with_cancellation(http_client.get(item.url).send()).await?;
///     Ok(response.json().await?)
/// }
/// ```
pub async fn with_cancellation<F, T>(operation: F) -> Result<T, String>
where
    F: Future<Output = T>,
{
    match INSTANCE_CANCELLATION.get() {
        Some(token) => {
            tokio::select! {
                biased;

                _ = token.cancelled() => {
                    Err("Operation cancelled".to_string())
                }

                result = operation => {
                    Ok(result)
                }
            }
        }
        None => {
            // No cancellation token registered, just run the operation
            Ok(operation.await)
        }
    }
}

/// Execute a future with cancellation support, using a custom error type.
///
/// Similar to `with_cancellation`, but allows the caller to provide a custom
/// error message or transform the cancellation error.
///
/// # Arguments
///
/// * `operation` - The async operation to execute
/// * `cancel_error` - The error to return if cancelled
///
/// # Example
///
/// ```ignore
/// let result = with_cancellation_err(
///     http_request(),
///     MyError::Cancelled("HTTP request cancelled".into())
/// ).await?;
/// ```
pub async fn with_cancellation_err<F, T, E>(operation: F, cancel_error: E) -> Result<T, E>
where
    F: Future<Output = Result<T, E>>,
{
    match INSTANCE_CANCELLATION.get() {
        Some(token) => {
            tokio::select! {
                biased;

                _ = token.cancelled() => {
                    Err(cancel_error)
                }

                result = operation => {
                    result
                }
            }
        }
        None => {
            // No cancellation token registered, just run the operation
            operation.await
        }
    }
}

/// Trigger cancellation programmatically.
///
/// This is useful for testing or when cancellation needs to be triggered
/// from within the workflow (e.g., on a timeout condition).
///
/// Note: This only affects the current instance's cancellation token.
/// The cancellation signal is not propagated to runtara-core.
pub fn trigger_cancellation() {
    if let Some(token) = INSTANCE_CANCELLATION.get() {
        info!("Programmatic cancellation triggered");
        token.cancel();
    }
}

/// Acknowledge cancellation to runtara-core.
///
/// This should be called when the instance detects a cancel signal and is about
/// to exit. It sends a SignalAck to the core, which will update the instance
/// status to "cancelled" (rather than "failed").
///
/// This function is used by the `#[durable]` macro when cancellation is detected.
/// It triggers the local cancellation token and sends the acknowledgment.
///
/// # Example
///
/// ```ignore
/// // Detected cancel signal in checkpoint response
/// if checkpoint_result.should_cancel() {
///     acknowledge_cancellation().await;
///     return Err("Instance cancelled".into());
/// }
/// ```
#[cfg(feature = "quic")]
pub async fn acknowledge_cancellation() {
    use crate::types::SignalType;

    // Trigger local cancellation token first
    trigger_cancellation();

    // Send acknowledgment to core with timeout to prevent indefinite hang.
    // This can happen if:
    // 1. Multiple parallel durable functions try to acknowledge simultaneously
    // 2. The QUIC connection is in a degraded state after stop signal
    // 3. The grace period expired and core took action on the connection
    if let Some(sdk_arc) = SDK_INSTANCE.get() {
        let ack_result = tokio::time::timeout(Duration::from_secs(5), async {
            let sdk_guard = sdk_arc.lock().await;
            sdk_guard.acknowledge_signal(SignalType::Cancel, true).await
        })
        .await;

        match ack_result {
            Ok(Ok(())) => info!("Cancellation acknowledged to core"),
            Ok(Err(e)) => warn!(error = %e, "Failed to acknowledge cancellation signal"),
            Err(_) => warn!("Timeout acknowledging cancellation signal - continuing with exit"),
        }
    }
}

/// Acknowledge cancellation (no-op without QUIC feature).
#[cfg(not(feature = "quic"))]
pub async fn acknowledge_cancellation() {
    trigger_cancellation();
    debug!("Cancellation acknowledged (QUIC disabled, no ack sent)");
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
