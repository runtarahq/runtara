// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Guest SDK for the workflow API.
//!
//! A workflow on the workflow API ABI exports `wasi:cli/run@0.3.0` and talks
//! to the host only over `wasi:http@0.3.0` at
//! [`runtara_workflow_api::ORIGIN`]. This crate wraps that transport: it
//! builds the requests, sends a per-launch `Idempotency-Key` on every write,
//! and decodes the error envelope. It never names the run; the host attaches
//! the binding.
//!
//! This first slice covers the run lifecycle: [`run_info`], [`input`] and
//! [`post_outcome`]. Export a workflow with [`export_workflow!`]:
//!
//! ```ignore
//! struct Workflow;
//! impl runtara_workflow_sdk::bindings::exports::wasi::cli::run::Guest for Workflow {
//!     async fn run() -> Result<(), ()> { /* ... */ Ok(()) }
//! }
//! runtara_workflow_sdk::export_workflow!(Workflow);
//! ```

use runtara_workflow_api::{ApiError, ErrorCode};

pub use runtara_workflow_api::{ErrorInfo, PostedOutcome, RunInfo, Suspension, Wake};

#[cfg(target_arch = "wasm32")]
pub mod bindings {
    wit_bindgen::generate!({
        path: "../runtara-wit/wit-p3",
        world: "runtara:workflow-guest/workflow",
        pub_export_macro: true,
        export_macro_name: "export_workflow",
        default_bindings_module: "runtara_workflow_sdk::bindings",
        generate_all,
    });
}

#[cfg(target_arch = "wasm32")]
pub use bindings::export_workflow;

/// Why an SDK call failed.
#[derive(Debug)]
pub enum Error {
    /// The host answered with the workflow API's error envelope.
    Api(ApiError),
    /// The request never got an API answer (transport refused or broke).
    Transport(String),
}

impl Error {
    /// The API error code, when the host answered.
    pub fn code(&self) -> Option<&ErrorCode> {
        match self {
            Self::Api(error) => Some(&error.code),
            Self::Transport(_) => None,
        }
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Api(error) => write!(f, "{error}"),
            Self::Transport(message) => write!(f, "transport: {message}"),
        }
    }
}

impl std::error::Error for Error {}

/// A raw workflow API response.
#[derive(Debug, Clone)]
pub struct Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Response {
    /// The value of header `name`, if present.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    /// `Ok` for a 2xx answer, otherwise the decoded error envelope.
    pub fn into_result(self) -> Result<Self, Error> {
        if (200..300).contains(&self.status) {
            return Ok(self);
        }
        match serde_json::from_slice::<ApiError>(&self.body) {
            Ok(error) => Err(Error::Api(error)),
            Err(_) => Err(Error::Transport(format!(
                "workflow API answered {} without an error envelope",
                self.status
            ))),
        }
    }
}

/// The idempotency key of the next write: unique per launch, since every
/// launch is a fresh instance of the component.
fn next_idempotency_key() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    format!("sdk-{}", NEXT.fetch_add(1, Ordering::Relaxed))
}

#[cfg(target_arch = "wasm32")]
mod transport {
    use super::{Error, Response};
    use crate::bindings::wasi::http::client;
    use crate::bindings::wasi::http::types::{ErrorCode, Fields, Method, Request, Scheme};
    use crate::bindings::{wit_future, wit_stream};

    pub(crate) async fn raw(
        method: &str,
        path: &str,
        headers: &[(&str, &str)],
        body: Option<Vec<u8>>,
    ) -> Result<Response, Error> {
        let mut all: Vec<(String, Vec<u8>)> = vec![(
            runtara_workflow_api::headers::API_VERSION.to_string(),
            runtara_workflow_api::api_version().into_bytes(),
        )];
        all.extend(
            headers
                .iter()
                .map(|(name, value)| (name.to_string(), value.as_bytes().to_vec())),
        );
        let fields = Fields::from_list(&all)
            .map_err(|error| Error::Transport(format!("headers: {error:?}")))?;

        // Dropping the writer delivers its default, no trailers. It must go
        // before `send`: the host reads the whole body, trailers included,
        // before it answers.
        let (trailers_tx, trailers_rx) =
            wit_future::new::<Result<Option<Fields>, ErrorCode>>(|| Ok(None));
        drop(trailers_tx);
        let contents = body.map(|body| {
            let (mut tx, rx) = wit_stream::new::<u8>();
            // The host buffers the whole body before it answers, so the body
            // is written while `send` waits.
            wit_bindgen::spawn_local(async move {
                tx.write_all(body).await;
            });
            rx
        });
        let (request, _transmitted) = Request::new(fields, contents, trailers_rx, None);
        let method = match method {
            "GET" => Method::Get,
            "POST" => Method::Post,
            "PUT" => Method::Put,
            "PATCH" => Method::Patch,
            "DELETE" => Method::Delete,
            other => Method::Other(other.to_string()),
        };
        let set = request.set_method(&method).is_ok()
            && request.set_scheme(Some(&Scheme::Http)).is_ok()
            && request
                .set_authority(Some(runtara_workflow_api::HOST))
                .is_ok()
            && request.set_path_with_query(Some(path)).is_ok();
        if !set {
            return Err(Error::Transport("invalid request line".into()));
        }

        let response = client::send(request)
            .await
            .map_err(|error| Error::Transport(format!("{error:?}")))?;
        let status = response.get_status_code();
        let headers = response
            .get_headers()
            .copy_all()
            .into_iter()
            .map(|(name, value)| (name, String::from_utf8_lossy(&value).into_owned()))
            .collect();
        let (done_tx, done_rx) = wit_future::new::<Result<(), ErrorCode>>(|| Ok(()));
        drop(done_tx);
        let (body, _trailers) =
            crate::bindings::wasi::http::types::Response::consume_body(response, done_rx);
        let body = body.collect().await;
        Ok(Response {
            status,
            headers,
            body,
        })
    }

    pub(crate) async fn call(
        method: &str,
        path: &str,
        headers: &[(&str, &str)],
        body: Option<Vec<u8>>,
    ) -> Result<Response, Error> {
        raw(method, path, headers, body).await?.into_result()
    }
}

#[cfg(not(target_arch = "wasm32"))]
mod transport {
    use super::{Error, Response};

    pub(crate) async fn raw(
        _method: &str,
        _path: &str,
        _headers: &[(&str, &str)],
        _body: Option<Vec<u8>>,
    ) -> Result<Response, Error> {
        Err(Error::Transport(
            "the workflow API is only reachable from a wasm32 guest".into(),
        ))
    }

    pub(crate) async fn call(
        method: &str,
        path: &str,
        headers: &[(&str, &str)],
        body: Option<Vec<u8>>,
    ) -> Result<Response, Error> {
        raw(method, path, headers, body).await
    }
}

/// Send one workflow API request. Writes get an `Idempotency-Key`.
pub async fn call(
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Option<Vec<u8>>,
) -> Result<Response, Error> {
    if method == "GET" {
        return transport::call(method, path, headers, body).await;
    }
    let key = next_idempotency_key();
    let mut all = vec![(runtara_workflow_api::headers::IDEMPOTENCY_KEY, key.as_str())];
    all.extend_from_slice(headers);
    transport::call(method, path, &all, body).await
}

/// `GET /v1/run`.
pub async fn run_info() -> Result<RunInfo, Error> {
    let response = call("GET", "/v1/run", &[], None).await?;
    serde_json::from_slice(&response.body)
        .map_err(|error| Error::Transport(format!("run info: {error}")))
}

/// `GET /v1/run/input`: the raw `{data, variables}` input envelope.
pub async fn input() -> Result<Vec<u8>, Error> {
    Ok(call("GET", "/v1/run/input", &[], None).await?.body)
}

/// `POST /v1/run/outcome`. Call it once, then return from `run`.
pub async fn post_outcome(outcome: PostedOutcome) -> Result<(), Error> {
    let (kind, body) = match outcome {
        PostedOutcome::Completed(output) => (runtara_workflow_api::OutcomeKind::Completed, output),
        PostedOutcome::Failed(error) => (
            runtara_workflow_api::OutcomeKind::Failed,
            serde_json::to_vec(&error).map_err(|e| Error::Transport(e.to_string()))?,
        ),
        PostedOutcome::Suspended(suspension) => (
            runtara_workflow_api::OutcomeKind::Suspended,
            serde_json::to_vec(&suspension).map_err(|e| Error::Transport(e.to_string()))?,
        ),
    };
    call(
        "POST",
        "/v1/run/outcome",
        &[(runtara_workflow_api::headers::OUTCOME, kind.as_str())],
        Some(body),
    )
    .await
    .map(|_| ())
}

/// The result of [`checkpoint`].
#[derive(Debug, Clone, PartialEq)]
pub struct CheckpointWritten {
    /// False when the checkpoint already existed; `state` is the one that won.
    pub stored: bool,
    pub state: Vec<u8>,
    /// A lifecycle command waiting for the run; read it with
    /// [`poll_lifecycle_signal`].
    pub pending_signal: Option<runtara_workflow_api::LifecycleNotice>,
    /// A custom signal waiting at this checkpoint; read it with
    /// [`poll_custom_signal`].
    pub custom_signal: Option<runtara_workflow_api::CustomSignalNotice>,
}

/// A lifecycle command delivered by [`poll_lifecycle_signal`].
#[derive(Debug, Clone, PartialEq)]
pub struct LifecycleSignal {
    pub notice: runtara_workflow_api::LifecycleNotice,
    pub payload: Vec<u8>,
}

fn key_path(prefix: &str, key: &str) -> String {
    format!("{prefix}/{}", runtara_workflow_api::encode_path_key(key))
}

fn header_json<T: serde::de::DeserializeOwned>(response: &Response, name: &str) -> Option<T> {
    serde_json::from_str(response.header(name)?).ok()
}

/// `GET /v1/checkpoints/{id}`: the committed state, if any.
pub async fn get_checkpoint(id: &str) -> Result<Option<Vec<u8>>, Error> {
    match call("GET", &key_path("/v1/checkpoints", id), &[], None).await {
        Ok(response) => Ok(Some(response.body)),
        Err(Error::Api(error)) if error.code == ErrorCode::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

/// `PUT /v1/checkpoints/{id}`: commit `state`; the first committed state wins.
pub async fn checkpoint(id: &str, state: Vec<u8>) -> Result<CheckpointWritten, Error> {
    let response = call("PUT", &key_path("/v1/checkpoints", id), &[], Some(state)).await?;
    Ok(CheckpointWritten {
        stored: response.status == 201,
        pending_signal: header_json(&response, runtara_workflow_api::headers::PENDING_SIGNAL),
        custom_signal: header_json(&response, runtara_workflow_api::headers::CUSTOM_SIGNAL),
        state: response.body,
    })
}

/// `POST /v1/checkpoints/{id}/retry-attempts`.
pub async fn record_retry_attempt(
    id: &str,
    attempt_number: u32,
    error_message: Option<&str>,
) -> Result<(), Error> {
    let body = serde_json::json!({"attemptNumber": attempt_number, "errorMessage": error_message});
    call(
        "POST",
        &format!("{}/retry-attempts", key_path("/v1/checkpoints", id)),
        &[("content-type", "application/json")],
        Some(body.to_string().into_bytes()),
    )
    .await
    .map(|_| ())
}

/// `GET /v1/signals/lifecycle`: the pending lifecycle command, unacknowledged.
pub async fn poll_lifecycle_signal() -> Result<Option<LifecycleSignal>, Error> {
    let response = call("GET", "/v1/signals/lifecycle", &[], None).await?;
    if response.status == 204 {
        return Ok(None);
    }
    let notice = header_json(&response, runtara_workflow_api::headers::SIGNAL)
        .ok_or_else(|| Error::Transport("lifecycle signal without its notice".into()))?;
    Ok(Some(LifecycleSignal {
        notice,
        payload: response.body,
    }))
}

/// `POST /v1/signals/lifecycle/{commandId}/ack`: whether it applied.
pub async fn ack_lifecycle_signal(command_id: &str, signal_type: &str) -> Result<bool, Error> {
    let body = serde_json::json!({"signalType": signal_type});
    let response = call(
        "POST",
        &format!("{}/ack", key_path("/v1/signals/lifecycle", command_id)),
        &[("content-type", "application/json")],
        Some(body.to_string().into_bytes()),
    )
    .await?;
    let applied: serde_json::Value = serde_json::from_slice(&response.body)
        .map_err(|e| Error::Transport(format!("acknowledgement: {e}")))?;
    Ok(applied["applied"].as_bool().unwrap_or(false))
}

/// `GET /v1/signals/custom/{checkpointId}`, waiting up to `wait_ms`.
pub async fn poll_custom_signal(
    checkpoint_id: &str,
    wait_ms: u64,
) -> Result<Option<Vec<u8>>, Error> {
    let path = format!(
        "{}?waitMs={wait_ms}",
        key_path("/v1/signals/custom", checkpoint_id)
    );
    let response = call("GET", &path, &[], None).await?;
    Ok((response.status == 200).then_some(response.body))
}

/// `GET /v1/time`: persistence time in ms since the Unix epoch.
pub async fn now_ms() -> Result<u64, Error> {
    let response = call("GET", "/v1/time", &[], None).await?;
    let now: serde_json::Value = serde_json::from_slice(&response.body)
        .map_err(|e| Error::Transport(format!("time: {e}")))?;
    now["nowMs"]
        .as_u64()
        .ok_or_else(|| Error::Transport("time without nowMs".into()))
}

/// `POST /v1/heartbeat`.
pub async fn heartbeat() -> Result<(), Error> {
    call("POST", "/v1/heartbeat", &[], None).await.map(|_| ())
}

/// `POST /v1/events`: append `events` in order, as one NDJSON batch.
pub async fn emit_events(events: &[runtara_workflow_api::Event]) -> Result<(), Error> {
    let mut body = Vec::new();
    for event in events {
        serde_json::to_writer(&mut body, event).map_err(|e| Error::Transport(e.to_string()))?;
        body.push(b'\n');
    }
    call(
        "POST",
        "/v1/events",
        &[("content-type", "application/x-ndjson")],
        Some(body),
    )
    .await
    .map(|_| ())
}

/// `PATCH /v1/state/{key}`: merge `patch` into the run's published state, once
/// per SetState step `key` (its canonical v2 `state` key).
pub async fn set_state(
    key: &str,
    patch: &serde_json::Map<String, serde_json::Value>,
) -> Result<(), Error> {
    let body = serde_json::to_vec(patch).map_err(|e| Error::Transport(e.to_string()))?;
    call(
        "PATCH",
        &key_path("/v1/state", key),
        &[("content-type", "application/json")],
        Some(body),
    )
    .await
    .map(|_| ())
}

/// `GET /v1/state/{key}`: the run's published state, read by GetState step
/// `key` (its canonical v2 `state-read` key).
pub async fn get_state(key: &str) -> Result<serde_json::Map<String, serde_json::Value>, Error> {
    let response = call("GET", &key_path("/v1/state", key), &[], None).await?;
    serde_json::from_slice(&response.body).map_err(|e| Error::Transport(format!("state: {e}")))
}

/// `PUT /v1/inputs/{signalId}`: register a managed input. `descriptor` must
/// carry `signal_id`; tenant and run come from the host, never from it.
pub async fn register_input(
    descriptor: &serde_json::Map<String, serde_json::Value>,
    deadline_ms: Option<u64>,
) -> Result<(), Error> {
    let signal_id = descriptor
        .get("signal_id")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| Error::Transport("an input descriptor needs a signal_id".into()))?;
    let body = serde_json::json!({"descriptor": descriptor, "deadlineMs": deadline_ms});
    call(
        "PUT",
        &key_path("/v1/inputs", signal_id),
        &[("content-type", "application/json")],
        Some(body.to_string().into_bytes()),
    )
    .await
    .map(|_| ())
}

/// `GET /v1/inputs/{signalId}`, waiting up to `wait_ms` while it is open.
pub async fn poll_input(
    signal_id: &str,
    wait_ms: u64,
) -> Result<runtara_workflow_api::InputRead, Error> {
    let path = format!("{}?waitMs={wait_ms}", key_path("/v1/inputs", signal_id));
    let response = call("GET", &path, &[], None).await?;
    serde_json::from_slice(&response.body).map_err(|e| Error::Transport(format!("input: {e}")))
}

/// `DELETE /v1/inputs/{signalId}`: abandon the input; returns its final state.
pub async fn close_input(signal_id: &str) -> Result<runtara_workflow_api::InputRead, Error> {
    let response = call("DELETE", &key_path("/v1/inputs", signal_id), &[], None).await?;
    serde_json::from_slice(&response.body).map_err(|e| Error::Transport(format!("input: {e}")))
}

/// `PUT /v1/waits/{key}`: register the WaitForInstances step `key`'s wait on
/// `instance_ids` (its direct child runs), or find it on replay, and read it.
/// A pending read (`resolution` null) carries the `waitId` to suspend on.
pub async fn register_wait(
    key: &str,
    instance_ids: &[String],
    mode: &str,
    deadline_ms: Option<u64>,
) -> Result<serde_json::Value, Error> {
    let body =
        serde_json::json!({"instanceIds": instance_ids, "mode": mode, "deadlineMs": deadline_ms});
    let response = call(
        "PUT",
        &key_path("/v1/waits", key),
        &[("content-type", "application/json")],
        Some(body.to_string().into_bytes()),
    )
    .await?;
    serde_json::from_slice(&response.body).map_err(|e| Error::Transport(format!("wait: {e}")))
}

/// `GET /v1/waits/{key}`, waiting up to `wait_ms` while it is pending.
pub async fn poll_wait(key: &str, wait_ms: u64) -> Result<serde_json::Value, Error> {
    let path = format!("{}?waitMs={wait_ms}", key_path("/v1/waits", key));
    let response = call("GET", &path, &[], None).await?;
    serde_json::from_slice(&response.body).map_err(|e| Error::Transport(format!("wait: {e}")))
}

/// `DELETE /v1/waits/{key}`: drop the settled wait once its result is kept.
pub async fn release_wait(key: &str) -> Result<(), Error> {
    call("DELETE", &key_path("/v1/waits", key), &[], None)
        .await
        .map(|_| ())
}

/// `PUT /v1/run/deadline`: end the run `abort_after_ms` from now at the
/// latest. It can only shorten the launch deadline.
pub async fn set_deadline(abort_after_ms: u64) -> Result<(), Error> {
    let body = serde_json::json!({"abortAfterMs": abort_after_ms});
    call(
        "PUT",
        "/v1/run/deadline",
        &[("content-type", "application/json")],
        Some(body.to_string().into_bytes()),
    )
    .await
    .map(|_| ())
}

/// `GET /v1/connections/{id}`: the safe descriptor of a connection (JSON).
pub async fn describe_connection(connection_id: &str) -> Result<serde_json::Value, Error> {
    let response = call(
        "GET",
        &key_path("/v1/connections", connection_id),
        &[],
        None,
    )
    .await?;
    serde_json::from_slice(&response.body).map_err(|e| Error::Transport(format!("connection: {e}")))
}

/// `POST /v1/connections/{id}/resources`: resolve a resource request.
pub async fn resolve_resource(
    connection_id: &str,
    request: &serde_json::Value,
) -> Result<serde_json::Value, Error> {
    let response = call(
        "POST",
        &format!("{}/resources", key_path("/v1/connections", connection_id)),
        &[("content-type", "application/json")],
        Some(request.to_string().into_bytes()),
    )
    .await?;
    serde_json::from_slice(&response.body).map_err(|e| Error::Transport(format!("resource: {e}")))
}

/// `POST /v1/connections/{id}/sql/{operation}` with a
/// `runtara-database-contract` request; database failures arrive as
/// `database-error` with the `DatabaseError` in `details`.
pub async fn sql(
    connection_id: &str,
    operation: runtara_workflow_api::SqlOperation,
    request: &serde_json::Value,
) -> Result<serde_json::Value, Error> {
    let operation = match operation {
        runtara_workflow_api::SqlOperation::Query => "query",
        runtara_workflow_api::SqlOperation::Execute => "execute",
        runtara_workflow_api::SqlOperation::Batch => "batch",
    };
    let response = call(
        "POST",
        &format!(
            "{}/sql/{operation}",
            key_path("/v1/connections", connection_id)
        ),
        &[("content-type", "application/json")],
        Some(request.to_string().into_bytes()),
    )
    .await?;
    serde_json::from_slice(&response.body).map_err(|e| Error::Transport(format!("sql: {e}")))
}

/// The result of [`http`]: the upstream answer, or the call's failure with
/// whatever upstream body was received.
#[derive(Debug, Clone)]
pub enum Outbound {
    Reply {
        status: u16,
        headers: Vec<(String, String)>,
        body: Vec<u8>,
    },
    Failed {
        failure: runtara_workflow_api::OutboundFailure,
        body: Vec<u8>,
    },
}

/// `POST /v1/http`: an outbound call through a connection or the outbound
/// guard. Outbound headers travel in the metadata, never as request headers.
pub async fn http(
    meta: &runtara_workflow_api::OutboundMeta,
    body: Option<Vec<u8>>,
) -> Result<Outbound, Error> {
    let meta_json = serde_json::to_string(meta).map_err(|e| Error::Transport(e.to_string()))?;
    let raw = call_raw(
        "POST",
        "/v1/http",
        &[(runtara_workflow_api::headers::OUTBOUND, meta_json.as_str())],
        body,
    )
    .await?;
    if raw.status == 502
        && let Some(failure) = header_json(&raw, runtara_workflow_api::headers::ERROR)
    {
        return Ok(Outbound::Failed {
            failure,
            body: raw.body,
        });
    }
    let raw = raw.into_result()?;
    let status = raw
        .header(runtara_workflow_api::headers::UPSTREAM_STATUS)
        .and_then(|value| value.parse().ok())
        .ok_or_else(|| Error::Transport("reply without an upstream status".into()))?;
    let headers =
        header_json(&raw, runtara_workflow_api::headers::UPSTREAM_HEADERS).unwrap_or_default();
    Ok(Outbound::Reply {
        status,
        headers,
        body: raw.body,
    })
}

/// Like [`call`], but hands back non-2xx answers instead of decoding them.
async fn call_raw(
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Option<Vec<u8>>,
) -> Result<Response, Error> {
    let key = next_idempotency_key();
    let mut all = vec![(runtara_workflow_api::headers::IDEMPOTENCY_KEY, key.as_str())];
    all.extend_from_slice(headers);
    transport::raw(method, path, &all, body).await
}

/// `POST /v1/operations`: enter the operation of the call site
/// `checkpoint_key` at `attempt`. Returns its id, and with `load` the
/// continuation saved for it.
pub async fn enter_operation(
    checkpoint_key: &str,
    attempt: u32,
    load: bool,
) -> Result<(String, Option<Vec<u8>>), Error> {
    let body =
        serde_json::json!({"checkpointKey": checkpoint_key, "attempt": attempt, "load": load});
    let response = call(
        "POST",
        "/v1/operations",
        &[("content-type", "application/json")],
        Some(body.to_string().into_bytes()),
    )
    .await?;
    let id = response
        .header(runtara_workflow_api::headers::OPERATION)
        .ok_or_else(|| Error::Transport("operation without an id".into()))?
        .to_string();
    Ok((id, (response.status == 200).then_some(response.body)))
}

/// `POST /v1/operations/{id}/suspend`: keep `state` as the continuation and
/// leave the operation suspended on `wakes`.
pub async fn suspend_operation(
    operation_id: &str,
    state: Vec<u8>,
    wakes: &[runtara_workflow_api::Wake],
) -> Result<(), Error> {
    let wakes = serde_json::to_string(wakes).map_err(|e| Error::Transport(e.to_string()))?;
    call(
        "POST",
        &format!("{}/suspend", key_path("/v1/operations", operation_id)),
        &[(runtara_workflow_api::headers::WAKES, wakes.as_str())],
        Some(state),
    )
    .await
    .map(|_| ())
}

/// `POST /v1/operations/{id}/exit`: leave the operation; a failure discards
/// what it kept.
pub async fn exit_operation(operation_id: &str, failed: bool) -> Result<(), Error> {
    let body = serde_json::json!({"failed": failed});
    call(
        "POST",
        &format!("{}/exit", key_path("/v1/operations", operation_id)),
        &[("content-type", "application/json")],
        Some(body.to_string().into_bytes()),
    )
    .await
    .map(|_| ())
}

/// `DELETE /v1/operations/{id}`: drop everything kept for the operation.
pub async fn release_operation(operation_id: &str) -> Result<(), Error> {
    call(
        "DELETE",
        &key_path("/v1/operations", operation_id),
        &[],
        None,
    )
    .await
    .map(|_| ())
}

/// `/v1/instances`: run a control call, inside the entered `operation` when
/// it changes another run. The answer is the call's JSON result.
pub async fn control(
    call: &runtara_workflow_api::ControlCall,
    operation: Option<&str>,
) -> Result<serde_json::Value, Error> {
    use runtara_workflow_api::ControlCall;
    let instance = |id: &str| key_path("/v1/instances", id);
    let (method, path, body) = match call {
        ControlCall::Start(body) => ("POST", "/v1/instances".to_string(), Some(body)),
        ControlCall::Get(id) => ("GET", instance(id), None),
        ControlCall::GetState(id) => ("GET", format!("{}/state", instance(id)), None),
        ControlCall::Query(body) => ("POST", "/v1/instances/query".to_string(), Some(body)),
        ControlCall::PendingSignals(body) => (
            "POST",
            "/v1/instances/signals/pending".to_string(),
            Some(body),
        ),
        ControlCall::SendSignal(id, body) => {
            ("POST", format!("{}/signals", instance(id)), Some(body))
        }
        ControlCall::Cancel(id, body) => ("POST", format!("{}/cancel", instance(id)), Some(body)),
        ControlCall::Pause(id) => ("POST", format!("{}/pause", instance(id)), None),
        ControlCall::Resume(id) => ("POST", format!("{}/resume", instance(id)), None),
    };
    let mut headers = vec![("content-type", "application/json")];
    if let Some(operation) = operation {
        headers.push((runtara_workflow_api::headers::OPERATION, operation));
    }
    let response = call_json(method, &path, &headers, body).await?;
    serde_json::from_slice(&response.body).map_err(|e| Error::Transport(format!("control: {e}")))
}

async fn call_json(
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Option<&serde_json::Value>,
) -> Result<Response, Error> {
    let body = body
        .filter(|body| !body.is_null())
        .map(|body| body.to_string().into_bytes());
    call(method, path, headers, body).await
}

/// `POST /v1/trusted/{agentId}/{capabilityId}`: run an approved built-in
/// capability with the tenant's `connection_id`. Failures arrive as
/// `trusted-error` with the agent error envelope in `details`.
pub async fn trusted(
    agent_id: &str,
    capability_id: &str,
    connection_id: &str,
    input: &serde_json::Value,
    operation: Option<&str>,
) -> Result<serde_json::Value, Error> {
    let path = format!(
        "{}/{}",
        key_path("/v1/trusted", agent_id),
        runtara_workflow_api::encode_path_key(capability_id)
    );
    let mut headers = vec![("content-type", "application/json")];
    if let Some(operation) = operation {
        headers.push((runtara_workflow_api::headers::OPERATION, operation));
    }
    let body = serde_json::json!({"connectionId": connection_id, "input": input});
    let response = call_json("POST", &path, &headers, Some(&body)).await?;
    serde_json::from_slice(&response.body).map_err(|e| Error::Transport(format!("trusted: {e}")))
}
