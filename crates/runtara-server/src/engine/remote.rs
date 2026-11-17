// Copyright (C) 2026 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! The API's side of the wire: an [`Engine`] that asks an engine in another
//! process, over mutual TLS.
//!
//! The API role's certificate is accepted by the engine's listener and by
//! nothing else, and the engine's is the only one this side accepts. A call
//! that cannot be made says so in the error of the operation it was; one that
//! was sent and not answered in time says that what was asked may have been
//! done.

use std::net::SocketAddr;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use axum::body::Body;
use axum::http::{self, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use base64::Engine as _;
use bytes::Bytes;
use runtara_executor::tls;

use super::wire::{self, EngineFacts, Operation};
use super::*;

/// The name in the API role's certificate. The engine's listener accepts a
/// certificate for this name alone; the run channel, which takes an
/// executor's, refuses it.
pub const API_NAME: &str = runtara_executor::protocol::API_NAME;

/// How long an ordinary call may take.
const ORDINARY: Duration = Duration::from_secs(60);
/// How long a call that is held until work is done may take: running a
/// workflow and waiting for it, compiling, running an agent, and any request
/// the engine serves.
const HELD: Duration = Duration::from_secs(15 * 60);
/// The largest answer to an operation this side reads.
const ANSWER_LIMIT: usize = 256 * 1024 * 1024;
/// The largest body of a request handed to the engine: what the engine's own
/// routes accept at most.
const HANDED_OVER_LIMIT: usize = 96 * 1024 * 1024;

/// Why the engine did not answer.
#[derive(Debug, Clone)]
pub struct Away {
    /// Whether the call may have reached the engine. `false` if it was never
    /// sent: nothing was done.
    pub sent: bool,
    pub reason: String,
}

impl std::fmt::Display for Away {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.sent {
            write!(
                f,
                "the engine did not answer ({}); what was asked of it may have been done",
                self.reason
            )
        } else {
            write!(f, "the engine cannot be reached ({})", self.reason)
        }
    }
}

/// How a call is carried to the engine.
#[async_trait]
pub(crate) trait Transport: Send + Sync {
    async fn send(
        &self,
        request: http::Request<Bytes>,
        within: Duration,
    ) -> Result<http::Response<Body>, Away>;
}

/// Over mutual TLS, to the engine at one address.
struct Mtls {
    client: tls::Client,
    engine: String,
}

#[async_trait]
impl Transport for Mtls {
    async fn send(
        &self,
        request: http::Request<Bytes>,
        within: Duration,
    ) -> Result<http::Response<Body>, Away> {
        let (mut parts, body) = request.into_parts();
        let path = parts
            .uri
            .path_and_query()
            .map(|path| path.as_str())
            .unwrap_or("/");
        parts.uri = format!("{}{path}", self.engine).parse().map_err(|_| Away {
            sent: false,
            reason: "the request's path is not one".to_owned(),
        })?;
        let request = http::Request::from_parts(parts, http_body_util::Full::new(body));
        match tokio::time::timeout(within, self.client.request(request)).await {
            Err(_) => Err(Away {
                sent: true,
                reason: format!("no answer within {} seconds", within.as_secs()),
            }),
            Ok(Err(error)) => Err(Away {
                // A connection that was never made carried nothing.
                sent: !error.is_connect(),
                reason: error.to_string(),
            }),
            Ok(Ok(response)) => Ok(response.map(Body::new)),
        }
    }
}

/// Straight into a router, for tests of both sides in one process.
#[cfg(test)]
pub(crate) struct InProcess(pub axum::Router);

#[cfg(test)]
#[async_trait]
impl Transport for InProcess {
    async fn send(
        &self,
        request: http::Request<Bytes>,
        _within: Duration,
    ) -> Result<http::Response<Body>, Away> {
        use tower::ServiceExt;
        match self.0.clone().oneshot(request.map(Body::from)).await {
            Ok(response) => Ok(response),
            Err(never) => match never {},
        }
    }
}

/// What a call answers when the engine did not.
pub(crate) trait Unanswered {
    fn unanswered(away: Away) -> Self;
}

impl<T, E: From<Away>> Unanswered for Result<T, E> {
    fn unanswered(away: Away) -> Self {
        Err(E::from(away))
    }
}

impl Unanswered for () {
    fn unanswered(away: Away) -> Self {
        tracing::warn!(%away, "A call to the engine was not answered");
    }
}

impl<T> Unanswered for Vec<T> {
    fn unanswered(away: Away) -> Self {
        tracing::warn!(%away, "A call to the engine was not answered");
        Vec::new()
    }
}

impl<T> Unanswered for Option<T> {
    fn unanswered(away: Away) -> Self {
        tracing::warn!(%away, "A call to the engine was not answered");
        None
    }
}

impl From<Away> for ExecutionError {
    fn from(away: Away) -> Self {
        Self::NotConnected(away.to_string())
    }
}

impl From<Away> for RuntimeError {
    fn from(away: Away) -> Self {
        Self::ConnectionFailed(away.to_string())
    }
}

impl From<Away> for InputError {
    fn from(away: Away) -> Self {
        Self::Storage(away.to_string())
    }
}

impl From<Away> for WorkflowRuntimeError {
    fn from(away: Away) -> Self {
        Self::Runtime(away.to_string())
    }
}

impl From<Away> for CompilationError {
    fn from(away: Away) -> Self {
        Self::CompilationError(away.to_string())
    }
}

impl From<Away> for CompilationQueueError {
    fn from(away: Away) -> Self {
        Self::ConnectionError(away.to_string())
    }
}

impl From<Away> for AgentTestError {
    fn from(away: Away) -> Self {
        Self::ExecutionError(away.to_string())
    }
}

impl From<Away> for String {
    fn from(away: Away) -> Self {
        away.to_string()
    }
}

/// An engine in another process.
pub struct RemoteEngine {
    transport: Arc<dyn Transport>,
    /// What the engine last said of itself. Until it has, it is taken to be
    /// able to do what it is asked, and a call it cannot answer says why.
    facts: RwLock<Option<EngineFacts>>,
    /// Whether what the engine says it compiles by and runs becomes what
    /// this process judges compiled workflows by: so for the API role's one
    /// engine, and not for an engine a test stands beside its own.
    judges_by_it: bool,
}

impl RemoteEngine {
    /// To the engine at `engine` (`https://host:port`), with this side's
    /// certificate from `tls_dir`.
    pub fn connect(engine: &str, tls_dir: &std::path::Path) -> Result<Arc<Self>, String> {
        let engine = engine.trim_end_matches('/').to_owned();
        if !engine.starts_with("https://") {
            return Err("the engine's address is not an https:// one".to_owned());
        }
        let identity = tls::Identity::load(tls_dir)?;
        let config = identity.client_config(&identity.ca_pem, None)?;
        let client = tls::client(config, runtara_executor::protocol::SERVER_NAME)?;
        Ok(Arc::new(Self {
            transport: Arc::new(Mtls { client, engine }),
            facts: RwLock::new(None),
            judges_by_it: true,
        }))
    }

    #[cfg(test)]
    pub(crate) fn over(transport: Arc<dyn Transport>) -> Arc<Self> {
        Arc::new(Self {
            transport,
            facts: RwLock::new(None),
            judges_by_it: false,
        })
    }

    /// Asks the engine what it says of itself, now and then for as long as
    /// this process runs.
    pub fn keep_asking_what_it_is(self: &Arc<Self>) -> impl Future<Output = ()> + Send + 'static {
        let engine = self.clone();
        async move {
            let mut reached = None;
            loop {
                let known = engine.learn().await;
                if reached != Some(known) {
                    if known {
                        tracing::info!("The engine answers");
                    } else {
                        tracing::warn!("The engine does not answer");
                    }
                    reached = Some(known);
                }
                tokio::time::sleep(Duration::from_secs(if known { 30 } else { 2 })).await;
            }
        }
    }

    /// Asks once. Whether the engine answered.
    pub async fn learn(&self) -> bool {
        match self.try_ask(wire::Facts {}, Duration::from_secs(10)).await {
            Ok(facts) => {
                // What this process judges compiled workflows by is the
                // engine's compiler and what the engine runs, not its own.
                if self.judges_by_it {
                    crate::api::repositories::workflows::set_installed_trusted_pins(
                        facts.trusted_pins.iter().cloned(),
                    );
                    crate::api::repositories::workflows::set_engine_compiles_by(
                        facts.compiles_by.clone(),
                    );
                }
                *self
                    .facts
                    .write()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(facts);
                true
            }
            Err(_) => false,
        }
    }

    fn facts(&self) -> Option<EngineFacts> {
        self.facts
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    async fn try_ask<O: Operation>(
        &self,
        operation: O,
        within: Duration,
    ) -> Result<O::Answer, Away> {
        let unreadable = |reason: String| Away { sent: true, reason };
        let body = serde_json::to_vec(&operation).map_err(|error| Away {
            sent: false,
            reason: error.to_string(),
        })?;
        let request = http::Request::post(format!("{}{}", wire::OPERATIONS, O::NAME))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Bytes::from(body))
            .map_err(|error| Away {
                sent: false,
                reason: error.to_string(),
            })?;
        let response = self.transport.send(request, within).await?;
        let status = response.status();
        let answer = tokio::time::timeout(
            within,
            axum::body::to_bytes(response.into_body(), ANSWER_LIMIT),
        )
        .await
        .map_err(|_| unreadable("its answer did not arrive in time".to_owned()))?
        .map_err(|error| unreadable(error.to_string()))?;
        if status != StatusCode::OK {
            return Err(Away {
                // Refused before anything was done: the operation is not one
                // the engine knows, or it could not read it.
                sent: false,
                reason: format!(
                    "it refused {} with {status}: {}",
                    O::NAME,
                    String::from_utf8_lossy(&answer[..answer.len().min(300)])
                ),
            });
        }
        serde_json::from_slice(&answer).map_err(|error| {
            unreadable(format!(
                "its answer to {} could not be read: {error}",
                O::NAME
            ))
        })
    }

    async fn ask<O: Operation>(&self, operation: O) -> O::Answer
    where
        O::Answer: Unanswered,
    {
        self.ask_within(operation, ORDINARY).await
    }

    async fn ask_within<O: Operation>(&self, operation: O, within: Duration) -> O::Answer
    where
        O::Answer: Unanswered,
    {
        match self.try_ask(operation, within).await {
            Ok(answer) => answer,
            Err(away) => O::Answer::unanswered(away),
        }
    }
}

/// The request the API hands the engine for a part it serves: the request as
/// it arrived, with what the API established about it in headers of its own.
/// Headers of those names that came from outside are dropped first.
async fn handed_over(
    part: Served,
    request: axum::extract::Request,
) -> Result<http::Request<Bytes>, Response> {
    let (mut parts, body) = request.into_parts();
    let arrived = parts
        .extensions
        .get::<axum::extract::OriginalUri>()
        .map(|original| original.0.clone())
        .unwrap_or_else(|| parts.uri.clone());
    let own = arrived
        .path_and_query()
        .map(|path| path.as_str())
        .unwrap_or("/");
    let path = format!("{}{}{own}", wire::SERVED, part.name());

    for name in [wire::CALLER, wire::REQUESTED_TENANT, wire::EVENT_SOURCE] {
        parts.headers.remove(name);
    }
    // What belongs to the connection the request came on, and not to it.
    for name in [
        header::CONNECTION.as_str(),
        "keep-alive",
        header::TRANSFER_ENCODING.as_str(),
        header::UPGRADE.as_str(),
        header::TE.as_str(),
        header::HOST.as_str(),
        "proxy-connection",
    ] {
        parts.headers.remove(name);
    }
    let bad = || StatusCode::BAD_REQUEST.into_response();
    if let Some(caller) = parts.extensions.get::<crate::auth::AuthContext>() {
        let json = serde_json::to_vec(caller).map_err(|_| bad())?;
        let value = base64::engine::general_purpose::STANDARD.encode(json);
        parts.headers.insert(
            wire::CALLER,
            HeaderValue::from_str(&value).map_err(|_| bad())?,
        );
    }
    if let Some(tenant) = parts
        .extensions
        .get::<crate::middleware::tenant_path::RequestedTenant>()
    {
        parts.headers.insert(
            wire::REQUESTED_TENANT,
            HeaderValue::from_str(&tenant.0).map_err(|_| bad())?,
        );
    }
    if let Some(source) = parts.extensions.get::<crate::product_events::EventSource>()
        && let Ok(serde_json::Value::String(source)) = serde_json::to_value(source)
    {
        parts.headers.insert(
            wire::EVENT_SOURCE,
            HeaderValue::from_str(&source).map_err(|_| bad())?,
        );
    }
    let body = axum::body::to_bytes(body, HANDED_OVER_LIMIT)
        .await
        .map_err(|_| StatusCode::PAYLOAD_TOO_LARGE.into_response())?;
    let mut request = http::Request::new(body);
    *request.method_mut() = parts.method;
    *request.uri_mut() = path.parse().map_err(|_| bad())?;
    *request.headers_mut() = parts.headers;
    Ok(request)
}

/// What a caller is told when the engine does not answer a request it serves.
fn engine_away(away: &Away) -> Response {
    tracing::warn!(%away, "A request the engine serves was not answered");
    (
        StatusCode::SERVICE_UNAVAILABLE,
        [(header::RETRY_AFTER, "5")],
        axum::Json(serde_json::json!({
            "error": "ENGINE_UNAVAILABLE",
            "message": away.to_string(),
        })),
    )
        .into_response()
}

#[async_trait]
impl Engine for RemoteEngine {
    fn available(&self) -> bool {
        self.facts().is_none_or(|facts| facts.available)
    }

    async fn queue(&self, request: QueueRequest<'_>) -> Result<QueuedExecution, ExecutionError> {
        self.ask(wire::Queue {
            run_label: request.run_label,
            tenant_id: request.tenant_id.to_owned(),
            workflow_id: request.workflow_id.to_owned(),
            version: request.version,
            inputs: request.inputs,
            debug: request.debug,
            correlation_id: request.correlation_id,
            idempotency_key: request.idempotency_key,
            trigger_source: request.trigger_source,
            instance_id: request.instance_id,
        })
        .await
    }

    async fn run_sync(&self, request: SyncRequest<'_>) -> Result<SyncExecution, ExecutionError> {
        self.ask_within(
            wire::RunSync {
                run_label: request.run_label,
                tenant_id: request.tenant_id.to_owned(),
                workflow_id: request.workflow_id.to_owned(),
                version: request.version,
                inputs: request.inputs,
            },
            HELD,
        )
        .await
    }

    async fn enqueue_trigger_event(
        &self,
        tenant_id: &str,
        event: TriggerEvent,
        idempotency_key: String,
    ) -> Result<EnqueuedExecution, ExecutionError> {
        self.ask(wire::EnqueueTriggerEvent {
            tenant_id: tenant_id.to_owned(),
            event,
            idempotency_key,
        })
        .await
    }

    async fn replay(
        &self,
        tenant_id: &str,
        original_instance_id: &str,
    ) -> Result<QueuedExecution, ExecutionError> {
        self.ask(wire::Replay {
            tenant_id: tenant_id.to_owned(),
            original_instance_id: original_instance_id.to_owned(),
        })
        .await
    }

    async fn stop(
        &self,
        tenant_id: &str,
        instance_id: &str,
    ) -> Result<StopOutcome, ExecutionError> {
        self.ask(wire::Stop {
            tenant_id: tenant_id.to_owned(),
            instance_id: instance_id.to_owned(),
        })
        .await
    }

    async fn pause(
        &self,
        tenant_id: &str,
        instance_id: &str,
    ) -> Result<PauseOutcome, ExecutionError> {
        self.ask(wire::Pause {
            tenant_id: tenant_id.to_owned(),
            instance_id: instance_id.to_owned(),
        })
        .await
    }

    async fn resume(
        &self,
        tenant_id: &str,
        instance_id: &str,
    ) -> Result<ResumeOutcome, ExecutionError> {
        self.ask(wire::Resume {
            tenant_id: tenant_id.to_owned(),
            instance_id: instance_id.to_owned(),
        })
        .await
    }

    async fn get_execution(
        &self,
        tenant_id: &str,
        instance_id: &str,
    ) -> Result<WorkflowInstanceDto, ExecutionError> {
        self.ask(wire::GetExecution {
            tenant_id: tenant_id.to_owned(),
            instance_id: instance_id.to_owned(),
        })
        .await
    }

    async fn get_execution_with_metadata(
        &self,
        workflow_id: &str,
        instance_id: &str,
        tenant_id: &str,
    ) -> Result<ExecutionWithMetadata, ExecutionError> {
        self.ask(wire::GetExecutionWithMetadata {
            workflow_id: workflow_id.to_owned(),
            instance_id: instance_id.to_owned(),
            tenant_id: tenant_id.to_owned(),
        })
        .await
    }

    async fn list_executions(
        &self,
        tenant_id: &str,
        workflow_id: &str,
        page: Option<i32>,
        size: Option<i32>,
    ) -> Result<PageWorkflowInstanceHistoryDto, ExecutionError> {
        self.ask(wire::ListExecutions {
            tenant_id: tenant_id.to_owned(),
            workflow_id: workflow_id.to_owned(),
            page,
            size,
        })
        .await
    }

    async fn list_all_executions(
        &self,
        tenant_id: &str,
        page: Option<i32>,
        size: Option<i32>,
        filters: ExecutionFilters,
    ) -> Result<PageWorkflowInstanceHistoryDto, ExecutionError> {
        self.ask(wire::ListAllExecutions {
            tenant_id: tenant_id.to_owned(),
            page,
            size,
            filters,
        })
        .await
    }

    async fn execution_summary(
        &self,
        tenant_id: &str,
        filters: ExecutionFilters,
    ) -> Result<ExecutionSummary, ExecutionError> {
        self.ask(wire::ExecutionSummaryOf {
            tenant_id: tenant_id.to_owned(),
            filters,
        })
        .await
    }

    async fn execution_listing_options(
        &self,
        tenant_id: &str,
        page: i32,
        size: i32,
        filters: &ExecutionFilters,
    ) -> Result<ListInstancesOptions, ExecutionError> {
        self.ask(wire::ExecutionListingOptions {
            tenant_id: tenant_id.to_owned(),
            page,
            size,
            filters: filters.clone(),
        })
        .await
    }

    async fn authorize_execution(
        &self,
        workflow_id: &str,
        instance_id: &str,
        tenant_id: &str,
    ) -> Result<InstanceInfo, ExecutionError> {
        self.ask(wire::AuthorizeExecution {
            workflow_id: workflow_id.to_owned(),
            instance_id: instance_id.to_owned(),
            tenant_id: tenant_id.to_owned(),
        })
        .await
    }

    async fn get_instance_info(
        &self,
        tenant_id: &str,
        instance_id: &str,
    ) -> Result<InstanceInfo, RuntimeError> {
        self.ask(wire::GetInstanceInfo {
            tenant_id: tenant_id.to_owned(),
            instance_id: instance_id.to_owned(),
        })
        .await
    }

    async fn list_events(
        &self,
        tenant_id: &str,
        instance_id: &str,
        options: Option<ListEventsOptions>,
    ) -> Result<ListEventsResult, RuntimeError> {
        self.ask(wire::ListEvents {
            tenant_id: tenant_id.to_owned(),
            instance_id: instance_id.to_owned(),
            options,
        })
        .await
    }

    async fn list_step_summaries(
        &self,
        tenant_id: &str,
        instance_id: &str,
        options: Option<ListStepSummariesOptions>,
    ) -> Result<ListStepSummariesResult, RuntimeError> {
        self.ask(wire::ListStepSummaries {
            tenant_id: tenant_id.to_owned(),
            instance_id: instance_id.to_owned(),
            options,
        })
        .await
    }

    async fn get_scope_ancestors(
        &self,
        tenant_id: &str,
        instance_id: &str,
        scope_id: &str,
    ) -> Result<Vec<ScopeInfo>, RuntimeError> {
        self.ask(wire::GetScopeAncestors {
            tenant_id: tenant_id.to_owned(),
            instance_id: instance_id.to_owned(),
            scope_id: scope_id.to_owned(),
        })
        .await
    }

    async fn list_checkpoints(
        &self,
        tenant_id: &str,
        instance_id: &str,
        limit: Option<u32>,
        offset: Option<u32>,
    ) -> Result<ListCheckpointsResult, RuntimeError> {
        self.ask(wire::ListCheckpoints {
            tenant_id: tenant_id.to_owned(),
            instance_id: instance_id.to_owned(),
            limit,
            offset,
        })
        .await
    }

    async fn get_tenant_metrics(
        &self,
        options: GetTenantMetricsOptions,
    ) -> Result<TenantMetricsResult, RuntimeError> {
        self.ask(wire::GetTenantMetrics { options }).await
    }

    async fn operation_requests(
        &self,
        tenant_id: &str,
        workflow_id: &str,
        key: &str,
        listing: &ListInstancesOptions,
        fields: &[String],
    ) -> Result<runtara_environment::operations::RequestPage, RuntimeError> {
        self.ask(wire::OperationRequests {
            tenant_id: tenant_id.to_owned(),
            workflow_id: workflow_id.to_owned(),
            key: key.to_owned(),
            listing: listing.clone(),
            fields: fields.to_vec(),
        })
        .await
    }

    async fn list_input_requests(
        &self,
        tenant_id: &str,
        instances: &[String],
        offset: u64,
        limit: u32,
    ) -> InputResult<InputRequestPage> {
        self.ask(wire::ListInputRequests {
            tenant_id: tenant_id.to_owned(),
            instances: instances.to_vec(),
            offset,
            limit,
        })
        .await
    }

    async fn get_input_request(
        &self,
        tenant_id: &str,
        instance_id: &str,
        request: &str,
    ) -> InputResult<InputRequest> {
        self.ask(wire::GetInputRequest {
            tenant_id: tenant_id.to_owned(),
            instance_id: instance_id.to_owned(),
            request: request.to_owned(),
        })
        .await
    }

    async fn submit_input_response(
        &self,
        tenant_id: &str,
        instance_id: &str,
        request: &str,
        operation: &str,
        payload: &Value,
    ) -> InputResult<InputReceipt> {
        self.ask(wire::SubmitInputResponse {
            tenant_id: tenant_id.to_owned(),
            instance_id: instance_id.to_owned(),
            request: request.to_owned(),
            operation: operation.to_owned(),
            payload: payload.clone(),
        })
        .await
    }

    async fn submit_authenticated_input_response(
        &self,
        auth: &crate::auth::AuthContext,
        instance_id: &str,
        request: &str,
        operation: &str,
        payload: &Value,
    ) -> InputResult<InputReceipt> {
        self.ask(wire::SubmitAuthenticatedInputResponse {
            auth: auth.clone(),
            instance_id: instance_id.to_owned(),
            request: request.to_owned(),
            operation: operation.to_owned(),
            payload: payload.clone(),
        })
        .await
    }

    async fn pending_input_page(
        &self,
        tenant_id: &str,
        instance_id: &str,
    ) -> InputResult<PendingInputPage> {
        self.ask(wire::PendingInputPageOf {
            tenant_id: tenant_id.to_owned(),
            instance_id: instance_id.to_owned(),
        })
        .await
    }

    async fn list_instance_actions(
        &self,
        tenant_id: &str,
        workflow_id: &str,
        instance_id: &str,
    ) -> Result<Vec<WorkflowRuntimeAction>, WorkflowRuntimeError> {
        self.ask(wire::ListInstanceActions {
            tenant_id: tenant_id.to_owned(),
            workflow_id: workflow_id.to_owned(),
            instance_id: instance_id.to_owned(),
        })
        .await
    }

    async fn list_workflow_actions(
        &self,
        tenant_id: &str,
        workflow_id: &str,
        page: Option<i32>,
        size: Option<i32>,
    ) -> Result<WorkflowRuntimeActionPage, WorkflowRuntimeError> {
        self.ask(wire::ListWorkflowActions {
            tenant_id: tenant_id.to_owned(),
            workflow_id: workflow_id.to_owned(),
            page,
            size,
        })
        .await
    }

    async fn submit_workflow_action(
        &self,
        auth: &crate::auth::AuthContext,
        workflow_id: &str,
        instance_id: &str,
        request_id: &str,
        operation_id: &str,
        payload: &Value,
    ) -> Result<WorkflowActionReceipt, WorkflowRuntimeError> {
        self.ask(wire::SubmitWorkflowAction {
            auth: auth.clone(),
            workflow_id: workflow_id.to_owned(),
            instance_id: instance_id.to_owned(),
            request_id: request_id.to_owned(),
            operation_id: operation_id.to_owned(),
            payload: payload.clone(),
        })
        .await
    }

    async fn submit_authenticated_input(
        &self,
        auth: &crate::auth::AuthContext,
        instance_id: &str,
        request_id: &str,
        operation_id: &str,
        payload: &Value,
    ) -> Result<WorkflowActionReceipt, WorkflowRuntimeError> {
        self.ask(wire::SubmitAuthenticatedInput {
            auth: auth.clone(),
            instance_id: instance_id.to_owned(),
            request_id: request_id.to_owned(),
            operation_id: operation_id.to_owned(),
            payload: payload.clone(),
        })
        .await
    }

    async fn operation_queues(
        &self,
        tenant_id: &str,
    ) -> Result<Vec<OperationQueue>, WorkflowRuntimeError> {
        self.ask(wire::OperationQueues {
            tenant_id: tenant_id.to_owned(),
        })
        .await
    }

    async fn compile_workflow(
        &self,
        tenant_id: &str,
        workflow_id: &str,
        version: i32,
        force_recompile: bool,
    ) -> Result<CompilationResultDto, CompilationError> {
        self.ask_within(
            wire::CompileWorkflow {
                tenant_id: tenant_id.to_owned(),
                workflow_id: workflow_id.to_owned(),
                version,
                force_recompile,
            },
            HELD,
        )
        .await
    }

    async fn publish_workflow_agent(
        &self,
        tenant_id: &str,
        workflow_id: &str,
        version: i32,
        slug: String,
    ) -> Result<Value, CompilationError> {
        self.ask_within(
            wire::PublishWorkflowAgent {
                tenant_id: tenant_id.to_owned(),
                workflow_id: workflow_id.to_owned(),
                version,
                slug,
            },
            HELD,
        )
        .await
    }

    async fn get_image(
        &self,
        tenant_id: &str,
        image_id: &str,
    ) -> Result<Option<ImageSummary>, RuntimeError> {
        self.ask(wire::GetImage {
            tenant_id: tenant_id.to_owned(),
            image_id: image_id.to_owned(),
        })
        .await
    }

    async fn workflow_agents(&self, tenant_id: &str) -> Vec<runtara_dsl::agent_meta::AgentInfo> {
        self.ask(wire::WorkflowAgents {
            tenant_id: tenant_id.to_owned(),
        })
        .await
    }

    async fn unstage_workflow_agent(&self, tenant_id: &str, slug: &str) -> Result<(), String> {
        self.ask(wire::UnstageWorkflowAgent {
            tenant_id: tenant_id.to_owned(),
            slug: slug.to_owned(),
        })
        .await
    }

    async fn tenant_connections(&self, tenant_id: &str) -> Result<Vec<ConnectionRef>, String> {
        self.ask(wire::TenantConnections {
            tenant_id: tenant_id.to_owned(),
        })
        .await
    }

    async fn reconcile_channel_webhooks(&self, tenant_id: &str, removed: &[InvocationTrigger]) {
        self.ask(wire::ReconcileChannelWebhooks {
            tenant_id: tenant_id.to_owned(),
            removed: removed.to_vec(),
        })
        .await
    }

    fn queues_compilations(&self) -> bool {
        self.facts().is_none_or(|facts| facts.queues_compilations)
    }

    async fn enqueue_compilation(
        &self,
        tenant_id: &str,
        workflow_id: &str,
        version: i32,
        force_recompile: bool,
        compiled: Option<ProductEvent>,
    ) -> Result<bool, CompilationQueueError> {
        self.ask(wire::EnqueueCompilation {
            tenant_id: tenant_id.to_owned(),
            workflow_id: workflow_id.to_owned(),
            version,
            force_recompile,
            compiled,
        })
        .await
    }

    async fn wait_for_compilation(
        &self,
        tenant_id: &str,
        workflow_id: &str,
        version: i32,
        timeout: Duration,
    ) -> Result<bool, CompilationQueueError> {
        self.ask_within(
            wire::WaitForCompilation {
                tenant_id: tenant_id.to_owned(),
                workflow_id: workflow_id.to_owned(),
                version,
                timeout_ms: u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX),
            },
            // The engine answers when the wait is over; a little longer here.
            timeout.saturating_add(Duration::from_secs(10)),
        )
        .await
    }

    async fn is_compilation_pending(
        &self,
        tenant_id: &str,
        workflow_id: &str,
        version: i32,
    ) -> Result<bool, CompilationQueueError> {
        self.ask(wire::IsCompilationPending {
            tenant_id: tenant_id.to_owned(),
            workflow_id: workflow_id.to_owned(),
            version,
        })
        .await
    }

    async fn mark_compilation_queued(&self, tenant_id: &str, workflow_id: &str, version: i32) {
        self.ask(wire::MarkCompilationQueued {
            tenant_id: tenant_id.to_owned(),
            workflow_id: workflow_id.to_owned(),
            version,
        })
        .await
    }

    async fn compilation_progress(
        &self,
        tenant_id: &str,
        workflow_id: &str,
        version: i32,
    ) -> Option<CompilationProgress> {
        self.ask(wire::CompilationProgressOf {
            tenant_id: tenant_id.to_owned(),
            workflow_id: workflow_id.to_owned(),
            version,
        })
        .await
    }

    fn tests_agents(&self) -> bool {
        self.facts().is_none_or(|facts| facts.tests_agents)
    }

    async fn test_agent(
        &self,
        tenant_id: &str,
        agent_name: &str,
        capability_id: &str,
        input: Value,
        connection_id: Option<String>,
        engine: TestEngine,
    ) -> Result<TestResult, AgentTestError> {
        self.ask_within(
            wire::TestAgent {
                tenant_id: tenant_id.to_owned(),
                agent_name: agent_name.to_owned(),
                capability_id: capability_id.to_owned(),
                input,
                connection_id,
                engine,
            },
            HELD,
        )
        .await
    }

    async fn serve(&self, part: Served, request: axum::extract::Request) -> Response {
        let request = match handed_over(part, request).await {
            Ok(request) => request,
            Err(refused) => return refused,
        };
        match self.transport.send(request, HELD).await {
            Ok(response) => response.into_response(),
            Err(away) => engine_away(&away),
        }
    }
}

/// Serves `router` (see [`super::listener::router`]) on `address` to the API
/// role and to no one else: a connection whose certificate is not the API
/// role's is closed after the handshake. Returns the address it listens on
/// and the serving itself, which runs until dropped.
pub async fn serve_api_role(
    address: SocketAddr,
    tls_dir: &std::path::Path,
    router: axum::Router,
) -> Result<(SocketAddr, impl Future<Output = ()> + Send + 'static), String> {
    let identity = tls::Identity::load(tls_dir)?;
    let config = identity.server_config()?;
    let listener = tls::bind(address).await?;
    let bound = listener.local_addr().map_err(|error| error.to_string())?;
    Ok((bound, tls::serve(listener, config, API_NAME, router)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::extract::{MatchedPath, Path, RawQuery};
    use axum::routing::{get, post};
    use tower::ServiceExt;

    async fn text(response: Response) -> (StatusCode, String) {
        let status = response.status();
        let body = axum::body::to_bytes(response.into_body(), 1 << 20)
            .await
            .unwrap();
        (status, String::from_utf8(body.to_vec()).unwrap())
    }

    /// An engine that serves one part with a handler that says what it saw.
    fn engine_side() -> Arc<LocalEngine> {
        let engine = local_without_runtime();
        engine.serves(
            Served::Triggers,
            axum::Router::new().route(
                "/api/runtime/triggers/{id}",
                post(
                    |Path(id): Path<String>,
                     matched: MatchedPath,
                     RawQuery(query): RawQuery,
                     request: axum::extract::Request| async move {
                        let caller = request
                            .extensions()
                            .get::<crate::auth::AuthContext>()
                            .map(|caller| format!("{} of {}", caller.user_id, caller.org_id));
                        let named = request
                            .extensions()
                            .get::<crate::middleware::tenant_path::RequestedTenant>()
                            .map(|tenant| tenant.0.clone());
                        let source = request
                            .extensions()
                            .get::<crate::product_events::EventSource>()
                            .copied();
                        let forged = request.headers().contains_key(wire::CALLER);
                        let kept = request
                            .headers()
                            .get("x-signature")
                            .and_then(|value| value.to_str().ok())
                            .map(str::to_owned);
                        let scope = runtara_store_postgres::tenant_scope::current();
                        let body = axum::body::to_bytes(request.into_body(), 1 << 20)
                            .await
                            .unwrap();
                        format!(
                            "{id} at {} ?{} caller={caller:?} named={named:?} source={source:?} \
                             scope={scope:?} header={forged} signature={kept:?} body={}",
                            matched.as_str(),
                            query.unwrap_or_default(),
                            String::from_utf8_lossy(&body),
                        )
                    },
                ),
            ),
        );
        engine
    }

    /// The API's side, with the engine in another "process": everything it
    /// asks goes through the wire's two halves.
    fn api_side(engine: Arc<LocalEngine>) -> Arc<RemoteEngine> {
        RemoteEngine::over(Arc::new(InProcess(super::super::listener::router(engine))))
    }

    fn api(engine: Arc<dyn Engine>) -> axum::Router {
        axum::Router::new().nest("/api/runtime/triggers", forwarded(engine, Served::Triggers))
    }

    fn caller(tenant: &str, user: &str) -> crate::auth::AuthContext {
        crate::auth::AuthContext::new(
            tenant.to_owned(),
            user.to_owned(),
            crate::auth::AuthMethod::Jwt,
        )
    }

    #[tokio::test]
    async fn a_request_the_engine_serves_crosses_as_it_arrived() {
        let api = api(api_side(engine_side()));
        let mut request = http::Request::post("/api/runtime/triggers/t-1?page=2&q=a%20b")
            .header("x-signature", "sha256=abc")
            .body(Body::from("the body"))
            .unwrap();
        request.extensions_mut().insert(caller("acme", "ann"));
        request
            .extensions_mut()
            .insert(crate::middleware::tenant_path::RequestedTenant(
                "acme".to_owned(),
            ));
        request
            .extensions_mut()
            .insert(crate::product_events::EventSource::Mcp);
        let (status, seen) = text(api.oneshot(request).await.unwrap()).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            seen,
            "t-1 at /api/runtime/triggers/{id} ?page=2&q=a%20b caller=Some(\"ann of acme\") \
             named=Some(\"acme\") source=Some(Mcp) scope=Some(\"acme\") header=false \
             signature=Some(\"sha256=abc\") body=the body"
        );
    }

    /// What says who the caller is comes from the API and from nowhere else:
    /// a header of that name sent from outside is dropped, whether or not the
    /// API authenticated anyone.
    #[tokio::test]
    async fn a_caller_named_from_outside_is_not_the_caller() {
        let api = api(api_side(engine_side()));
        let forged = base64::engine::general_purpose::STANDARD
            .encode(serde_json::to_vec(&caller("victim", "mallory")).unwrap());
        let ask = |authenticated: Option<crate::auth::AuthContext>| {
            let mut request = http::Request::post("/api/runtime/triggers/t-1")
                .header(wire::CALLER, &forged)
                .header(wire::REQUESTED_TENANT, "victim")
                .body(Body::empty())
                .unwrap();
            if let Some(caller) = authenticated {
                request.extensions_mut().insert(caller);
            }
            let api = api.clone();
            async move { text(api.oneshot(request).await.unwrap()).await.1 }
        };
        let seen = ask(None).await;
        assert!(seen.contains("caller=None named=None"), "{seen}");
        assert!(seen.contains("scope=None"), "{seen}");
        let seen = ask(Some(caller("acme", "ann"))).await;
        assert!(
            seen.contains("caller=Some(\"ann of acme\") named=None"),
            "{seen}"
        );
        assert!(seen.contains("scope=Some(\"acme\")"), "{seen}");
    }

    #[tokio::test]
    async fn an_operation_is_answered_as_the_engine_in_this_process_answers_it() {
        let local = engine_side();
        let remote = api_side(local.clone());
        // An error with its fields, and a plain answer.
        assert_eq!(
            format!("{:?}", remote.stop("acme", "run-1").await),
            format!("{:?}", local.stop("acme", "run-1").await),
        );
        assert_eq!(
            format!("{:?}", remote.tenant_connections("acme").await),
            format!("{:?}", local.tenant_connections("acme").await),
        );
        assert_eq!(
            format!("{:?}", remote.get_instance_info("acme", "run-1").await),
            format!("{:?}", local.get_instance_info("acme", "run-1").await),
        );
        // What the engine says of itself is learnt, and until then assumed.
        assert!(remote.available() && remote.tests_agents());
        assert!(remote.learn().await);
        assert_eq!(remote.available(), local.available());
        assert_eq!(remote.tests_agents(), local.tests_agents());
        assert_eq!(remote.queues_compilations(), local.queues_compilations());
    }

    #[tokio::test]
    async fn the_engine_knows_each_operation_by_one_name_and_no_other() {
        let mut names = wire::NAMES.to_vec();
        names.sort_unstable();
        names.dedup();
        assert_eq!(
            names.len(),
            wire::NAMES.len(),
            "two operations share a name"
        );

        let engine = super::super::listener::router(engine_side());
        let ask = |method: http::Method, path: &str, body: &'static str| {
            let request = http::Request::builder()
                .method(method)
                .uri(path)
                .body(Body::from(body))
                .unwrap();
            let engine = engine.clone();
            async move { engine.oneshot(request).await.unwrap().status() }
        };
        let stop = format!("{}Stop", wire::OPERATIONS);
        let post = http::Method::POST;
        assert_eq!(
            ask(
                post.clone(),
                &stop,
                r#"{"tenant_id":"acme","instance_id":"r"}"#
            )
            .await,
            StatusCode::OK
        );
        assert_eq!(
            ask(post.clone(), &stop, "{}").await,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            ask(http::Method::GET, &stop, "").await,
            StatusCode::METHOD_NOT_ALLOWED
        );
        assert_eq!(
            ask(post.clone(), &format!("{}Unknown", wire::OPERATIONS), "{}").await,
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            ask(post.clone(), &format!("{}unknown/x", wire::SERVED), "").await,
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            ask(post, "/api/runtime/workflows", "").await,
            StatusCode::NOT_FOUND
        );
    }

    /// A transport to an engine that is not there.
    struct Gone;

    #[async_trait]
    impl Transport for Gone {
        async fn send(
            &self,
            _request: http::Request<Bytes>,
            _within: Duration,
        ) -> Result<http::Response<Body>, Away> {
            Err(Away {
                sent: false,
                reason: "connection refused".to_owned(),
            })
        }
    }

    #[tokio::test]
    async fn an_engine_that_is_away_is_an_error_that_says_so() {
        let remote = RemoteEngine::over(Arc::new(Gone));
        match remote.stop("acme", "run-1").await {
            Err(ExecutionError::NotConnected(why)) => {
                assert!(why.contains("the engine cannot be reached"), "{why}");
            }
            other => panic!("{other:?}"),
        }
        assert!(remote.compilation_progress("acme", "w", 1).await.is_none());
        assert!(!remote.learn().await);

        let request = http::Request::post("/api/runtime/triggers/t-1")
            .body(Body::empty())
            .unwrap();
        let (status, body) = text(api(remote).oneshot(request).await.unwrap()).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert!(body.contains("ENGINE_UNAVAILABLE"), "{body}");
    }

    /// An engine reached over mutual TLS, which a test stands beside its own
    /// and so does not judge by.
    fn connected(engine: &str, tls_dir: &std::path::Path) -> Arc<RemoteEngine> {
        let connected = RemoteEngine::connect(engine, tls_dir).unwrap();
        Arc::new(RemoteEngine {
            transport: connected.transport.clone(),
            facts: RwLock::new(None),
            judges_by_it: false,
        })
    }

    /// The certificates of a deployment, in three directories.
    struct Certificates {
        _held: tempfile::TempDir,
        engine: std::path::PathBuf,
        executor: std::path::PathBuf,
        api: std::path::PathBuf,
    }

    fn certificates() -> Certificates {
        let held = tempfile::tempdir().unwrap();
        let (engine, executor, api) = (
            held.path().join("engine"),
            held.path().join("executor"),
            held.path().join("api"),
        );
        runtara_executor::pki::init(&engine, &executor, Some(&api), false).unwrap();
        Certificates {
            _held: held,
            engine,
            executor,
            api,
        }
    }

    #[tokio::test]
    async fn the_engine_listens_to_the_api_roles_certificate_and_no_other() {
        let certificates = certificates();
        let (address, serving) = serve_api_role(
            "127.0.0.1:0".parse().unwrap(),
            &certificates.engine,
            super::super::listener::router(engine_side()),
        )
        .await
        .unwrap();
        let serving = tokio::spawn(serving);
        let engine = format!("https://{address}");

        let api_role = connected(&engine, &certificates.api);
        assert!(
            api_role.learn().await,
            "the API role's certificate is accepted"
        );
        assert_eq!(
            format!("{:?}", api_role.tenant_connections("acme").await),
            "Err(\"this engine holds no connections\")"
        );
        let mut request = http::Request::post("/api/runtime/triggers/t-1?x=1")
            .body(Body::from("over the wire"))
            .unwrap();
        request.extensions_mut().insert(caller("acme", "ann"));
        let (status, seen) = text(api(api_role).oneshot(request).await.unwrap()).await;
        assert_eq!(status, StatusCode::OK);
        assert!(seen.contains("caller=Some(\"ann of acme\")"), "{seen}");
        assert!(seen.ends_with("body=over the wire"), "{seen}");

        // An executor's certificate is from the same CA, and is not the API's.
        let executor = connected(&engine, &certificates.executor);
        assert!(
            !executor.learn().await,
            "an executor's certificate is refused"
        );
        // Nor is the engine's own.
        let itself = connected(&engine, &certificates.engine);
        assert!(!itself.learn().await);
        serving.abort();
    }

    /// Where the engine listens for executors, the API role's certificate is
    /// not one.
    #[tokio::test]
    async fn the_run_channel_refuses_the_api_roles_certificate() {
        let certificates = certificates();
        let identity = tls::Identity::load(&certificates.engine).unwrap();
        let listener = tls::bind("127.0.0.1:0".parse().unwrap()).await.unwrap();
        let address = listener.local_addr().unwrap();
        let serving = tokio::spawn(tls::serve(
            listener,
            identity.server_config().unwrap(),
            runtara_executor::protocol::EXECUTOR_NAME,
            axum::Router::new().fallback(get(|| async { "the run channel" })),
        ));
        let engine = format!("https://{address}");
        let ask = |directory: &std::path::Path| {
            let remote = connected(&engine, directory);
            async move {
                let request = http::Request::get("/anything").body(Bytes::new()).unwrap();
                remote
                    .transport
                    .send(request, Duration::from_secs(10))
                    .await
                    .map(|response| response.status())
                    .map_err(|away| away.reason)
            }
        };
        assert_eq!(ask(&certificates.executor).await, Ok(StatusCode::OK));
        assert!(ask(&certificates.api).await.is_err());
        serving.abort();
    }

    #[tokio::test]
    async fn an_engine_that_is_not_listening_is_never_sent_anything() {
        let certificates = certificates();
        // A port nothing listens on.
        let port = {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            listener.local_addr().unwrap().port()
        };
        let remote = connected(&format!("https://127.0.0.1:{port}"), &certificates.api);
        let away = remote
            .try_ask(wire::Facts {}, Duration::from_secs(10))
            .await
            .unwrap_err();
        assert!(!away.sent, "{away}");
        assert!(RemoteEngine::connect("http://127.0.0.1:1", &certificates.api).is_err());
    }
}
