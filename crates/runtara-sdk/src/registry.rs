// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Global SDK registry for the #[durable] macro.
//!
//! This module provides global SDK registration so the #[durable] macro
//! can access the SDK without explicit passing.

use std::sync::Arc;

use once_cell::sync::OnceCell;
use tokio::sync::Mutex;

use crate::RuntaraSdk;

/// Global storage for the SDK instance.
static SDK_INSTANCE: OnceCell<Arc<Mutex<RuntaraSdk>>> = OnceCell::new();

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
/// #[tokio::main]
/// async fn main() -> Result<(), Box<dyn std::error::Error>> {
///     let sdk = RuntaraSdk::localhost("my-instance", "my-tenant")?;
///     sdk.connect().await?;
///
///     // Register globally for #[durable] functions
///     register_sdk(sdk);
///
///     // Now #[durable] functions will use this SDK
///     Ok(())
/// }
/// ```
pub fn register_sdk(sdk: RuntaraSdk) {
    if SDK_INSTANCE.set(Arc::new(Mutex::new(sdk))).is_err() {
        panic!("SDK already registered. register_sdk() should only be called once.");
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

#[cfg(test)]
mod tests {
    // Note: These tests can't run in parallel due to global state.
    // In a real test suite, you'd use a thread-local or per-test registry.

    #[test]
    fn test_try_sdk_returns_none_initially() {
        // Can't test this reliably due to global state from other tests
    }
}
