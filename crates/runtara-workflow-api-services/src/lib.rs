// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! The `runtara:workflow` imports of compiled workflow logic, served over the
//! workflow API.
//!
//! The compiler composes this component in front of the logic of a workflow
//! built for the workflow API ABI, so the logic keeps its imports while the
//! launched artifact imports only WASI. The interface's functions are `async`
//! (the logic still calls them synchronously), so each export awaits its API
//! call. The host behind each endpoint is the same one that serves the old
//! import, so a call means the same thing on both ABIs.
//!
//! Not served yet, and refused loudly: durable sleep and breakpoints (their
//! endpoints are still to come).

#[cfg(target_arch = "wasm32")]
mod bindings {
    wit_bindgen::generate!({
        path: [
            "../runtara-wit/wit-p3",
            "../runtara-wit/wit/agent",
            "../runtara-wit/wit/workflow",
            "../runtara-wit/wit-p3/entry",
            "../runtara-wit/wit-p3/adapter",
        ],
        world: "runtara:workflow-adapter/services",
        generate_all,
    });
}

/// An SDK failure as the `string` error of the old interface. The API's error
/// envelope is kept as JSON so its code survives.
#[cfg(any(target_arch = "wasm32", test))]
fn message(error: runtara_workflow_sdk::Error) -> String {
    match error {
        runtara_workflow_sdk::Error::Api(error) => {
            serde_json::to_string(&error).unwrap_or(error.message)
        }
        runtara_workflow_sdk::Error::Transport(message) => message,
    }
}

/// Bytes an old call passes as JSON, as the JSON value they hold (a string
/// when they are not JSON).
#[cfg(any(target_arch = "wasm32", test))]
fn json_value(bytes: &[u8]) -> serde_json::Value {
    serde_json::from_slice(bytes)
        .unwrap_or_else(|_| serde_json::Value::String(String::from_utf8_lossy(bytes).into_owned()))
}

#[cfg(target_arch = "wasm32")]
mod services {
    use std::sync::atomic::{AtomicBool, Ordering};

    use runtara_workflow_sdk as sdk;

    use super::{json_value, message};
    use crate::bindings::exports::runtara::workflow::runtime::{
        CheckpointResult, CustomSignalInfo, Guest, InputState, SignalInfo,
    };

    /// Set once this run acknowledged a stop (cancel or shutdown), as the old
    /// host's `is-cancelled` remembers it.
    static STOPPED: AtomicBool = AtomicBool::new(false);

    fn not_served(what: &str) -> String {
        format!("{what} is not served on the workflow API yet")
    }

    fn signal(signal: sdk::LifecycleSignal) -> SignalInfo {
        SignalInfo {
            signal_type: signal.notice.signal_type,
            command_id: signal.notice.command_id,
            payload: signal.payload,
            checkpoint_id: signal.notice.checkpoint_id,
        }
    }

    fn input_state(read: runtara_workflow_api::InputRead) -> InputState {
        match read {
            runtara_workflow_api::InputRead::Open => InputState::Open,
            runtara_workflow_api::InputRead::Accepted { value } => {
                InputState::Accepted(serde_json::to_vec(&value).unwrap_or_default())
            }
            runtara_workflow_api::InputRead::Closed { reason } => InputState::Closed(reason),
        }
    }

    /// Acknowledge `command_id`; a stop it applied is remembered.
    async fn acknowledge(signal_type: &str, command_id: &str) -> Result<bool, String> {
        let applied = sdk::ack_lifecycle_signal(command_id, signal_type)
            .await
            .map_err(message)?;
        if applied && matches!(signal_type, "cancel" | "shutdown") {
            STOPPED.store(true, Ordering::SeqCst);
        }
        Ok(applied)
    }

    struct Services;

    impl Guest for Services {
        async fn instance_id() -> Result<String, String> {
            sdk::run_info()
                .await
                .map(|info| info.instance_id)
                .map_err(message)
        }

        async fn custom_event(kind: String, payload: Vec<u8>) -> Result<(), String> {
            let event = runtara_workflow_api::Event {
                kind,
                payload: json_value(&payload),
            };
            sdk::emit_events(&[event]).await.map_err(message)
        }

        async fn debug_mode_enabled() -> Result<bool, String> {
            sdk::run_info()
                .await
                .map(|info| info.debug)
                .map_err(message)
        }

        async fn breakpoint_pause() -> Result<(), String> {
            Err(not_served("breakpoint-pause"))
        }

        async fn heartbeat() -> Result<(), String> {
            sdk::heartbeat().await.map_err(message)
        }

        async fn poll_signal() -> Result<Option<SignalInfo>, String> {
            sdk::poll_lifecycle_signal()
                .await
                .map(|polled| polled.map(signal))
                .map_err(message)
        }

        async fn is_cancelled() -> Result<bool, String> {
            if STOPPED.load(Ordering::SeqCst) {
                return Ok(true);
            }
            match sdk::poll_lifecycle_signal().await.map_err(message)? {
                Some(polled) if polled.notice.signal_type == "cancel" => {
                    acknowledge("cancel", &polled.notice.command_id).await
                }
                _ => Ok(false),
            }
        }

        async fn check_signals() -> Result<bool, String> {
            match sdk::poll_lifecycle_signal().await.map_err(message)? {
                Some(polled) => {
                    acknowledge(&polled.notice.signal_type, &polled.notice.command_id).await
                }
                None => Ok(false),
            }
        }

        async fn poll_custom_signal(checkpoint_id: String) -> Result<Option<Vec<u8>>, String> {
            sdk::poll_custom_signal(&checkpoint_id, 0)
                .await
                .map_err(message)
        }

        async fn register_input(
            descriptor: Vec<u8>,
            deadline_ms: Option<u64>,
        ) -> Result<(), String> {
            let descriptor: serde_json::Map<String, serde_json::Value> =
                serde_json::from_slice(&descriptor)
                    .map_err(|e| format!("input descriptor is not a JSON object: {e}"))?;
            sdk::register_input(&descriptor, deadline_ms)
                .await
                .map(|_| ())
                .map_err(message)
        }

        async fn poll_input(signal_id: String) -> Result<InputState, String> {
            sdk::poll_input(&signal_id, 0)
                .await
                .map(input_state)
                .map_err(message)
        }

        async fn close_input(signal_id: String) -> Result<InputState, String> {
            // The old host traps when it cannot confirm the abandonment, so
            // guest error handling never continues with an orphaned request.
            match sdk::close_input(&signal_id).await {
                Ok(read) => Ok(input_state(read)),
                Err(_) => std::process::abort(),
            }
        }

        async fn now_ms() -> Result<u64, String> {
            sdk::now_ms().await.map_err(message)
        }

        async fn durable_sleep(_ms: u64) -> Result<(), String> {
            Err(not_served("durable-sleep"))
        }

        async fn blocking_sleep(ms: u64) -> Result<(), String> {
            sdk::sleep(ms).await;
            Ok(())
        }

        async fn get_checkpoint(checkpoint_id: String) -> Result<Option<Vec<u8>>, String> {
            sdk::get_checkpoint(&checkpoint_id).await.map_err(message)
        }

        async fn checkpoint(
            checkpoint_id: String,
            state: Vec<u8>,
        ) -> Result<CheckpointResult, String> {
            let written = sdk::checkpoint(&checkpoint_id, state)
                .await
                .map_err(message)?;
            // The write only announces signals; their payloads are read
            // without consuming them, as the old result carried them.
            let pending_signal = match written.pending_signal {
                Some(_) => sdk::poll_lifecycle_signal()
                    .await
                    .map_err(message)?
                    .map(signal),
                None => None,
            };
            let custom_signal = match written.custom_signal {
                Some(notice) => sdk::poll_custom_signal(&notice.checkpoint_id, 0)
                    .await
                    .map_err(message)?
                    .map(|payload| CustomSignalInfo {
                        signal_id: notice.signal_id,
                        checkpoint_id: notice.checkpoint_id,
                        payload,
                    }),
                None => None,
            };
            Ok(CheckpointResult {
                found: !written.stored,
                state: written.state,
                pending_signal,
                custom_signal,
            })
        }

        async fn handle_checkpoint_signal(
            signal_type: String,
            command_id: String,
        ) -> Result<bool, String> {
            acknowledge(&signal_type, &command_id).await
        }

        async fn record_retry_attempt(
            checkpoint_id: String,
            attempt_number: u32,
            error_message: Option<String>,
        ) -> Result<(), String> {
            sdk::record_retry_attempt(&checkpoint_id, attempt_number, error_message.as_deref())
                .await
                .map_err(message)
        }

        async fn durable_sleep_checkpoint(_: String, _: Vec<u8>, _: u64) -> Result<(), String> {
            Err(not_served("durable-sleep-checkpoint"))
        }
    }

    crate::bindings::export!(Services with_types_in crate::bindings);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn api_errors_keep_their_envelope_and_payloads_stay_json() {
        let error = runtara_workflow_sdk::Error::Api(runtara_workflow_api::ApiError::new(
            runtara_workflow_api::ErrorCode::Fenced,
            "lost the lease",
        ));
        let text = message(error);
        let envelope: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(envelope["code"], "fenced");
        assert_eq!(json_value(br#"{"a":1}"#), serde_json::json!({"a": 1}));
        assert_eq!(json_value(b"plain"), serde_json::json!("plain"));
    }
}
