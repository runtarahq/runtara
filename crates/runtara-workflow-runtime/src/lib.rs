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

use runtara_sdk::{
    CheckpointResult, CustomSignal, RuntaraSdk, Signal, SignalType, register_sdk, sdk, try_sdk,
};

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CheckpointSignalAction {
    Cancel,
    Pause,
    Shutdown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeSignalInfo {
    pub signal_type: String,
    pub payload: Vec<u8>,
    pub checkpoint_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeCustomSignalInfo {
    pub checkpoint_id: String,
    pub payload: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeCheckpointResult {
    pub found: bool,
    pub state: Vec<u8>,
    pub pending_signal: Option<RuntimeSignalInfo>,
    pub custom_signal: Option<RuntimeCustomSignalInfo>,
}

fn signal_type_name(signal_type: SignalType) -> &'static str {
    match signal_type {
        SignalType::Cancel => "cancel",
        SignalType::Pause => "pause",
        SignalType::Resume => "resume",
        SignalType::Shutdown => "shutdown",
    }
}

fn checkpoint_signal_action(signal_type: &str) -> Option<CheckpointSignalAction> {
    match signal_type {
        "cancel" => Some(CheckpointSignalAction::Cancel),
        "pause" => Some(CheckpointSignalAction::Pause),
        "shutdown" => Some(CheckpointSignalAction::Shutdown),
        _ => None,
    }
}

fn runtime_signal(signal: Signal) -> RuntimeSignalInfo {
    RuntimeSignalInfo {
        signal_type: signal_type_name(signal.signal_type).to_string(),
        payload: signal.payload,
        checkpoint_id: signal.checkpoint_id,
    }
}

fn runtime_custom_signal(signal: CustomSignal) -> RuntimeCustomSignalInfo {
    RuntimeCustomSignalInfo {
        checkpoint_id: signal.checkpoint_id,
        payload: signal.payload,
    }
}

fn runtime_checkpoint_result(result: CheckpointResult) -> RuntimeCheckpointResult {
    RuntimeCheckpointResult {
        found: result.found,
        state: result.state,
        pending_signal: result.pending_signal.map(runtime_signal),
        custom_signal: result.custom_signal.map(runtime_custom_signal),
    }
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
    durable_sleep_checkpoint("__direct_workflow_runtime_durable_sleep", &[], ms)
}

pub fn get_checkpoint(checkpoint_id: &str) -> Result<Option<Vec<u8>>, String> {
    with_sdk(|sdk| sdk.get_checkpoint(checkpoint_id).map_err(sdk_error))
}

pub fn checkpoint(checkpoint_id: &str, state: &[u8]) -> Result<RuntimeCheckpointResult, String> {
    with_sdk(|sdk| {
        sdk.checkpoint(checkpoint_id, state)
            .map(runtime_checkpoint_result)
            .map_err(sdk_error)
    })
}

pub fn handle_checkpoint_signal(signal_type: &str) -> Result<bool, String> {
    match checkpoint_signal_action(signal_type) {
        Some(CheckpointSignalAction::Cancel) => {
            runtara_sdk::acknowledge_cancellation();
            Ok(true)
        }
        Some(CheckpointSignalAction::Pause) => {
            runtara_sdk::acknowledge_pause();
            with_sdk(|sdk| sdk.suspended().map_err(sdk_error))?;
            Ok(true)
        }
        Some(CheckpointSignalAction::Shutdown) => {
            runtara_sdk::acknowledge_shutdown();
            with_sdk(|sdk| sdk.suspended().map_err(sdk_error))?;
            Ok(true)
        }
        None => Ok(false),
    }
}

pub fn record_retry_attempt(
    checkpoint_id: &str,
    attempt_number: u32,
    error_message: Option<&str>,
) -> Result<(), String> {
    with_sdk(|sdk| {
        sdk.record_retry_attempt(checkpoint_id, attempt_number, error_message)
            .map_err(sdk_error)
    })
}

pub fn durable_sleep_checkpoint(checkpoint_id: &str, state: &[u8], ms: u64) -> Result<(), String> {
    with_sdk(|sdk| {
        sdk.sleep(Duration::from_millis(ms), checkpoint_id, state)
            .map_err(sdk_error)
    })
}

#[cfg(target_arch = "wasm32")]
mod component {
    use super::bindings::exports::runtara::workflow_runtime::runtime::{
        CheckpointResult, CustomSignalInfo, Guest, SignalInfo,
    };

    struct Component;

    fn signal_info(signal: super::RuntimeSignalInfo) -> SignalInfo {
        SignalInfo {
            signal_type: signal.signal_type,
            payload: signal.payload,
            checkpoint_id: signal.checkpoint_id,
        }
    }

    fn custom_signal_info(signal: super::RuntimeCustomSignalInfo) -> CustomSignalInfo {
        CustomSignalInfo {
            checkpoint_id: signal.checkpoint_id,
            payload: signal.payload,
        }
    }

    fn checkpoint_result(result: super::RuntimeCheckpointResult) -> CheckpointResult {
        CheckpointResult {
            found: result.found,
            state: result.state,
            pending_signal: result.pending_signal.map(signal_info),
            custom_signal: result.custom_signal.map(custom_signal_info),
        }
    }

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

        fn get_checkpoint(checkpoint_id: String) -> Result<Option<Vec<u8>>, String> {
            super::get_checkpoint(&checkpoint_id)
        }

        fn checkpoint(checkpoint_id: String, state: Vec<u8>) -> Result<CheckpointResult, String> {
            super::checkpoint(&checkpoint_id, &state).map(checkpoint_result)
        }

        fn handle_checkpoint_signal(signal_type: String) -> Result<bool, String> {
            super::handle_checkpoint_signal(&signal_type)
        }

        fn record_retry_attempt(
            checkpoint_id: String,
            attempt_number: u32,
            error_message: Option<String>,
        ) -> Result<(), String> {
            super::record_retry_attempt(&checkpoint_id, attempt_number, error_message.as_deref())
        }

        fn durable_sleep_checkpoint(
            checkpoint_id: String,
            state: Vec<u8>,
            ms: u64,
        ) -> Result<(), String> {
            super::durable_sleep_checkpoint(&checkpoint_id, &state, ms)
        }
    }

    super::bindings::export!(Component with_types_in super::bindings);
}

#[cfg(test)]
mod tests {
    use runtara_sdk::{CheckpointResult, CustomSignal, Signal, SignalType};

    use super::{
        CheckpointSignalAction, checkpoint_signal_action, runtime_checkpoint_result, sdk_error,
        signal_is_cancel, signal_type_name,
    };

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

    #[test]
    fn signal_type_names_match_runtime_abi() {
        assert_eq!(signal_type_name(SignalType::Cancel), "cancel");
        assert_eq!(signal_type_name(SignalType::Pause), "pause");
        assert_eq!(signal_type_name(SignalType::Resume), "resume");
        assert_eq!(signal_type_name(SignalType::Shutdown), "shutdown");
    }

    #[test]
    fn checkpoint_signal_actions_match_lifecycle_signals() {
        assert_eq!(
            checkpoint_signal_action("cancel"),
            Some(CheckpointSignalAction::Cancel)
        );
        assert_eq!(
            checkpoint_signal_action("pause"),
            Some(CheckpointSignalAction::Pause)
        );
        assert_eq!(
            checkpoint_signal_action("shutdown"),
            Some(CheckpointSignalAction::Shutdown)
        );
        assert_eq!(checkpoint_signal_action("resume"), None);
        assert_eq!(checkpoint_signal_action("custom"), None);
    }

    #[test]
    fn checkpoint_result_converts_to_runtime_wire_shape() {
        let result = CheckpointResult {
            found: true,
            state: br#"{"ok":true}"#.to_vec(),
            pending_signal: Some(Signal {
                signal_type: SignalType::Pause,
                payload: b"pause-now".to_vec(),
                checkpoint_id: Some("step-a".to_string()),
            }),
            custom_signal: Some(CustomSignal {
                checkpoint_id: "wait-a".to_string(),
                payload: br#"{"resume":true}"#.to_vec(),
            }),
        };

        let wire = runtime_checkpoint_result(result);

        assert!(wire.found);
        assert_eq!(wire.state, br#"{"ok":true}"#);
        let signal = wire.pending_signal.expect("pending signal");
        assert_eq!(signal.signal_type, "pause");
        assert_eq!(signal.payload, b"pause-now");
        assert_eq!(signal.checkpoint_id.as_deref(), Some("step-a"));
        let custom = wire.custom_signal.expect("custom signal");
        assert_eq!(custom.checkpoint_id, "wait-a");
        assert_eq!(custom.payload, br#"{"resume":true}"#);
    }
}
