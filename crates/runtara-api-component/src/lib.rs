// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! `runtara:api@1.0.0`: the workflow API as a component interface.
//!
//! Every export is one `/v1` endpoint, called through the guest SDK over
//! `wasi:http`. The compiler composes this component into artifacts built for
//! the workflow API ABI; guest code in other languages may import the same
//! interface. The WIT (`crates/runtara-wit/wit-p3/api`) is the contract.

/// An SDK failure as the API's error envelope. A request that got no API
/// answer is `unavailable`.
#[cfg(any(target_arch = "wasm32", test))]
fn envelope(error: runtara_workflow_sdk::Error) -> (String, String, Option<u64>, Option<String>) {
    match error {
        runtara_workflow_sdk::Error::Api(error) => (
            error.code.as_str().to_string(),
            error.message,
            error.retry_after_ms,
            error.details.map(|details| details.to_string()),
        ),
        runtara_workflow_sdk::Error::Transport(message) => {
            ("unavailable".to_string(), message, None, None)
        }
    }
}

/// JSON bytes of a request as a value; malformed JSON is the caller's
/// mistake.
#[cfg(any(target_arch = "wasm32", test))]
fn json(bytes: &[u8], what: &str) -> Result<serde_json::Value, String> {
    serde_json::from_slice(bytes).map_err(|e| format!("{what} is not JSON: {e}"))
}

/// An optional JSON text field as a value (a string when it is not JSON).
#[cfg(any(target_arch = "wasm32", test))]
fn json_text(text: Option<String>) -> Option<serde_json::Value> {
    text.map(|text| serde_json::from_str(&text).unwrap_or(serde_json::Value::String(text)))
}

#[cfg(target_arch = "wasm32")]
mod bindings {
    wit_bindgen::generate!({
        path: ["../runtara-wit/wit-p3", "../runtara-wit/wit-p3/api"],
        world: "runtara:api/api",
        generate_all,
    });
}

#[cfg(target_arch = "wasm32")]
mod api {
    use runtara_workflow_api as wire;
    use runtara_workflow_sdk as sdk;

    use super::{envelope, json, json_text};
    use crate::bindings::exports::runtara::api::{
        checkpoints, connections, control, debug, events, inputs, operations, run, signals, state,
        time, trusted, types, waits,
    };
    use types::{ApiError, Wake};

    fn api_error(error: sdk::Error) -> ApiError {
        let (code, message, retry_after_ms, details) = envelope(error);
        ApiError {
            code,
            message,
            retry_after_ms,
            details,
        }
    }

    fn invalid(message: String) -> ApiError {
        ApiError {
            code: "invalid".into(),
            message,
            retry_after_ms: None,
            details: None,
        }
    }

    fn json_bytes(value: &serde_json::Value) -> Vec<u8> {
        serde_json::to_vec(value).unwrap_or_default()
    }

    fn object(
        bytes: &[u8],
        what: &str,
    ) -> Result<serde_json::Map<String, serde_json::Value>, ApiError> {
        match json(bytes, what).map_err(invalid)? {
            serde_json::Value::Object(map) => Ok(map),
            _ => Err(invalid(format!("{what} must be a JSON object"))),
        }
    }

    fn wake(wake: Wake) -> wire::Wake {
        match wake {
            Wake::At(at) => wire::Wake::At(at),
            Wake::OnSignal(wait) => wire::Wake::OnSignal(wire::SignalWait {
                checkpoint_id: wait.checkpoint_id,
                deadline_ms: wait.deadline_ms,
            }),
            Wake::OnResume => wire::Wake::OnResume,
            Wake::Instances(id) => wire::Wake::Instances(id),
        }
    }

    fn notice(notice: wire::LifecycleNotice) -> signals::LifecycleNotice {
        signals::LifecycleNotice {
            signal_type: notice.signal_type,
            command_id: notice.command_id,
            checkpoint_id: notice.checkpoint_id,
        }
    }

    fn input_read(read: wire::InputRead) -> inputs::InputRead {
        match read {
            wire::InputRead::Open => inputs::InputRead::Open,
            wire::InputRead::Accepted { value } => inputs::InputRead::Accepted(json_bytes(&value)),
            wire::InputRead::Closed { reason } => inputs::InputRead::Closed(reason),
        }
    }

    struct Api;

    impl run::Guest for Api {
        async fn info() -> Result<run::RunInfo, ApiError> {
            let info = sdk::run_info().await.map_err(api_error)?;
            Ok(run::RunInfo {
                instance_id: info.instance_id,
                tenant_id: info.tenant_id,
                workflow_id: info.workflow_id,
                version: info.version,
                attempt: info.attempt,
                launch_kind: info.launch_kind,
                debug: info.debug,
                deadline_ms: info.deadline_ms,
                api_version: info.api_version,
                capability_id: info.capability_id,
            })
        }

        async fn input() -> Result<Vec<u8>, ApiError> {
            sdk::input().await.map_err(api_error)
        }

        async fn post_outcome(outcome: run::Outcome) -> Result<(), ApiError> {
            let outcome = match outcome {
                run::Outcome::Completed(output) => wire::PostedOutcome::Completed(output),
                run::Outcome::Failed(error) => wire::PostedOutcome::Failed(wire::ErrorInfo {
                    code: error.code,
                    message: error.message,
                    category: error.category,
                    severity: error.severity,
                    retryable: error.retryable,
                    retry_after_ms: error.retry_after_ms,
                    attributes: json_text(error.attributes),
                    details: json_text(error.details),
                }),
                run::Outcome::Suspended(wakes) => {
                    wire::PostedOutcome::Suspended(wire::Suspension {
                        wakes: wakes.into_iter().map(wake).collect(),
                    })
                }
            };
            sdk::post_outcome(outcome).await.map_err(api_error)
        }

        async fn set_deadline(abort_after_ms: u64) -> Result<(), ApiError> {
            sdk::set_deadline(abort_after_ms).await.map_err(api_error)
        }
    }

    impl time::Guest for Api {
        async fn now_ms() -> Result<u64, ApiError> {
            sdk::now_ms().await.map_err(api_error)
        }

        async fn heartbeat() -> Result<(), ApiError> {
            sdk::heartbeat().await.map_err(api_error)
        }
    }

    impl signals::Guest for Api {
        async fn poll_lifecycle() -> Result<Option<signals::LifecycleSignal>, ApiError> {
            Ok(sdk::poll_lifecycle_signal()
                .await
                .map_err(api_error)?
                .map(|signal| signals::LifecycleSignal {
                    notice: notice(signal.notice),
                    payload: signal.payload,
                }))
        }

        async fn ack_lifecycle(command_id: String, signal_type: String) -> Result<bool, ApiError> {
            sdk::ack_lifecycle_signal(&command_id, &signal_type)
                .await
                .map_err(api_error)
        }

        async fn poll_custom(
            checkpoint_id: String,
            wait_ms: u64,
        ) -> Result<Option<Vec<u8>>, ApiError> {
            sdk::poll_custom_signal(&checkpoint_id, wait_ms)
                .await
                .map_err(api_error)
        }
    }

    impl checkpoints::Guest for Api {
        async fn get(checkpoint_id: String) -> Result<Option<Vec<u8>>, ApiError> {
            sdk::get_checkpoint(&checkpoint_id).await.map_err(api_error)
        }

        async fn put(
            checkpoint_id: String,
            state: Vec<u8>,
        ) -> Result<checkpoints::CheckpointWritten, ApiError> {
            let written = sdk::checkpoint(&checkpoint_id, state)
                .await
                .map_err(api_error)?;
            Ok(checkpoints::CheckpointWritten {
                stored: written.stored,
                state: written.state,
                pending_signal: written.pending_signal.map(notice),
                custom_signal: written
                    .custom_signal
                    .map(|signal| signals::CustomSignalNotice {
                        signal_id: signal.signal_id,
                        checkpoint_id: signal.checkpoint_id,
                    }),
            })
        }

        async fn record_retry_attempt(
            checkpoint_id: String,
            attempt_number: u32,
            error_message: Option<String>,
        ) -> Result<(), ApiError> {
            sdk::record_retry_attempt(&checkpoint_id, attempt_number, error_message.as_deref())
                .await
                .map_err(api_error)
        }
    }

    impl events::Guest for Api {
        async fn append(events: Vec<events::Event>) -> Result<(), ApiError> {
            let events = events
                .into_iter()
                .map(|event| {
                    Ok(wire::Event {
                        kind: event.kind,
                        payload: json(&event.payload, "an event payload").map_err(invalid)?,
                    })
                })
                .collect::<Result<Vec<_>, ApiError>>()?;
            sdk::emit_events(&events).await.map_err(api_error)
        }
    }

    impl inputs::Guest for Api {
        async fn register(descriptor: Vec<u8>, deadline_ms: Option<u64>) -> Result<(), ApiError> {
            let descriptor = object(&descriptor, "an input descriptor")?;
            sdk::register_input(&descriptor, deadline_ms)
                .await
                .map_err(api_error)
        }

        async fn poll(signal_id: String, wait_ms: u64) -> Result<inputs::InputRead, ApiError> {
            sdk::poll_input(&signal_id, wait_ms)
                .await
                .map(input_read)
                .map_err(api_error)
        }

        async fn close(signal_id: String) -> Result<inputs::InputRead, ApiError> {
            sdk::close_input(&signal_id)
                .await
                .map(input_read)
                .map_err(api_error)
        }
    }

    impl waits::Guest for Api {
        async fn register(
            key: String,
            instance_ids: Vec<String>,
            mode: waits::WaitMode,
            deadline_ms: Option<u64>,
        ) -> Result<Vec<u8>, ApiError> {
            let mode = match mode {
                waits::WaitMode::All => "all",
                waits::WaitMode::Any => "any",
            };
            sdk::register_wait(&key, &instance_ids, mode, deadline_ms)
                .await
                .map(|read| json_bytes(&read))
                .map_err(api_error)
        }

        async fn poll(key: String, wait_ms: u64) -> Result<Vec<u8>, ApiError> {
            sdk::poll_wait(&key, wait_ms)
                .await
                .map(|read| json_bytes(&read))
                .map_err(api_error)
        }

        async fn release(key: String) -> Result<(), ApiError> {
            sdk::release_wait(&key).await.map_err(api_error)
        }
    }

    impl state::Guest for Api {
        async fn set(key: String, patch: Vec<u8>) -> Result<(), ApiError> {
            let patch = object(&patch, "a state patch")?;
            sdk::set_state(&key, &patch).await.map_err(api_error)
        }

        async fn get(key: String) -> Result<Vec<u8>, ApiError> {
            sdk::get_state(&key)
                .await
                .map(|state| json_bytes(&serde_json::Value::Object(state)))
                .map_err(api_error)
        }
    }

    impl operations::Guest for Api {
        async fn enter(
            checkpoint_key: String,
            attempt: u32,
            load: bool,
        ) -> Result<operations::Entered, ApiError> {
            let (operation_id, continuation) = sdk::enter_operation(&checkpoint_key, attempt, load)
                .await
                .map_err(api_error)?;
            Ok(operations::Entered {
                operation_id,
                continuation,
            })
        }

        async fn suspend(
            operation_id: String,
            state: Vec<u8>,
            wakes: Vec<Wake>,
        ) -> Result<(), ApiError> {
            let wakes: Vec<_> = wakes.into_iter().map(wake).collect();
            sdk::suspend_operation(&operation_id, state, &wakes)
                .await
                .map_err(api_error)
        }

        async fn exit(operation_id: String, failed: bool) -> Result<(), ApiError> {
            sdk::exit_operation(&operation_id, failed)
                .await
                .map_err(api_error)
        }

        async fn release(operation_id: String) -> Result<(), ApiError> {
            sdk::release_operation(&operation_id)
                .await
                .map_err(api_error)
        }
    }

    impl connections::Guest for Api {
        async fn describe(connection_id: String) -> Result<Vec<u8>, ApiError> {
            sdk::describe_connection(&connection_id)
                .await
                .map(|descriptor| json_bytes(&descriptor))
                .map_err(api_error)
        }

        async fn resolve_resource(
            connection_id: String,
            request: Vec<u8>,
        ) -> Result<Vec<u8>, ApiError> {
            let request = json(&request, "a resource request").map_err(invalid)?;
            sdk::resolve_resource(&connection_id, &request)
                .await
                .map(|resolved| json_bytes(&resolved))
                .map_err(api_error)
        }

        async fn sql(
            connection_id: String,
            operation: connections::SqlOperation,
            request: Vec<u8>,
        ) -> Result<Vec<u8>, ApiError> {
            let operation = match operation {
                connections::SqlOperation::Query => wire::SqlOperation::Query,
                connections::SqlOperation::Execute => wire::SqlOperation::Execute,
                connections::SqlOperation::Batch => wire::SqlOperation::Batch,
            };
            let request = json(&request, "a SQL request").map_err(invalid)?;
            sdk::sql(&connection_id, operation, &request)
                .await
                .map(|result| json_bytes(&result))
                .map_err(api_error)
        }

        async fn http(
            request: connections::OutboundRequest,
            body: Option<Vec<u8>>,
        ) -> Result<Result<connections::OutboundReply, connections::OutboundFailure>, ApiError>
        {
            let destination = match request.destination {
                connections::Destination::Connection(target) => {
                    wire::OutboundDestination::Connection(wire::ConnectionTarget {
                        connection_id: target.connection_id,
                        url: target.url,
                        endpoint: target.endpoint,
                        endpoint_ref: target.endpoint_ref,
                        ai_provider: target.ai_provider,
                        aws_service: target.aws_service,
                    })
                }
                connections::Destination::Public(url) => wire::OutboundDestination::Public(url),
            };
            let meta = wire::OutboundMeta {
                method: request.method,
                destination,
                headers: request.headers,
                timeout_ms: request.timeout_ms,
                max_response_bytes: request.max_response_bytes,
            };
            Ok(match sdk::http(&meta, body).await.map_err(api_error)? {
                sdk::Outbound::Reply {
                    status,
                    headers,
                    body,
                } => Ok(connections::OutboundReply {
                    status,
                    headers,
                    body,
                }),
                sdk::Outbound::Failed { failure, body } => Err(connections::OutboundFailure {
                    code: failure.code,
                    message: failure.message,
                    status: failure.status,
                    retry_after_ms: failure.retry_after_ms,
                    body,
                }),
            })
        }
    }

    async fn call_control(
        call: wire::ControlCall,
        operation_id: Option<String>,
    ) -> Result<Vec<u8>, ApiError> {
        sdk::control(&call, operation_id.as_deref())
            .await
            .map(|answer| json_bytes(&answer))
            .map_err(api_error)
    }

    fn body(bytes: &[u8], what: &str) -> Result<serde_json::Value, ApiError> {
        json(bytes, what).map_err(invalid)
    }

    impl control::Guest for Api {
        async fn start(
            request: Vec<u8>,
            operation_id: Option<String>,
        ) -> Result<Vec<u8>, ApiError> {
            call_control(
                wire::ControlCall::Start(body(&request, "a start request")?),
                operation_id,
            )
            .await
        }

        async fn get(instance_id: String) -> Result<Vec<u8>, ApiError> {
            call_control(wire::ControlCall::Get(instance_id), None).await
        }

        async fn get_state(instance_id: String) -> Result<Vec<u8>, ApiError> {
            call_control(wire::ControlCall::GetState(instance_id), None).await
        }

        async fn query(request: Vec<u8>) -> Result<Vec<u8>, ApiError> {
            call_control(wire::ControlCall::Query(body(&request, "a query")?), None).await
        }

        async fn pending_signals(request: Vec<u8>) -> Result<Vec<u8>, ApiError> {
            call_control(
                wire::ControlCall::PendingSignals(body(&request, "a pending-signals request")?),
                None,
            )
            .await
        }

        async fn send_signal(
            instance_id: String,
            request: Vec<u8>,
            operation_id: Option<String>,
        ) -> Result<Vec<u8>, ApiError> {
            call_control(
                wire::ControlCall::SendSignal(instance_id, body(&request, "a signal")?),
                operation_id,
            )
            .await
        }

        async fn cancel(
            instance_id: String,
            request: Option<Vec<u8>>,
            operation_id: Option<String>,
        ) -> Result<Vec<u8>, ApiError> {
            let request = match request {
                Some(request) => body(&request, "a cancel request")?,
                None => serde_json::Value::Null,
            };
            call_control(
                wire::ControlCall::Cancel(instance_id, request),
                operation_id,
            )
            .await
        }

        async fn pause(
            instance_id: String,
            operation_id: Option<String>,
        ) -> Result<Vec<u8>, ApiError> {
            call_control(wire::ControlCall::Pause(instance_id), operation_id).await
        }

        async fn resume(
            instance_id: String,
            operation_id: Option<String>,
        ) -> Result<Vec<u8>, ApiError> {
            call_control(wire::ControlCall::Resume(instance_id), operation_id).await
        }
    }

    impl trusted::Guest for Api {
        async fn invoke(
            agent_id: String,
            capability_id: String,
            connection_id: String,
            input: Vec<u8>,
            operation_id: Option<String>,
        ) -> Result<Vec<u8>, ApiError> {
            let input = body(&input, "a trusted input")?;
            sdk::trusted(
                &agent_id,
                &capability_id,
                &connection_id,
                &input,
                operation_id.as_deref(),
            )
            .await
            .map(|answer| json_bytes(&answer))
            .map_err(api_error)
        }
    }

    impl debug::Guest for Api {
        async fn breakpoint(wait_ms: u64) -> Result<bool, ApiError> {
            sdk::breakpoint(wait_ms).await.map_err(api_error)
        }
    }

    crate::bindings::export!(Api with_types_in crate::bindings);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sdk_failures_become_the_envelope() {
        let error = runtara_workflow_api::ApiError::domain(
            "not-child",
            runtara_workflow_api::ErrorCode::Denied,
            "no",
        )
        .with_details(serde_json::json!({"x": 1}));
        let (code, message, _, details) = envelope(runtara_workflow_sdk::Error::Api(error));
        assert_eq!((code.as_str(), message.as_str()), ("not-child", "no"));
        assert_eq!(details.as_deref(), Some(r#"{"x":1}"#));
        let (code, ..) = envelope(runtara_workflow_sdk::Error::Transport("down".into()));
        assert_eq!(code, "unavailable");
        assert!(json(b"{", "a request").is_err());
        assert_eq!(
            json_text(Some("plain".into())),
            Some(serde_json::json!("plain"))
        );
    }
}
