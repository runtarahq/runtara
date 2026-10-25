//! The host-side `/v1` router.
//!
//! [`router`] serves the workflow API for every runtime that can attach a
//! [`Binding`]: the embedded runner's `send_request` hook calls it in process,
//! and runtara-cloud's binding gateway mounts it behind node traffic. Neither
//! path lets the guest set the binding; it arrives as a request extension.
//!
//! What the router owns, for every endpoint: the binding check, version
//! negotiation, body caps, `Idempotency-Key` receipts, the `fenced` and
//! `finished` checks, and the error envelope. What it delegates: the effect
//! itself, through [`WorkflowApiBackend`]. Endpoint families land one at a
//! time; anything not served yet answers `unsupported`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use axum::Router;
use axum::body::{Body, to_bytes};
use axum::extract::{Request, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use bytes::Bytes;
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::{
    ApiError, Binding, Caps, ErrorCode, ErrorInfo, OutcomeKind, RunInfo, Suspension, headers,
    parse_api_version, serves_version, validate_idempotency_key,
};

/// Where a launch stands, as far as the API is concerned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LaunchState {
    /// The binding's launch holds the run's current lease and has not posted
    /// its outcome.
    Active,
    /// The launch posted its outcome; only `GET /v1/run` is still served.
    Finished,
    /// The binding's lease epoch is older than the run's current lease.
    Fenced,
}

pub use crate::PostedOutcome;

mod runtime;
pub use runtime::{CheckpointWrite, CustomSignalNotice, Event, LifecycleNotice, LifecycleSignal};

/// The effects behind the router. Implemented by the runtime that owns runs
/// (runtara-environment for the embedded runner). Every method receives the
/// request's binding and must scope everything it touches to it.
#[async_trait]
pub trait WorkflowApiBackend: Send + Sync + 'static {
    async fn launch_state(&self, binding: &Binding) -> Result<LaunchState, ApiError>;
    async fn run_info(&self, binding: &Binding) -> Result<RunInfo, ApiError>;
    async fn run_input(&self, binding: &Binding) -> Result<Vec<u8>, ApiError>;
    /// Accept the launch's single outcome. After this succeeds,
    /// [`Self::launch_state`] must report [`LaunchState::Finished`].
    async fn post_outcome(&self, binding: &Binding, outcome: PostedOutcome)
    -> Result<(), ApiError>;
    /// Shorten the launch deadline to `abort_after_ms` from now. Never extends.
    async fn set_deadline(&self, binding: &Binding, abort_after_ms: u64) -> Result<(), ApiError>;

    // Run services. Each answers `unsupported` until a backend serves it.

    /// The checkpoint `id`, if one was committed.
    async fn get_checkpoint(
        &self,
        binding: &Binding,
        id: &str,
    ) -> Result<Option<Vec<u8>>, ApiError> {
        let _ = (binding, id);
        Err(unsupported_service("checkpoints"))
    }
    /// Commit `state` as checkpoint `id`; the first committed state wins.
    async fn put_checkpoint(
        &self,
        binding: &Binding,
        id: &str,
        state: Vec<u8>,
    ) -> Result<CheckpointWrite, ApiError> {
        let _ = (binding, id, state);
        Err(unsupported_service("checkpoints"))
    }
    async fn record_retry_attempt(
        &self,
        binding: &Binding,
        id: &str,
        attempt: u32,
        error: Option<String>,
    ) -> Result<(), ApiError> {
        let _ = (binding, id, attempt, error);
        Err(unsupported_service("retry attempts"))
    }
    /// The pending lifecycle command, without acknowledging it.
    async fn poll_lifecycle_signal(
        &self,
        binding: &Binding,
    ) -> Result<Option<LifecycleSignal>, ApiError> {
        let _ = binding;
        Err(unsupported_service("lifecycle signals"))
    }
    /// Acknowledge a lifecycle command the run observed.
    async fn ack_lifecycle_signal(
        &self,
        binding: &Binding,
        command_id: &str,
        signal_type: &str,
    ) -> Result<bool, ApiError> {
        let _ = (binding, command_id, signal_type);
        Err(unsupported_service("lifecycle signals"))
    }
    /// The custom signal payload waiting at `checkpoint_id`, if any.
    async fn poll_custom_signal(
        &self,
        binding: &Binding,
        checkpoint_id: &str,
    ) -> Result<Option<Vec<u8>>, ApiError> {
        let _ = (binding, checkpoint_id);
        Err(unsupported_service("custom signals"))
    }
    /// Persistence time, so deadlines agree with the host.
    async fn now_ms(&self, binding: &Binding) -> Result<u64, ApiError> {
        let _ = binding;
        Err(unsupported_service("time"))
    }
    async fn heartbeat(&self, binding: &Binding) -> Result<(), ApiError> {
        let _ = binding;
        Err(unsupported_service("heartbeat"))
    }
    /// Append events, in order.
    async fn append_events(&self, binding: &Binding, events: Vec<Event>) -> Result<(), ApiError> {
        let _ = (binding, events);
        Err(unsupported_service("events"))
    }
}

fn unsupported_service(service: &str) -> ApiError {
    ApiError::new(
        ErrorCode::Unsupported,
        format!("{service} are not served by this host"),
    )
}

/// A stored response for one `(launch, Idempotency-Key)`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Receipt {
    pub request_hash: [u8; 32],
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

/// Idempotency receipts, kept for the life of a launch.
#[async_trait]
pub trait ReceiptStore: Send + Sync + 'static {
    async fn get(&self, launch_id: &str, key: &str) -> Result<Option<Receipt>, ApiError>;
    async fn put(&self, launch_id: &str, key: &str, receipt: Receipt) -> Result<(), ApiError>;
}

/// In-memory receipts for one process. The embedded runner forgets a launch's
/// receipts when the launch ends; a relaunch is a new launch with new keys.
#[derive(Default)]
pub struct MemoryReceiptStore {
    receipts: Mutex<HashMap<(String, String), Receipt>>,
}

impl MemoryReceiptStore {
    pub fn forget_launch(&self, launch_id: &str) {
        self.receipts
            .lock()
            .expect("receipt store poisoned")
            .retain(|(launch, _), _| launch != launch_id);
    }
}

#[async_trait]
impl ReceiptStore for MemoryReceiptStore {
    async fn get(&self, launch_id: &str, key: &str) -> Result<Option<Receipt>, ApiError> {
        Ok(self
            .receipts
            .lock()
            .expect("receipt store poisoned")
            .get(&(launch_id.to_string(), key.to_string()))
            .cloned())
    }

    async fn put(&self, launch_id: &str, key: &str, receipt: Receipt) -> Result<(), ApiError> {
        self.receipts
            .lock()
            .expect("receipt store poisoned")
            .insert((launch_id.to_string(), key.to_string()), receipt);
        Ok(())
    }
}

/// Everything the router needs.
#[derive(Clone)]
pub struct WorkflowApiState {
    pub backend: Arc<dyn WorkflowApiBackend>,
    pub receipts: Arc<dyn ReceiptStore>,
    pub caps: Caps,
}

/// The `/v1` router. The caller attaches the [`Binding`] as a request
/// extension before handing it over.
pub fn router(state: WorkflowApiState) -> Router {
    let api = Router::new()
        .route("/v1/run", get(get_run))
        .route("/v1/run/input", get(get_run_input))
        .route("/v1/run/outcome", post(post_run_outcome))
        .route("/v1/run/deadline", put(put_run_deadline))
        .route(
            "/v1/checkpoints/{id}",
            get(runtime::get_checkpoint).put(runtime::put_checkpoint),
        )
        .route(
            "/v1/checkpoints/{id}/retry-attempts",
            post(runtime::post_retry_attempt),
        )
        .route("/v1/signals/lifecycle", get(runtime::get_lifecycle_signal))
        .route(
            "/v1/signals/lifecycle/{command_id}/ack",
            post(runtime::ack_lifecycle_signal),
        )
        .route(
            "/v1/signals/custom/{checkpoint_id}",
            get(runtime::get_custom_signal),
        )
        .route("/v1/time", get(runtime::get_time))
        .route("/v1/heartbeat", post(runtime::post_heartbeat))
        .route("/v1/events", post(runtime::post_events))
        .fallback(unsupported)
        .layer(middleware::from_fn_with_state(state.clone(), guard_launch))
        .layer(middleware::from_fn_with_state(state.clone(), idempotency))
        .layer(middleware::from_fn(check_version))
        .layer(middleware::from_fn(require_binding));
    api.with_state(state)
}

/// Render an [`ApiError`] as the uniform envelope.
pub fn error_response(error: &ApiError) -> Response {
    let status = StatusCode::from_u16(error.http_status()).unwrap_or(StatusCode::BAD_REQUEST);
    let body = serde_json::to_vec(error).unwrap_or_default();
    let mut response = (status, body).into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    response
}

struct Failure(ApiError);

impl From<ApiError> for Failure {
    fn from(error: ApiError) -> Self {
        Self(error)
    }
}

impl IntoResponse for Failure {
    fn into_response(self) -> Response {
        error_response(&self.0)
    }
}

fn binding_of(request: &Request) -> Result<Binding, ApiError> {
    request
        .extensions()
        .get::<Binding>()
        .cloned()
        .ok_or_else(|| ApiError::new(ErrorCode::Denied, "request carries no run binding"))
}

async fn require_binding(request: Request, next: Next) -> Response {
    match binding_of(&request) {
        Ok(_) => next.run(request).await,
        Err(error) => error_response(&error),
    }
}

async fn check_version(request: Request, next: Next) -> Response {
    if let Some(value) = request.headers().get(headers::API_VERSION) {
        let requested = value.to_str().ok().and_then(parse_api_version);
        match requested {
            Some(version) if serves_version(version) => {}
            _ => {
                return error_response(&ApiError::new(
                    ErrorCode::Unsupported,
                    format!("this host serves workflow API {}", crate::api_version()),
                ));
            }
        }
    }
    next.run(request).await
}

fn request_hash(method: &Method, uri: &str, headers: &HeaderMap, body: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(method.as_str().as_bytes());
    hasher.update([0]);
    hasher.update(uri.as_bytes());
    hasher.update([0]);
    // Metadata headers change what a request means, so they are part of it.
    for name in [
        headers::OUTCOME,
        headers::OPERATION,
        headers::WAKES,
        headers::OUTBOUND,
        headers::TASK,
    ] {
        if let Some(value) = headers.get(name) {
            hasher.update(name.as_bytes());
            hasher.update(b":");
            hasher.update(value.as_bytes());
        }
        hasher.update([0]);
    }
    hasher.update(body);
    hasher.finalize().into()
}

fn receipt_response(receipt: &Receipt) -> Response {
    let status = StatusCode::from_u16(receipt.status).unwrap_or(StatusCode::OK);
    let mut response = (status, receipt.body.clone()).into_response();
    for (name, value) in &receipt.headers {
        if let (Ok(name), Ok(value)) = (
            header::HeaderName::try_from(name.as_str()),
            HeaderValue::from_str(value),
        ) {
            response.headers_mut().insert(name, value);
        }
    }
    response
}

/// Retryable failures are not remembered, so a retry runs the request again.
fn keep_receipt(status: StatusCode) -> bool {
    !matches!(
        status,
        StatusCode::TOO_MANY_REQUESTS
            | StatusCode::SERVICE_UNAVAILABLE
            | StatusCode::GATEWAY_TIMEOUT
    )
}

async fn idempotency(
    State(state): State<WorkflowApiState>,
    request: Request,
    next: Next,
) -> Response {
    if request.method() == Method::GET {
        return next.run(request).await;
    }
    let binding = match binding_of(&request) {
        Ok(binding) => binding,
        Err(error) => return error_response(&error),
    };
    let key = match request.headers().get(headers::IDEMPOTENCY_KEY) {
        Some(value) => match value.to_str() {
            Ok(key) if validate_idempotency_key(key).is_ok() => key.to_string(),
            _ => {
                return error_response(&ApiError::new(
                    ErrorCode::Invalid,
                    "Idempotency-Key must be 1-128 printable ASCII bytes",
                ));
            }
        },
        None => {
            return error_response(&ApiError::new(
                ErrorCode::Invalid,
                "every non-GET request needs an Idempotency-Key",
            ));
        }
    };

    let (parts, body) = request.into_parts();
    let body = match to_bytes(body, state.caps.request_body_bytes).await {
        Ok(body) => body,
        Err(_) => {
            return error_response(&ApiError::new(
                ErrorCode::TooLarge,
                format!(
                    "request body is over {} bytes",
                    state.caps.request_body_bytes
                ),
            ));
        }
    };
    let uri = parts.uri.to_string();
    let hash = request_hash(&parts.method, &uri, &parts.headers, &body);

    match state.receipts.get(&binding.launch_id, &key).await {
        Ok(Some(receipt)) if receipt.request_hash == hash => return receipt_response(&receipt),
        Ok(Some(_)) => {
            return error_response(&ApiError::new(
                ErrorCode::ReplayConflict,
                "Idempotency-Key was already used for a different request",
            ));
        }
        Ok(None) => {}
        Err(error) => return error_response(&error),
    }

    let response = next.run(Request::from_parts(parts, Body::from(body))).await;
    let (parts, body) = response.into_parts();
    let body = match to_bytes(body, state.caps.response_body_bytes).await {
        Ok(body) => body,
        Err(_) => {
            return error_response(&ApiError::new(
                ErrorCode::TooLarge,
                "response body is over the cap",
            ));
        }
    };
    if keep_receipt(parts.status) {
        let receipt = Receipt {
            request_hash: hash,
            status: parts.status.as_u16(),
            headers: parts
                .headers
                .iter()
                .filter_map(|(name, value)| {
                    Some((name.as_str().to_string(), value.to_str().ok()?.to_string()))
                })
                .collect(),
            body: body.to_vec(),
        };
        if let Err(error) = state.receipts.put(&binding.launch_id, &key, receipt).await {
            return error_response(&error);
        }
    }
    Response::from_parts(parts, Body::from(body))
}

async fn guard_launch(
    State(state): State<WorkflowApiState>,
    request: Request,
    next: Next,
) -> Response {
    let binding = match binding_of(&request) {
        Ok(binding) => binding,
        Err(error) => return error_response(&error),
    };
    let is_run_read = request.method() == Method::GET && request.uri().path() == "/v1/run";
    match state.backend.launch_state(&binding).await {
        Ok(LaunchState::Active) => next.run(request).await,
        Ok(LaunchState::Finished) if is_run_read => next.run(request).await,
        Ok(LaunchState::Finished) => error_response(&ApiError::new(
            ErrorCode::Finished,
            "the run already posted its outcome",
        )),
        Ok(LaunchState::Fenced) => error_response(&ApiError::new(
            ErrorCode::Fenced,
            "this launch no longer holds the run's lease",
        )),
        Err(error) => error_response(&error),
    }
}

async fn unsupported() -> Response {
    error_response(&ApiError::new(
        ErrorCode::Unsupported,
        "endpoint not served by this host",
    ))
}

async fn get_run(
    State(state): State<WorkflowApiState>,
    request: Request,
) -> Result<Response, Failure> {
    let binding = binding_of(&request)?;
    let info = state.backend.run_info(&binding).await?;
    let mut response = serde_json::to_vec(&info)
        .map_err(|e| ApiError::new(ErrorCode::Unavailable, e.to_string()))?
        .into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    Ok(response)
}

async fn get_run_input(
    State(state): State<WorkflowApiState>,
    request: Request,
) -> Result<Response, Failure> {
    let binding = binding_of(&request)?;
    let input = state.backend.run_input(&binding).await?;
    let mut response = Bytes::from(input).into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/octet-stream"),
    );
    Ok(response)
}

fn parse_outcome(
    caps: &Caps,
    kind: Option<&HeaderValue>,
    body: Bytes,
) -> Result<PostedOutcome, ApiError> {
    let kind = kind
        .and_then(|value| value.to_str().ok())
        .and_then(OutcomeKind::parse)
        .ok_or_else(|| {
            ApiError::new(
                ErrorCode::Invalid,
                "Runtara-Outcome must be completed, failed or suspended",
            )
        })?;
    match kind {
        OutcomeKind::Completed => Ok(PostedOutcome::Completed(body.to_vec())),
        OutcomeKind::Failed => serde_json::from_slice::<ErrorInfo>(&body)
            .map(PostedOutcome::Failed)
            .map_err(|e| ApiError::new(ErrorCode::Invalid, format!("failed outcome: {e}"))),
        OutcomeKind::Suspended => {
            let suspension: Suspension = serde_json::from_slice(&body).map_err(|e| {
                ApiError::new(ErrorCode::Invalid, format!("suspended outcome: {e}"))
            })?;
            if suspension.wakes.is_empty() {
                return Err(ApiError::new(
                    ErrorCode::Invalid,
                    "a suspension needs at least one wake",
                ));
            }
            if suspension.wakes.len() > caps.wakes_per_suspension {
                return Err(ApiError::new(
                    ErrorCode::TooLarge,
                    format!(
                        "a suspension carries at most {} wakes",
                        caps.wakes_per_suspension
                    ),
                ));
            }
            for wake in &suspension.wakes {
                check_wake(caps, wake)?;
            }
            Ok(PostedOutcome::Suspended(suspension))
        }
    }
}

/// Ids a wake names must be usable keys: instance-wait ids are bounded by the
/// wait-id cap, signal checkpoint ids must at least be present.
fn check_wake(caps: &Caps, wake: &crate::Wake) -> Result<(), ApiError> {
    match wake {
        crate::Wake::Instances(id) if id.is_empty() => Err(ApiError::new(
            ErrorCode::Invalid,
            "an instances wake needs a wait id",
        )),
        crate::Wake::Instances(id) if id.len() > caps.wait_id_bytes => Err(ApiError::new(
            ErrorCode::TooLarge,
            format!("a wait id is at most {} bytes", caps.wait_id_bytes),
        )),
        crate::Wake::OnSignal(wait) if wait.checkpoint_id.is_empty() => Err(ApiError::new(
            ErrorCode::Invalid,
            "an onSignal wake needs a checkpoint id",
        )),
        _ => Ok(()),
    }
}

async fn post_run_outcome(
    State(state): State<WorkflowApiState>,
    request: Request,
) -> Result<Response, Failure> {
    let binding = binding_of(&request)?;
    let (parts, body) = request.into_parts();
    let body = to_bytes(body, state.caps.request_body_bytes)
        .await
        .map_err(|_| ApiError::new(ErrorCode::TooLarge, "outcome body is over the cap"))?;
    let outcome = parse_outcome(&state.caps, parts.headers.get(headers::OUTCOME), body)?;
    state.backend.post_outcome(&binding, outcome).await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DeadlineRequest {
    abort_after_ms: u64,
}

async fn put_run_deadline(
    State(state): State<WorkflowApiState>,
    request: Request,
) -> Result<Response, Failure> {
    let binding = binding_of(&request)?;
    let body = to_bytes(request.into_body(), state.caps.request_body_bytes)
        .await
        .map_err(|_| ApiError::new(ErrorCode::TooLarge, "deadline body is over the cap"))?;
    let deadline: DeadlineRequest = serde_json::from_slice(&body)
        .map_err(|e| ApiError::new(ErrorCode::Invalid, format!("deadline: {e}")))?;
    state
        .backend
        .set_deadline(&binding, deadline.abort_after_ms)
        .await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

#[cfg(test)]
mod tests;
