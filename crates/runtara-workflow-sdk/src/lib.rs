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

    pub(crate) async fn call(
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
        Response {
            status,
            headers,
            body,
        }
        .into_result()
    }
}

#[cfg(not(target_arch = "wasm32"))]
mod transport {
    use super::{Error, Response};

    pub(crate) async fn call(
        _method: &str,
        _path: &str,
        _headers: &[(&str, &str)],
        _body: Option<Vec<u8>>,
    ) -> Result<Response, Error> {
        Err(Error::Transport(
            "the workflow API is only reachable from a wasm32 guest".into(),
        ))
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
