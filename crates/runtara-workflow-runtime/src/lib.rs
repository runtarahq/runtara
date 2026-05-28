// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Workflow runtime component for direct-emitted workflow Wasm.
//!
//! This component owns the SDK-facing lifecycle calls imported by direct
//! workflow logic components. It intentionally stays separate from the JSON
//! stdlib component so static composition can wire workflow logic to both
//! shared components without merging their responsibilities.

use std::sync::{MutexGuard, PoisonError};
use std::time::Duration;

use runtara_sdk::{RuntaraSdk, Signal, SignalType, register_sdk, sdk, try_sdk};

#[cfg(target_arch = "wasm32")]
#[allow(warnings)]
mod bindings;

fn sdk_error(error: impl std::fmt::Display) -> String {
    error.to_string()
}

fn poisoned<T>(error: PoisonError<MutexGuard<'_, T>>) -> String {
    format!("workflow runtime SDK lock poisoned: {error}")
}

fn ensure_sdk() -> Result<(), String> {
    if try_sdk().is_some() {
        return Ok(());
    }

    let mut sdk_instance = RuntaraSdk::from_env().map_err(sdk_error)?;
    sdk_instance.connect().map_err(sdk_error)?;
    sdk_instance.register(None).map_err(sdk_error)?;
    register_sdk(sdk_instance);
    Ok(())
}

fn with_sdk<T>(op: impl FnOnce(&RuntaraSdk) -> Result<T, String>) -> Result<T, String> {
    ensure_sdk()?;
    let guard = sdk().lock().map_err(poisoned)?;
    op(&guard)
}

fn with_sdk_mut<T>(op: impl FnOnce(&mut RuntaraSdk) -> Result<T, String>) -> Result<T, String> {
    ensure_sdk()?;
    let mut guard = sdk().lock().map_err(poisoned)?;
    op(&mut guard)
}

fn signal_is_cancel(signal: Option<Signal>) -> bool {
    signal.is_some_and(|signal| signal.signal_type == SignalType::Cancel)
}

pub fn load_input() -> Result<Vec<u8>, String> {
    with_sdk(|sdk| {
        sdk.load_input()
            .map(|input| input.unwrap_or_else(|| b"{}".to_vec()))
            .map_err(sdk_error)
    })
}

pub fn complete(output: &[u8]) -> Result<(), String> {
    with_sdk(|sdk| sdk.completed(output).map_err(sdk_error))
}

pub fn fail(error: &[u8]) -> Result<(), String> {
    let error = String::from_utf8_lossy(error);
    with_sdk(|sdk| sdk.failed(&error).map_err(sdk_error))
}

pub fn custom_event(kind: &str, payload: Vec<u8>) -> Result<(), String> {
    with_sdk(|sdk| sdk.custom_event(kind, payload).map_err(sdk_error))
}

pub fn heartbeat() -> Result<(), String> {
    with_sdk(|sdk| sdk.heartbeat().map_err(sdk_error))
}

pub fn is_cancelled() -> Result<bool, String> {
    if runtara_sdk::is_cancelled() {
        return Ok(true);
    }

    let cancelled = with_sdk_mut(|sdk| sdk.poll_signal().map(signal_is_cancel).map_err(sdk_error))?;

    if cancelled {
        runtara_sdk::acknowledge_cancellation();
    }

    Ok(cancelled)
}

pub fn durable_sleep(ms: u64) -> Result<(), String> {
    with_sdk(|sdk| {
        sdk.sleep(
            Duration::from_millis(ms),
            "__direct_workflow_runtime_durable_sleep",
            &[],
        )
        .map_err(sdk_error)
    })
}

#[cfg(target_arch = "wasm32")]
mod component {
    use super::bindings::exports::runtara::workflow_runtime::runtime::Guest;

    struct Component;

    impl Guest for Component {
        fn load_input() -> Result<Vec<u8>, String> {
            super::load_input()
        }

        fn complete(output: Vec<u8>) -> Result<(), String> {
            super::complete(&output)
        }

        fn fail(error: Vec<u8>) -> Result<(), String> {
            super::fail(&error)
        }

        fn custom_event(kind: String, payload: Vec<u8>) -> Result<(), String> {
            super::custom_event(&kind, payload)
        }

        fn heartbeat() -> Result<(), String> {
            super::heartbeat()
        }

        fn is_cancelled() -> Result<bool, String> {
            super::is_cancelled()
        }

        fn durable_sleep(ms: u64) -> Result<(), String> {
            super::durable_sleep(ms)
        }
    }

    super::bindings::export!(Component with_types_in super::bindings);
}

#[cfg(test)]
mod tests {
    use runtara_sdk::{Signal, SignalType};

    use super::{sdk_error, signal_is_cancel};

    #[test]
    fn sdk_errors_are_exposed_as_strings() {
        let error = sdk_error(std::io::Error::other("network unavailable"));

        assert_eq!(error, "network unavailable");
    }

    #[test]
    fn only_cancel_signals_are_terminal_cancellation() {
        let pause = Signal {
            signal_type: SignalType::Pause,
            payload: Vec::new(),
            checkpoint_id: None,
        };
        let cancel = Signal {
            signal_type: SignalType::Cancel,
            payload: Vec::new(),
            checkpoint_id: None,
        };

        assert!(!signal_is_cancel(None));
        assert!(!signal_is_cancel(Some(pause)));
        assert!(signal_is_cancel(Some(cancel)));
    }
}
