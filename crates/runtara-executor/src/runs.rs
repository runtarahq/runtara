// Copyright (C) 2026 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Runs: each is a `runtara-run` process and the task here that supervises
//! it.
//!
//! The process holds the guest and nothing else. Its workflow API calls come
//! out of it over a socket pair, and this task relays each to the server's
//! run channel, where it adds what the process never sees: the launch's
//! credential. When the process ends without the guest having posted an
//! outcome (it trapped, was killed, ran out of time), this task posts the
//! failed one, so the server always learns how a run ended from the run.

use std::collections::HashMap;
use std::convert::Infallible;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bytes::Bytes;
use http_body_util::{BodyExt, Full, Limited};
use hyper::body::Incoming;
use hyper_util::rt::{TokioExecutor, TokioIo};
use runtara_workflow_api::{Caps, headers};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use zeroize::Zeroizing;

use crate::artifacts::{Artifacts, Runner};
use crate::protocol::{Place, RUN_PROTOCOL, RunIdentity, RunSpec, RunState};
use crate::tls::{Client, Identity};

/// How long past its lifetime a run's process is left to end by itself.
const LIFETIME_GRACE: Duration = Duration::from_secs(5);
/// How long an ended run's state is kept for the server to read.
const KEPT_FOR: Duration = Duration::from_secs(600);
/// How much of a process's report and of its output is read.
const MAX_REPORT_BYTES: u64 = 64 * 1024;
const MAX_OUTPUT_BYTES: usize = 64 * 1024;
/// Workflow API calls one run has in flight.
const CONCURRENT_CALLS: u32 = 16;
/// How long a run waits for a slot before it is refused for now.
const SLOT_WAIT: Duration = Duration::from_secs(5);
/// How often the failed outcome is offered to a server that does not answer.
const OUTCOME_ATTEMPTS: u32 = 4;

/// Why a run was not placed.
#[derive(Debug)]
pub enum PlaceError {
    /// The spec is wrong.
    Invalid(String),
    /// The workflow is not staged for the tenant.
    NotStaged,
    /// Every slot stayed taken; the server offers the run again.
    Capacity,
    Internal(String),
}

struct Run {
    state: tokio::sync::watch::Sender<RunState>,
    stop: tokio::sync::Notify,
}

/// The run channel of one run: the server's endpoint, with the launch's
/// credential.
struct Upstream {
    client: Client,
    /// `https://host:port`, and any path prefix, without a trailing slash.
    endpoint: String,
    authorization: http::HeaderValue,
    trace_id: Option<http::HeaderValue>,
    caps: Caps,
}

impl Upstream {
    fn new(identity: &Identity, spec: &RunSpec) -> Result<Self, String> {
        let endpoint = spec.channel.endpoint.trim_end_matches('/').to_owned();
        let uri: http::Uri = endpoint
            .parse()
            .map_err(|_| "the run channel's endpoint is not a URL".to_owned())?;
        let host = uri.host().unwrap_or_default().to_owned();
        if uri.scheme_str() != Some("https") || host.is_empty() || uri.query().is_some() {
            return Err("the run channel's endpoint must be https://host[:port]".to_owned());
        }
        let trust = &spec.channel.trust;
        let config = identity.client_config(&trust.ca_pem, trust.pin.as_deref())?;
        let bearer = Zeroizing::new(format!("Bearer {}", spec.channel.credential));
        let mut authorization = http::HeaderValue::from_str(&bearer)
            .map_err(|_| "the credential is not a header value".to_owned())?;
        authorization.set_sensitive(true);
        let trace_id = spec
            .run
            .trace_id
            .as_deref()
            .and_then(|trace| http::HeaderValue::from_str(trace).ok());
        Ok(Self {
            client: crate::tls::client(config, trust.server_name.as_deref().unwrap_or(&host))?,
            endpoint,
            authorization,
            trace_id,
            caps: Caps::default(),
        })
    }

    /// Sends one call to the run channel as this run.
    async fn call(
        &self,
        method: http::Method,
        path_and_query: &str,
        guest_headers: &http::HeaderMap,
        body: Bytes,
    ) -> Result<http::Response<Bytes>, http::StatusCode> {
        let mut request = http::Request::builder()
            .method(method)
            .uri(format!("{}{path_and_query}", self.endpoint))
            .body(Full::new(body))
            .map_err(|_| http::StatusCode::BAD_REQUEST)?;
        for (name, value) in guest_headers {
            if !headers::is_stripped(name.as_str())
                && name != http::header::HOST
                && name != http::header::CONTENT_LENGTH
            {
                request.headers_mut().append(name.clone(), value.clone());
            }
        }
        request
            .headers_mut()
            .insert(http::header::AUTHORIZATION, self.authorization.clone());
        if let Some(trace_id) = &self.trace_id {
            request
                .headers_mut()
                .insert("runtara-trace-id", trace_id.clone());
        }
        let response = self
            .client
            .request(request)
            .await
            .map_err(|_| http::StatusCode::BAD_GATEWAY)?;
        let (mut parts, body) = response.into_parts();
        let body = Limited::new(body, self.caps.response_body_bytes)
            .collect()
            .await
            .map_err(|_| http::StatusCode::BAD_GATEWAY)?
            .to_bytes();
        parts.headers.remove(http::header::CONTENT_LENGTH);
        parts.headers.remove(http::header::TRANSFER_ENCODING);
        parts.headers.remove(http::header::CONNECTION);
        Ok(http::Response::from_parts(parts, body))
    }

    /// Posts the failed outcome of a run whose guest posted none. A server
    /// that refuses it (the run already ended there, or was fenced) is not
    /// asked again.
    async fn post_failed(&self, run: &RunIdentity, failure: &Failure) {
        let body = Bytes::from(
            json!({
                "code": failure.code,
                "message": failure.message,
                "category": failure.category,
                "severity": "error",
                "retryable": false,
                "attributes": {"source": "runtara-executor"},
            })
            .to_string(),
        );
        let mut request_headers = http::HeaderMap::new();
        request_headers.insert(headers::OUTCOME, http::HeaderValue::from_static("failed"));
        request_headers.insert(
            http::header::CONTENT_TYPE,
            http::HeaderValue::from_static("application/json"),
        );
        if let Ok(key) = http::HeaderValue::from_str(&format!(
            "runtara-executor-outcome-{}-{}",
            run.execution, run.generation
        )) {
            request_headers.insert(headers::IDEMPOTENCY_KEY, key);
        }
        for attempt in 0..OUTCOME_ATTEMPTS {
            match self
                .call(
                    http::Method::POST,
                    "/v1/run/outcome",
                    &request_headers,
                    body.clone(),
                )
                .await
            {
                Ok(response)
                    if response.status().is_success() || response.status().is_client_error() =>
                {
                    return;
                }
                _ => tokio::time::sleep(Duration::from_secs(1 << attempt)).await,
            }
        }
        tracing::warn!(
            execution = %run.execution,
            "the server did not take the run's failed outcome"
        );
    }
}

/// What the executor posts for a run that posted nothing.
struct Failure {
    code: &'static str,
    message: String,
    category: &'static str,
}

impl Failure {
    fn permanent(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            category: "permanent",
        }
    }
}

fn answer(status: http::StatusCode) -> http::Response<Full<Bytes>> {
    let mut response = http::Response::new(Full::default());
    *response.status_mut() = status;
    response
}

/// One call of the guest's, relayed.
async fn relay(
    request: http::Request<Incoming>,
    upstream: Arc<Upstream>,
    posted: Arc<AtomicBool>,
) -> Result<http::Response<Full<Bytes>>, Infallible> {
    let (parts, body) = request.into_parts();
    let path_and_query = parts
        .uri
        .path_and_query()
        .map(|path| path.as_str().to_owned())
        .unwrap_or_default();
    // Only the workflow API is behind the run channel.
    if !path_and_query.starts_with("/v1/") {
        return Ok(answer(http::StatusCode::NOT_FOUND));
    }
    let Ok(body) = Limited::new(body, upstream.caps.request_body_bytes)
        .collect()
        .await
    else {
        return Ok(answer(http::StatusCode::PAYLOAD_TOO_LARGE));
    };
    let outcome = parts.method == http::Method::POST && parts.uri.path() == "/v1/run/outcome";
    match upstream
        .call(
            parts.method,
            &path_and_query,
            &parts.headers,
            body.to_bytes(),
        )
        .await
    {
        Ok(response) => {
            if outcome && response.status().is_success() {
                posted.store(true, Ordering::Release);
            }
            Ok(response.map(Full::new))
        }
        Err(status) => Ok(answer(status)),
    }
}

async fn read_capped(reader: Option<impl tokio::io::AsyncRead + Unpin>, limit: u64) -> Vec<u8> {
    let mut bytes = Vec::new();
    if let Some(reader) = reader {
        let mut reader = reader.take(limit);
        let _ = reader.read_to_end(&mut bytes).await;
        // Past the limit, read on and keep nothing: a process blocked on a
        // full pipe would never end.
        let mut reader = reader.into_inner();
        let mut discard = [0u8; 8192];
        while matches!(reader.read(&mut discard).await, Ok(read) if read > 0) {}
    }
    bytes
}

/// Every run this executor holds.
pub struct Runs {
    runs: Mutex<HashMap<String, Arc<Run>>>,
    slots: Arc<tokio::sync::Semaphore>,
    artifacts: Arc<Artifacts>,
    identity: Arc<Identity>,
    runner: Runner,
}

impl Runs {
    pub fn new(
        artifacts: Arc<Artifacts>,
        identity: Arc<Identity>,
        runner: Runner,
        max_runs: usize,
    ) -> Arc<Self> {
        Arc::new(Self {
            runs: Mutex::default(),
            slots: Arc::new(tokio::sync::Semaphore::new(max_runs)),
            artifacts,
            identity,
            runner,
        })
    }

    fn get(&self, run: &str) -> Option<Arc<Run>> {
        self.runs
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(run)
            .cloned()
    }

    /// Runs in flight.
    pub fn running(&self) -> usize {
        self.runs
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .values()
            .filter(|run| *run.state.borrow() == RunState::Running)
            .count()
    }

    /// Starts a run; its id.
    pub async fn place(self: &Arc<Self>, place: Place) -> Result<String, PlaceError> {
        let spec = place.spec;
        if spec.protocol != RUN_PROTOCOL {
            return Err(PlaceError::Invalid(format!(
                "run protocol {} is not {RUN_PROTOCOL}",
                spec.protocol
            )));
        }
        if !(1..=86_400).contains(&spec.lifetime_seconds) {
            return Err(PlaceError::Invalid(
                "lifetime_seconds is out of range".to_owned(),
            ));
        }
        let upstream = Arc::new(Upstream::new(&self.identity, &spec).map_err(PlaceError::Invalid)?);
        let compiled = self
            .artifacts
            .open(&place.tenant, &place.artifact)
            .ok_or(PlaceError::NotStaged)?;
        // A burst a little over capacity waits its turn here: runs are
        // short, and a refusal costs the server a recovery. Only an executor
        // that stays full refuses.
        let slot = tokio::time::timeout(SLOT_WAIT, self.slots.clone().acquire_owned())
            .await
            .ok()
            .and_then(Result::ok)
            .ok_or_else(|| {
                tracing::warn!(
                    execution = %spec.run.execution,
                    "every run slot stayed taken; refusing a run for now"
                );
                PlaceError::Capacity
            })?;

        let internal = |error: std::io::Error| PlaceError::Internal(error.to_string());
        let (ours, theirs) = std::os::unix::net::UnixStream::pair().map_err(internal)?;
        ours.set_nonblocking(true).map_err(internal)?;
        let ours = tokio::net::UnixStream::from_std(ours).map_err(internal)?;
        let mut child = self
            .runner
            .command("run", compiled.into(), theirs.into())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(internal)?;
        // What the process is told: the guest's limits, and nothing of where
        // its calls go or as whom.
        let told = json!({
            "wasm_memory_bytes": spec.wasm_memory_bytes,
            "lifetime_seconds": spec.lifetime_seconds,
            "env": spec.env,
        });
        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(told.to_string().as_bytes())
                .await
                .map_err(internal)?;
        }

        let id = uuid::Uuid::new_v4().simple().to_string();
        let run = Arc::new(Run {
            state: tokio::sync::watch::Sender::new(RunState::Running),
            stop: tokio::sync::Notify::new(),
        });
        self.runs
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(id.clone(), run.clone());
        tracing::info!(
            run = %id,
            execution = %spec.run.execution,
            tenant = %place.tenant,
            "run placed"
        );
        let runs = Arc::clone(self);
        let supervised = id.clone();
        tokio::spawn(async move {
            let lifetime = Duration::from_secs(spec.lifetime_seconds);
            let state = supervise(
                &supervised,
                &run,
                child,
                ours,
                upstream,
                &spec.run,
                lifetime,
            )
            .await;
            tracing::info!(run = %supervised, execution = %spec.run.execution, ?state, "run ended");
            run.state.send_replace(state);
            drop(slot);
            tokio::time::sleep(KEPT_FOR).await;
            runs.runs
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .remove(&supervised);
        });
        Ok(id)
    }

    /// Stops a run; whether there is one.
    pub fn stop(&self, run: &str) -> bool {
        self.get(run).map(|run| run.stop.notify_one()).is_some()
    }

    /// A run's state, after waiting up to `wait` for it to end.
    pub async fn state(&self, run: &str, wait: Duration) -> Option<RunState> {
        let run = self.get(run)?;
        let mut state = run.state.subscribe();
        let _ =
            tokio::time::timeout(wait, state.wait_for(|state| *state != RunState::Running)).await;
        let state = state.borrow().clone();
        Some(state)
    }
}

/// Sees one run's process to its end; what became of it.
async fn supervise(
    id: &str,
    run: &Run,
    mut child: tokio::process::Child,
    socket: tokio::net::UnixStream,
    upstream: Arc<Upstream>,
    identity: &RunIdentity,
    lifetime: Duration,
) -> RunState {
    let posted = Arc::new(AtomicBool::new(false));
    let serving = {
        let (upstream, posted) = (upstream.clone(), posted.clone());
        tokio::spawn(async move {
            let service = hyper::service::service_fn(move |request| {
                relay(request, upstream.clone(), posted.clone())
            });
            let _ = hyper::server::conn::http2::Builder::new(TokioExecutor::new())
                .max_concurrent_streams(CONCURRENT_CALLS)
                .serve_connection(TokioIo::new(socket), service)
                .await;
        })
    };
    let report = tokio::spawn(read_capped(child.stdout.take(), MAX_REPORT_BYTES));
    let output = tokio::spawn(read_capped(child.stderr.take(), MAX_OUTPUT_BYTES as u64));

    enum End {
        Exited(std::io::Result<std::process::ExitStatus>),
        Stopped,
        Lifetime,
    }
    let end = tokio::select! {
        status = child.wait() => End::Exited(status),
        _ = run.stop.notified() => End::Stopped,
        _ = tokio::time::sleep(lifetime + LIFETIME_GRACE) => End::Lifetime,
    };
    if !matches!(end, End::Exited(_)) {
        let _ = child.start_kill();
        let _ = child.wait().await;
    }
    serving.abort();
    let report: Value = String::from_utf8_lossy(&report.await.unwrap_or_default())
        .lines()
        .last()
        .and_then(|line| serde_json::from_str(line).ok())
        .unwrap_or(Value::Null);
    let output = output.await.unwrap_or_default();
    if !output.is_empty() {
        tracing::debug!(run = %id, output = %String::from_utf8_lossy(&output), "run output");
    }
    let posted = posted.load(Ordering::Acquire);

    let (state, failure) = match end {
        // The server asked: it does not wait for an outcome.
        End::Stopped => (
            RunState::Failed {
                reason: "stopped".to_owned(),
            },
            None,
        ),
        End::Lifetime => (
            RunState::Failed {
                reason: "ran past its lifetime".to_owned(),
            },
            Some(Failure {
                code: "LIFETIME_EXCEEDED",
                message: "the run ran past its lifetime".to_owned(),
                category: "timeout",
            }),
        ),
        End::Exited(Err(error)) => (
            RunState::Failed {
                reason: format!("waiting for the process failed: {error}"),
            },
            Some(Failure::permanent(
                "GUEST_KILLED",
                "the run's process was lost",
            )),
        ),
        End::Exited(Ok(status)) => match status.code() {
            Some(code) => {
                let reason = report["reason"].as_str().unwrap_or_default();
                let failure = match (code, report["ended"].as_str()) {
                    (_, Some("timeout")) => Failure {
                        code: "LIFETIME_EXCEEDED",
                        message: "the run ran past its lifetime".to_owned(),
                        category: "timeout",
                    },
                    (_, Some("trapped")) => {
                        Failure::permanent("GUEST_FAILED", format!("the run trapped: {reason}"))
                    }
                    (_, Some("returned" | "failed")) => {
                        Failure::permanent("NO_OUTCOME", "the run ended without posting an outcome")
                    }
                    (2, _) => Failure::permanent("LAUNCH_FAILED", "the run could not be started"),
                    _ => Failure::permanent(
                        "GUEST_FAILED",
                        format!("the run's process exited with {code}"),
                    ),
                };
                if code == 2 {
                    tracing::error!(
                        run = %id,
                        output = %String::from_utf8_lossy(&output),
                        "the runner refused to start a run"
                    );
                }
                (
                    RunState::Exited {
                        code: i64::from(code),
                    },
                    Some(failure),
                )
            }
            None => {
                use std::os::unix::process::ExitStatusExt;
                let signal = status.signal().unwrap_or_default();
                (
                    RunState::Failed {
                        reason: format!("killed by signal {signal}"),
                    },
                    Some(Failure::permanent(
                        "GUEST_KILLED",
                        format!("the run's process was killed by signal {signal}"),
                    )),
                )
            }
        },
    };
    if let (false, Some(failure)) = (posted, failure) {
        upstream.post_failed(identity, &failure).await;
    }
    state
}
