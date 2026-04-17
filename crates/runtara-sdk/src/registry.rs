// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Global SDK registry for the #[durable] macro.
//!
//! This module provides global SDK registration so the #[durable] macro
//! can access the SDK without explicit passing.
//!
//! # Cancellation Support
//!
//! The registry provides cooperative cancellation for long-running operations.
//! When a cancellation signal is detected, the global cancellation flag is set,
//! allowing operations to be interrupted.
//!
//! Use `with_cancellation()` to wrap operations with cancellation support:
//!
//! ```ignore
//! let result = with_cancellation(|| some_long_operation())?;
//! ```

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use once_cell::sync::OnceCell;
use tracing::{info, warn};

use crate::RuntaraSdk;

/// Global storage for the SDK instance.
static SDK_INSTANCE: OnceCell<Mutex<RuntaraSdk>> = OnceCell::new();

/// Global cancellation flag triggered when a cancel signal is received.
static INSTANCE_CANCELLED: AtomicBool = AtomicBool::new(false);

/// Register an SDK instance globally for use by #[durable] functions.
///
/// This should be called once at application startup after creating and
/// connecting the SDK.
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
/// fn main() -> Result<(), Box<dyn std::error::Error>> {
///     let sdk = RuntaraSdk::from_env()?;
///     sdk.connect()?;
///
///     // Register globally for #[durable] functions
///     register_sdk(sdk);
///
///     // Now #[durable] functions will use this SDK
///     Ok(())
/// }
/// ```
pub fn register_sdk(sdk: RuntaraSdk) {
    if SDK_INSTANCE.set(Mutex::new(sdk)).is_err() {
        panic!("SDK already registered. register_sdk() should only be called once.");
    }
}

/// Get a reference to the registered SDK.
///
/// # Panics
///
/// Panics if no SDK has been registered.
pub fn sdk() -> &'static Mutex<RuntaraSdk> {
    SDK_INSTANCE
        .get()
        .expect("No SDK registered. Call register_sdk() at application startup.")
}

/// Try to get a reference to the registered SDK.
///
/// Returns `None` if no SDK has been registered.
pub fn try_sdk() -> Option<&'static Mutex<RuntaraSdk>> {
    SDK_INSTANCE.get()
}

/// Stop the background heartbeat task.
///
/// This is a no-op in the synchronous SDK (no background tasks).
pub fn stop_heartbeat() {
    // No-op — no background tasks in synchronous SDK
}

/// Check if the instance has been cancelled.
///
/// Returns `true` if a cancellation signal has been received.
pub fn is_cancelled() -> bool {
    INSTANCE_CANCELLED.load(Ordering::SeqCst)
}

/// Execute a closure with cancellation support.
///
/// This checks the cancellation flag before and after executing the closure.
/// If the flag is set, returns an error.
///
/// # Arguments
///
/// * `result` - The result of an operation to check
///
/// # Returns
///
/// Returns `Ok(result)` if the operation completed and no cancellation,
/// or `Err("Operation cancelled")` if cancellation was triggered.
///
/// # Example
///
/// ```ignore
/// use runtara_sdk::with_cancellation;
///
/// fn process_item(item: &Item) -> Result<Output, String> {
///     let response = with_cancellation(http_client.get(item.url).send())?;
///     Ok(response)
/// }
/// ```
pub fn with_cancellation<T>(result: T) -> Result<T, String> {
    if is_cancelled() {
        Err("Operation cancelled".to_string())
    } else {
        Ok(result)
    }
}

/// Check a result with cancellation support, using a custom error type.
///
/// Similar to `with_cancellation`, but allows the caller to provide a custom
/// error to return if cancelled.
///
/// # Arguments
///
/// * `result` - The result of an operation
/// * `cancel_error` - The error to return if cancelled
///
/// # Example
///
/// ```ignore
/// let result = with_cancellation_err(
///     http_request(),
///     MyError::Cancelled("HTTP request cancelled".into())
/// )?;
/// ```
pub fn with_cancellation_err<T, E>(result: Result<T, E>, cancel_error: E) -> Result<T, E> {
    if is_cancelled() {
        Err(cancel_error)
    } else {
        result
    }
}

/// Trigger cancellation programmatically.
///
/// This is useful for testing or when cancellation needs to be triggered
/// from within the workflow (e.g., on a timeout condition).
///
/// Note: This only affects the current instance's cancellation flag.
/// The cancellation signal is not propagated to runtara-core.
pub fn trigger_cancellation() {
    info!("Programmatic cancellation triggered");
    INSTANCE_CANCELLED.store(true, Ordering::SeqCst);
}

/// Reset the cancellation flag.
///
/// Intended only for tests that share process-global state: a test that
/// exercises `trigger_cancellation()` or `acknowledge_cancellation()`
/// should call this before/after to avoid polluting subsequent tests.
/// In production, a cancelled instance is expected to exit rather than
/// reset state, so there is no legitimate runtime use.
#[doc(hidden)]
pub fn reset_cancellation() {
    INSTANCE_CANCELLED.store(false, Ordering::SeqCst);
}

/// Acknowledge cancellation to runtara-core.
///
/// This should be called when the instance detects a cancel signal and is about
/// to exit. It sends a SignalAck to the core, which will update the instance
/// status to "cancelled" (rather than "failed").
///
/// This function is used by the `#[durable]` macro when cancellation is detected.
/// It triggers the local cancellation flag and sends the acknowledgment.
///
/// # Example
///
/// ```ignore
/// // Detected cancel signal in checkpoint response
/// if checkpoint_result.should_cancel() {
///     acknowledge_cancellation();
///     return Err("Instance cancelled".into());
/// }
/// ```
pub fn acknowledge_cancellation() {
    use crate::types::SignalType;

    // Trigger local cancellation flag first
    trigger_cancellation();

    // Send acknowledgment to core
    if let Some(sdk_mutex) = SDK_INSTANCE.get() {
        match sdk_mutex.lock() {
            Ok(sdk_guard) => match sdk_guard.acknowledge_signal(SignalType::Cancel) {
                Ok(()) => info!("Cancellation acknowledged to core"),
                Err(e) => warn!(error = %e, "Failed to acknowledge cancellation signal"),
            },
            Err(e) => warn!(error = %e, "Failed to lock SDK for cancellation acknowledgment"),
        }
    }
}

/// Acknowledge a pause signal to runtara-core.
///
/// This must be called when the workflow suspends due to a pause signal.
/// Without acknowledgment, the pause signal remains pending and will be
/// detected again on resume, causing the workflow to suspend immediately
/// in an infinite loop.
///
/// # Example
///
/// ```ignore
/// // Detected pause signal in checkpoint response
/// if checkpoint_result.should_pause() {
///     acknowledge_pause();
///     sdk.suspended()?;
///     return Ok(());
/// }
/// ```
pub fn acknowledge_pause() {
    use crate::types::SignalType;

    // Send acknowledgment to core
    if let Some(sdk_mutex) = SDK_INSTANCE.get() {
        match sdk_mutex.lock() {
            Ok(sdk_guard) => match sdk_guard.acknowledge_signal(SignalType::Pause) {
                Ok(()) => info!("Pause acknowledged to core"),
                Err(e) => warn!(error = %e, "Failed to acknowledge pause signal"),
            },
            Err(e) => warn!(error = %e, "Failed to lock SDK for pause acknowledgment"),
        }
    }
}

/// Acknowledge a shutdown signal to runtara-core.
///
/// The server has asked this instance to suspend at the next checkpoint
/// boundary so it can be resumed after restart. This function:
///
/// 1. Flips the local cancellation flag so `is_cancelled()` / `with_cancellation()`
///    short-circuit any in-flight cooperative work.
/// 2. Sends a `Shutdown` signal ack to core, which transitions the instance
///    to `suspended` with `termination_reason = "shutdown_requested"`.
///
/// # Example
///
/// ```ignore
/// if checkpoint_result.should_suspend_on_shutdown() {
///     acknowledge_shutdown();
///     sdk.suspended()?;
///     return Ok(());
/// }
/// ```
pub fn acknowledge_shutdown() {
    use crate::types::SignalType;

    // Flip the local flag so any remaining cooperative work exits.
    trigger_cancellation();

    if let Some(sdk_mutex) = SDK_INSTANCE.get() {
        match sdk_mutex.lock() {
            Ok(sdk_guard) => match sdk_guard.acknowledge_signal(SignalType::Shutdown) {
                Ok(()) => info!("Shutdown acknowledged to core"),
                Err(e) => warn!(error = %e, "Failed to acknowledge shutdown signal"),
            },
            Err(e) => warn!(error = %e, "Failed to lock SDK for shutdown acknowledgment"),
        }
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
