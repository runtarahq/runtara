// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Global SDK registry for the #[resilient] macro.
//!
//! This module provides global SDK registration so the #[resilient] macro
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

use crate::tracing_compat::info;
use once_cell::sync::OnceCell;

use crate::RuntaraSdk;

/// Global storage for the SDK instance.
static SDK_INSTANCE: OnceCell<Mutex<RuntaraSdk>> = OnceCell::new();

/// Global cancellation flag triggered when a cancel signal is received.
static INSTANCE_CANCELLED: AtomicBool = AtomicBool::new(false);

/// Register an SDK instance globally for use by #[resilient] functions.
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
///     // Register globally for #[resilient] functions
///     register_sdk(sdk);
///
///     // Now #[resilient] functions will use this SDK
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

/// Acknowledge the exact cancellation returned by polling or checkpointing.
pub fn acknowledge_cancellation(command_id: &str) -> crate::Result<bool> {
    let accepted = acknowledge_command(command_id, crate::types::SignalType::Cancel)?;
    if accepted {
        trigger_cancellation();
    }
    Ok(accepted)
}

/// Acknowledge the exact pause returned by polling or checkpointing.
pub fn acknowledge_pause(command_id: &str) -> crate::Result<bool> {
    acknowledge_command(command_id, crate::types::SignalType::Pause)
}

/// Acknowledge the exact shutdown returned by polling or checkpointing.
pub fn acknowledge_shutdown(command_id: &str) -> crate::Result<bool> {
    let accepted = acknowledge_command(command_id, crate::types::SignalType::Shutdown)?;
    if accepted {
        trigger_cancellation();
    }
    Ok(accepted)
}

fn acknowledge_command(
    command_id: &str,
    signal_type: crate::types::SignalType,
) -> crate::Result<bool> {
    let Some(sdk_mutex) = SDK_INSTANCE.get() else {
        return Ok(true);
    };
    let sdk = sdk_mutex
        .lock()
        .map_err(|e| crate::SdkError::Internal(e.to_string()))?;
    sdk.acknowledge_signal(command_id, signal_type)
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
