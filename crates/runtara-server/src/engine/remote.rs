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
/// The largest body of a request handed to the engine for a caller the API
/// authenticated: what the engine's own routes accept at most.
const HANDED_OVER_LIMIT: usize = 96 * 1024 * 1024;

/// The largest body the API reads before it hands a request over. A part
/// anyone may send to (a webhook, an OAuth redirect) is held to what the
/// engine's routes for it accept, so that no one makes this process hold
/// more than the engine would have taken.
fn handed_over_limit(part: Served) -> usize {
    match part {
        Served::Events => {
            crate::api::handlers::events::webhook_max_body_bytes().min(HANDED_OVER_LIMIT)
        }
        // A redirect from a provider carries its answer in the query.
        Served::OAuthCallbacks => 64 * 1024,
        Served::Connections | Served::ObjectModel | Served::Triggers => HANDED_OVER_LIMIT,
    }
}

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

impl From<Away> for crate::api::services::conversations::ConversationError {
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
        Self::connect_to(
            engine,
            tls_dir,
            runtara_executor::protocol::SERVER_NAME,
            true,
        )
    }

    /// To intake at `intake`, for the API: the parts of the API intake
    /// serves are handed to it, and it is asked nothing else.
    pub fn connect_to_intake(intake: &str, tls_dir: &std::path::Path) -> Result<Arc<Self>, String> {
        Self::connect_to(
            intake,
            tls_dir,
            runtara_executor::protocol::INTAKE_NAME,
            false,
        )
    }

    /// To the engine at `engine`, for intake: it asks the few operations
    /// anyone may have done ([`wire::ASKED_BY_INTAKE`]), and judges nothing
    /// by what the engine says of itself.
    pub fn connect_as_intake(engine: &str, tls_dir: &std::path::Path) -> Result<Arc<Self>, String> {
        Self::connect_to(
            engine,
            tls_dir,
            runtara_executor::protocol::SERVER_NAME,
            false,
        )
    }

    fn connect_to(
        engine: &str,
        tls_dir: &std::path::Path,
        server_name: &str,
        judges_by_it: bool,
    ) -> Result<Arc<Self>, String> {
        let engine = engine.trim_end_matches('/').to_owned();
        if !engine.starts_with("https://") {
            return Err("the engine's address is not an https:// one".to_owned());
        }
        let identity = tls::Identity::load(tls_dir)?;
        let config = identity.client_config(&identity.ca_pem, None)?;
        let client = tls::client(config, server_name)?;
        Ok(Arc::new(Self {
            transport: Arc::new(Mtls { client, engine }),
            facts: RwLock::new(None),
            judges_by_it,
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

    /// Runs a workflow and waits, for anyone: what intake asks, having
    /// received a call to the synchronous trigger.
    pub async fn run_sync_for_anyone(
        &self,
        request: SyncRequest<'_>,
    ) -> Result<SyncExecution, ExecutionError> {
        self.ask_within(
            wire::RunSyncForAnyone {
                run_label: request.run_label,
                tenant_id: request.tenant_id.to_owned(),
                workflow_id: request.workflow_id.to_owned(),
                inputs: request.inputs,
            },
            HELD,
        )
        .await
    }

    /// Finishes an OAuth flow, for anyone: what intake asks, having received
    /// a provider's redirect.
    pub async fn oauth_redirect_for_anyone(
        &self,
        path_and_query: String,
        cookie: Option<String>,
    ) -> Result<HandedBack, String> {
        self.ask(wire::OAuthRedirectForAnyone {
            path_and_query,
            cookie,
        })
        .await
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
        let mut request = http::Request::post(format!("{}{}", wire::OPERATIONS, O::NAME))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Bytes::from(body))
            .map_err(|error| Away {
                sent: false,
                reason: error.to_string(),
            })?;
        if let Some(presented) = presented_header() {
            request.headers_mut().insert(wire::PRESENTED, presented);
        }
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

/// What the caller of the current work presented, as it is sent with a call
/// made for them.
fn presented_header() -> Option<HeaderValue> {
    let presented = crate::auth::presented::presented()?;
    let json = serde_json::to_vec(&presented).ok()?;
    let mut value =
        HeaderValue::from_str(&base64::engine::general_purpose::STANDARD.encode(json)).ok()?;
    value.set_sensitive(true);
    Some(value)
}

/// The request the API hands the engine for a part it serves: the request as
/// it arrived, with what its caller presented and what the API says of how
/// it arrived in headers of its own. Headers of those names that came from
/// outside are dropped first.
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

    for name in [wire::PRESENTED, wire::REQUESTED_TENANT, wire::EVENT_SOURCE] {
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
    if let Some(presented) = presented_header() {
        parts.headers.insert(wire::PRESENTED, presented);
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
    let body = axum::body::to_bytes(body, handed_over_limit(part))
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

    async fn open_conversation(
        &self,
        tenant_id: &str,
        workflow_id: &str,
        key: &str,
        open: OpenConversation,
    ) -> ConversationResult<ConversationState> {
        self.ask(wire::OpenConversationWith {
            tenant_id: tenant_id.to_owned(),
            workflow_id: workflow_id.to_owned(),
            key: key.to_owned(),
            open,
        })
        .await
    }

    async fn conversation(
        &self,
        tenant_id: &str,
        workflow_id: &str,
        key: &str,
    ) -> ConversationResult<ConversationState> {
        self.ask(wire::ConversationOf {
            tenant_id: tenant_id.to_owned(),
            workflow_id: workflow_id.to_owned(),
            key: key.to_owned(),
        })
        .await
    }

    async fn send_conversation_message(
        &self,
        tenant_id: &str,
        workflow_id: &str,
        key: &str,
        message: Message,
    ) -> ConversationResult<Sent> {
        self.ask(wire::SendConversationMessage {
            tenant_id: tenant_id.to_owned(),
            workflow_id: workflow_id.to_owned(),
            key: key.to_owned(),
            message,
        })
        .await
    }

    async fn conversation_events(
        &self,
        tenant_id: &str,
        workflow_id: &str,
        key: &str,
        after: i64,
        limit: u32,
    ) -> ConversationResult<Vec<ConversationEvent>> {
        self.ask(wire::ConversationEvents {
            tenant_id: tenant_id.to_owned(),
            workflow_id: workflow_id.to_owned(),
            key: key.to_owned(),
            after,
            limit,
        })
        .await
    }

    async fn delete_conversation(
        &self,
        tenant_id: &str,
        workflow_id: &str,
        key: &str,
    ) -> ConversationResult<bool> {
        self.ask(wire::DeleteConversation {
            tenant_id: tenant_id.to_owned(),
            workflow_id: workflow_id.to_owned(),
            key: key.to_owned(),
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

    async fn list_agents(&self, tenant_id: &str) -> Result<AgentListing, String> {
        self.ask(wire::ListAgents {
            tenant_id: tenant_id.to_owned(),
        })
        .await
    }

    async fn agent(
        &self,
        tenant_id: &str,
        name: &str,
        published: bool,
    ) -> Result<Option<runtara_dsl::agent_meta::AgentInfo>, String> {
        self.ask(wire::AgentNamed {
            tenant_id: tenant_id.to_owned(),
            name: name.to_owned(),
            published,
        })
        .await
    }

    async fn components_status(&self, tenant_id: &str) -> Result<ComponentsStatus, String> {
        self.ask(wire::ComponentsStatusOf {
            tenant_id: tenant_id.to_owned(),
        })
        .await
    }

    async fn validate_closure(
        &self,
        tenant_id: &str,
        root_workflow_id: &str,
        root: runtara_dsl::ExecutionGraph,
        children: Vec<runtara_workflows::ClosureChildGraph>,
    ) -> Result<runtara_workflows::ClosureValidationReport, String> {
        self.ask(wire::ValidateClosure {
            tenant_id: tenant_id.to_owned(),
            root_workflow_id: root_workflow_id.to_owned(),
            root,
            children,
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

    async fn api_key_by_hash(
        &self,
        key_hash: &str,
    ) -> Result<crate::api::handlers::api_keys::ApiKey, String> {
        self.ask(wire::ApiKeyByHash {
            key_hash: key_hash.to_owned(),
        })
        .await
    }

    async fn set_current_version(
        &self,
        tenant_id: &str,
        workflow_id: &str,
        version: i32,
    ) -> Result<bool, String> {
        self.ask(wire::SetCurrentVersion {
            tenant_id: tenant_id.to_owned(),
            workflow_id: workflow_id.to_owned(),
            version,
        })
        .await
    }

    async fn invalidate_compilation(
        &self,
        tenant_id: &str,
        workflow_id: &str,
        version: i32,
    ) -> Result<(), String> {
        self.ask(wire::InvalidateCompilation {
            tenant_id: tenant_id.to_owned(),
            workflow_id: workflow_id.to_owned(),
            version,
        })
        .await
    }

    async fn tenant_connections(&self, tenant_id: &str) -> Result<Vec<ConnectionRef>, String> {
        self.ask(wire::TenantConnections {
            tenant_id: tenant_id.to_owned(),
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

    async fn finish_oauth_redirect(
        &self,
        path_and_query: String,
        cookie: Option<String>,
    ) -> Result<HandedBack, String> {
        self.oauth_redirect_for_anyone(path_and_query, cookie).await
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
    // The API's certificate, and intake's, which may ask the few operations
    // anyone may have done and nothing else ([`super::listener`] tells them
    // apart).
    Ok((
        bound,
        tls::serve_peers(
            listener,
            config,
            vec![API_NAME, runtara_executor::protocol::INTAKE_NAME],
            router,
        ),
    ))
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
                        let forged = request.headers().contains_key(wire::PRESENTED);
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

    /// The engine's verification, for tests: a bearer token `user@tenant` is
    /// that user acting for that tenant, unless the request names another;
    /// anything else is nobody.
    struct ByToken;

    #[async_trait]
    impl super::super::listener::Callers for ByToken {
        async fn verify(
            &self,
            presented: &crate::auth::presented::Presented,
        ) -> Result<crate::auth::AuthContext, Response> {
            let headers = presented.headers();
            let found = headers
                .get(header::AUTHORIZATION)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.strip_prefix("Bearer "))
                .and_then(|token| token.split_once('@'));
            let Some((user, tenant)) = found else {
                return Err(StatusCode::UNAUTHORIZED.into_response());
            };
            if presented
                .requested_tenant
                .as_deref()
                .is_some_and(|named| named != tenant)
            {
                return Err(StatusCode::FORBIDDEN.into_response());
            }
            let mut caller = caller(tenant, user);
            caller.role = Some(crate::authz::Role::Member);
            Ok(caller)
        }
    }

    fn listening(engine: Arc<LocalEngine>) -> axum::Router {
        super::super::listener::router(engine, Arc::new(ByToken))
    }

    /// The API's side, with the engine in another "process": everything it
    /// asks goes through the wire's two halves.
    fn api_side(engine: Arc<LocalEngine>) -> Arc<RemoteEngine> {
        RemoteEngine::over(Arc::new(InProcess(listening(engine))))
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

    /// What a caller with the bearer token `token` presented to the API.
    fn presented(token: &str, named: Option<&str>) -> crate::auth::presented::Presented {
        let mut headers = http::HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            format!("Bearer {token}").parse().unwrap(),
        );
        crate::auth::presented::Presented::of(
            crate::auth::presented::Door::Api,
            &headers,
            &[],
            named.map(str::to_owned),
            true,
        )
    }

    /// `work`, as the API does it for a caller with the bearer token `token`.
    async fn asked_by<F: Future>(token: &str, work: F) -> F::Output {
        crate::auth::presented::presenting(presented(token, None), work).await
    }

    #[tokio::test]
    async fn a_request_the_engine_serves_crosses_as_it_arrived() {
        let api = api(api_side(engine_side()));
        let mut request = http::Request::post("/api/runtime/triggers/t-1?page=2&q=a%20b")
            .header("x-signature", "sha256=abc")
            .body(Body::from("the body"))
            .unwrap();
        // What the API's own middleware found is not what crosses.
        request.extensions_mut().insert(caller("other", "eve"));
        request
            .extensions_mut()
            .insert(crate::middleware::tenant_path::RequestedTenant(
                "acme".to_owned(),
            ));
        request
            .extensions_mut()
            .insert(crate::product_events::EventSource::Mcp);
        let (status, seen) =
            crate::auth::presented::presenting(presented("ann@acme", Some("acme")), async {
                text(api.oneshot(request).await.unwrap()).await
            })
            .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            seen,
            "t-1 at /api/runtime/triggers/{id} ?page=2&q=a%20b caller=Some(\"ann of acme\") \
             named=Some(\"acme\") source=Some(Mcp) scope=Some(\"acme\") header=false \
             signature=Some(\"sha256=abc\") body=the body"
        );
    }

    /// Who the caller is, the engine finds from what the caller presented:
    /// not from a header of the wire's name sent from outside, and not from
    /// whom the API says it authenticated.
    #[tokio::test]
    async fn the_caller_is_whom_the_engine_finds_and_not_whom_the_api_names() {
        let api = api(api_side(engine_side()));
        let forged = base64::engine::general_purpose::STANDARD
            .encode(serde_json::to_vec(&presented("mallory@victim", None)).unwrap());
        let ask = |authenticated: Option<crate::auth::AuthContext>| {
            let mut request = http::Request::post("/api/runtime/triggers/t-1")
                .header(wire::PRESENTED, &forged)
                .header(wire::REQUESTED_TENANT, "victim")
                .body(Body::empty())
                .unwrap();
            if let Some(caller) = authenticated {
                request.extensions_mut().insert(caller);
            }
            let api = api.clone();
            async move { text(api.oneshot(request).await.unwrap()).await }
        };
        // Nothing presented to the API: the part is served to nobody, whatever
        // the request's headers and the API say.
        let (status, _) = ask(None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        let (status, _) = ask(Some(caller("victim", "mallory"))).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        // Presented by ann of acme: served to her, whomever the API names.
        let (status, seen) = asked_by("ann@acme", ask(Some(caller("victim", "mallory")))).await;
        assert_eq!(status, StatusCode::OK);
        assert!(
            seen.contains("caller=Some(\"ann of acme\") named=None"),
            "{seen}"
        );
        assert!(seen.contains("scope=Some(\"acme\")"), "{seen}");
        // What nobody's verification accepts is answered as the API's own
        // middleware answers it.
        let (status, _) = asked_by("not a token", ask(None)).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    /// The engine receives nothing anyone may send: a part with no caller
    /// (a webhook, a provider's redirect) is not served by it as it arrived,
    /// to the API or to anyone, whatever is presented with it. Intake
    /// receives those.
    #[tokio::test]
    async fn a_part_with_no_caller_is_not_served_by_the_engine() {
        let local = local_without_runtime();
        let reached = Arc::new(std::sync::atomic::AtomicBool::new(false));
        for part in [Served::Events, Served::OAuthCallbacks] {
            let reached = reached.clone();
            local.serves(
                part,
                axum::Router::new().fallback(move || {
                    reached.store(true, std::sync::atomic::Ordering::SeqCst);
                    async { "served" }
                }),
            );
        }
        let remote = api_side(local);
        for (part, path) in [
            (Served::Events, "/api/runtime/events/webhook/t-1"),
            (Served::OAuthCallbacks, "/api/oauth/acme/callback?code=c"),
        ] {
            let ask = || async {
                let request = http::Request::post(path).body(Body::empty()).unwrap();
                text(remote.serve(part, request).await).await
            };
            for (status, said) in [ask().await, asked_by("ann@acme", ask()).await] {
                assert_eq!(status, StatusCode::FORBIDDEN, "{part:?}");
                assert!(said.contains("NOT_FOR_THE_ENGINE"), "{said}");
            }
        }
        assert!(
            !reached.load(std::sync::atomic::Ordering::SeqCst),
            "nothing of them reached what the engine serves"
        );
    }

    /// A provider's redirect is finished by the engine when intake asks: with
    /// what was asked (a path and its query), nothing of the sender's request,
    /// and for intake alone.
    #[tokio::test]
    async fn a_redirect_is_finished_for_intake_from_what_was_asked_alone() {
        let local = local_without_runtime();
        local.serves(
            Served::OAuthCallbacks,
            axum::Router::new().fallback(|request: axum::extract::Request| async move {
                let cookie = request
                    .headers()
                    .get(http::header::COOKIE)
                    .and_then(|cookie| cookie.to_str().ok())
                    .unwrap_or("none")
                    .to_owned();
                (
                    StatusCode::FOUND,
                    [
                        (http::header::LOCATION, "https://console.example/done"),
                        (http::header::SET_COOKIE, "flow=; Max-Age=0"),
                    ],
                    format!(
                        "{} {} headers={} cookie={cookie}",
                        request.method(),
                        request.uri(),
                        request.headers().len()
                    ),
                )
            }),
        );
        let engine: Arc<dyn Engine> = local;
        let back = engine
            .finish_oauth_redirect(
                "/api/oauth/acme/callback?code=c&state=s".to_owned(),
                Some("flow=abc".to_owned()),
            )
            .await
            .expect("finished");
        assert_eq!(back.status, 302);
        assert_eq!(
            back.location.as_deref(),
            Some("https://console.example/done")
        );
        assert_eq!(
            String::from_utf8(back.body).unwrap(),
            "GET /api/oauth/acme/callback?code=c&state=s headers=1 cookie=flow=abc"
        );
        // What the engine tells the browser to forget goes back with it.
        assert_eq!(back.set_cookies, ["flow=; Max-Age=0"]);
        // A cookie that is not one line of a header is not handed on.
        let back = engine
            .finish_oauth_redirect(
                "/api/oauth/acme/callback?code=c".to_owned(),
                Some("flow=abc\r\nX-Injected: 1".to_owned()),
            )
            .await
            .expect("finished");
        assert!(
            String::from_utf8(back.body)
                .unwrap()
                .ends_with("headers=0 cookie=none")
        );
        // What is not a redirect of an OAuth flow is not handed to anything.
        for asked in [
            "/api/runtime/connections",
            "/api/oauth/acme/callback?x=a b",
            "/api/oauth/../runtime/connections\r\nHost: x",
            "https://elsewhere.example/api/oauth/x",
        ] {
            assert!(
                engine
                    .finish_oauth_redirect(asked.to_owned(), None)
                    .await
                    .is_err(),
                "{asked}"
            );
        }
        let too_long = format!(
            "/api/oauth/acme/callback?code={}",
            "c".repeat(MAX_OAUTH_REDIRECT)
        );
        assert!(engine.finish_oauth_redirect(too_long, None).await.is_err());

        // Over the wire it is intake's to ask, and nobody else's.
        let ask = |peer: wire::Peer| {
            let engine = engine.clone();
            async move {
                wire::answer(
                    engine.as_ref(),
                    "OAuthRedirectForAnyone",
                    br#"{"path_and_query":"/api/oauth/acme/callback?code=c","cookie":null}"#,
                    None,
                    peer,
                )
                .await
                .unwrap()
            }
        };
        assert!(ask(wire::Peer::Intake).await.is_ok());
        assert_eq!(ask(wire::Peer::Api).await, Err(wire::Refused::NotThePeers));
    }

    /// An operation is answered as far as the caller's credential allows,
    /// whether it changes something or reads.
    #[tokio::test]
    async fn an_operation_is_answered_only_as_far_as_the_callers_credential_allows() {
        use crate::authz::{ApiKeyScope, Permission, PermissionSet};
        let scoped = |scope: ApiKeyScope| {
            let mut caller = caller("acme", "ann");
            caller.api_key_scope = scope;
            caller
        };
        let only = |permission| ApiKeyScope::Restricted(PermissionSet::EMPTY.with(permission));
        let ask = |name: &'static str, body: &'static str, caller: crate::auth::AuthContext| async move {
            let engine = engine_side();
            wire::answer(
                engine.as_ref(),
                name,
                body.as_bytes(),
                Some(&caller),
                wire::Peer::Api,
            )
            .await
            .unwrap()
        };
        let run = r#"{"tenant_id":"acme","instance_id":"r"}"#;
        let refused = Err(wire::Refused::NotPermitted);

        // A key that only reads does not stop a run, and reads its history.
        assert_eq!(
            ask("Stop", run, scoped(ApiKeyScope::ReadOnly)).await,
            refused
        );
        assert!(
            ask("GetInstanceInfo", run, scoped(ApiKeyScope::ReadOnly))
                .await
                .is_ok()
        );
        assert!(ask("Stop", run, caller("acme", "ann")).await.is_ok());

        // A key held to connections reads no run, though it is the tenant's.
        let connections = only(Permission::ConnectionRead);
        for name in ["GetInstanceInfo", "GetExecution", "Stop"] {
            assert_eq!(ask(name, run, scoped(connections)).await, refused, "{name}");
        }
        // A key held to running workflows starts a run and sends into a
        // conversation, and reads no run: what was said is history.
        let runs = only(Permission::WorkflowExecute);
        for name in ["GetInstanceInfo", "GetExecution"] {
            assert_eq!(ask(name, run, scoped(runs)).await, refused, "{name}");
        }
        let conversation = r#"{"tenant_id":"acme","workflow_id":"w","key":"k"}"#;
        assert_eq!(
            ask("ConversationOf", conversation, scoped(runs)).await,
            refused
        );
        assert!(
            ask(
                "ConversationOf",
                conversation,
                scoped(ApiKeyScope::ReadOnly)
            )
            .await
            .is_ok()
        );
        // What the API lists to every caller, any member may ask.
        assert!(
            ask(
                "WorkflowAgents",
                r#"{"tenant_id":"acme"}"#,
                scoped(connections)
            )
            .await
            .is_ok()
        );
    }

    /// What intake asks is what anyone may have done, and only intake asks
    /// it; everything else is the API's to ask, for a caller.
    #[tokio::test]
    async fn each_peer_asks_only_what_is_its_to_ask() {
        let engine = engine_side();
        let ask = |name: &'static str, body: &'static str, peer: wire::Peer| {
            let engine = engine.clone();
            async move {
                wire::answer(engine.as_ref(), name, body.as_bytes(), None, peer)
                    .await
                    .unwrap()
            }
        };
        let sync = r#"{"run_label":null,"tenant_id":"acme","workflow_id":"w","inputs":{}}"#;
        let stop = r#"{"tenant_id":"acme","instance_id":"r"}"#;
        // Intake needs no caller for what anyone may have done: the engine
        // answers (here, that it has no runtime).
        assert!(
            ask("RunSyncForAnyone", sync, wire::Peer::Intake)
                .await
                .is_ok()
        );
        // The API does not ask it, with a caller or without.
        assert_eq!(
            ask("RunSyncForAnyone", sync, wire::Peer::Api).await,
            Err(wire::Refused::NotThePeers)
        );
        // Intake asks nothing else, whatever it sends.
        for name in ["Stop", "GetInstanceInfo", "Queue", "WorkflowAgents"] {
            assert_eq!(
                ask(name, stop, wire::Peer::Intake).await,
                Err(wire::Refused::NotThePeers),
                "{name}"
            );
        }
        assert_eq!(
            ask("Facts", "{}", wire::Peer::Intake).await,
            Err(wire::Refused::NotThePeers)
        );
        // Every operation is one or the other's.
        for name in wire::ASKED_BY_INTAKE {
            assert!(wire::NAMES.contains(name), "{name}");
        }
    }

    /// Every operation says what it needs. The few that need nothing are
    /// named here, so that a new one is not open by omission.
    #[test]
    fn every_operation_for_a_tenant_names_what_it_needs() {
        let open: Vec<&str> = wire::NEEDS
            .iter()
            .filter(|(_, needs)| needs.is_empty())
            .map(|(name, _)| *name)
            .collect();
        // What the engine says of itself and whose a key is are no tenant's;
        // agents are listed to every caller.
        assert_eq!(
            open,
            [
                "Facts",
                "OAuthRedirectForAnyone",
                "RunSyncForAnyone",
                "WorkflowAgents",
                "ListAgents",
                "AgentNamed",
                "ComponentsStatusOf",
                "ApiKeyByHash"
            ]
        );
    }

    /// What an operation needs is what the API's routes that ask it need
    /// (`permission_for`), so the engine refuses nothing the API allows and
    /// allows nothing the API refuses.
    #[test]
    fn an_operation_needs_what_the_routes_that_ask_it_need() {
        use crate::authz::Permission::{WorkflowCreate, WorkflowUpdate};
        use crate::middleware::authorization::permission_for;
        use http::Method;
        let get = |path: &str| permission_for(&Method::GET, path).unwrap();
        let post = |path: &str| permission_for(&Method::POST, path).unwrap();
        // Each operation, and the routes whose handlers ask it.
        let asked_by: &[(&str, Vec<crate::authz::Permission>)] = &[
            (
                "GetExecution",
                vec![get("/api/runtime/workflows/instances/{instanceId}")],
            ),
            (
                "GetExecutionWithMetadata",
                vec![get("/api/runtime/workflows/{id}/instances/{instanceId}")],
            ),
            (
                "ListExecutions",
                vec![get("/api/runtime/workflows/{id}/instances")],
            ),
            (
                "ListAllExecutions",
                vec![
                    get("/api/runtime/executions"),
                    post("/api/runtime/executions/query"),
                ],
            ),
            (
                "ExecutionSummaryOf",
                vec![post("/api/runtime/executions/summary")],
            ),
            (
                "ExecutionListingOptions",
                vec![post("/api/runtime/operations/requests/query")],
            ),
            (
                "OperationRequests",
                vec![post("/api/runtime/operations/requests/query")],
            ),
            (
                "OperationQueues",
                vec![get("/api/runtime/operations/queues")],
            ),
            (
                "AuthorizeExecution",
                vec![
                    get("/api/runtime/workflows/{workflowId}/instances/{instanceId}/pending-input"),
                    get("/api/runtime/workflows/{workflowId}/instances/{instanceId}/actions"),
                ],
            ),
            (
                "GetInstanceInfo",
                vec![get(
                    "/api/runtime/workflows/{workflowId}/instances/{instanceId}/steps",
                )],
            ),
            (
                "ListEvents",
                vec![get(
                    "/api/runtime/workflows/{workflowId}/instances/{instanceId}/step-events",
                )],
            ),
            (
                "ListStepSummaries",
                vec![get(
                    "/api/runtime/workflows/{workflowId}/instances/{instanceId}/steps",
                )],
            ),
            (
                "GetScopeAncestors",
                vec![get(
                    "/api/runtime/workflows/{workflowId}/instances/{instanceId}/scopes/{scopeId}/ancestors",
                )],
            ),
            (
                "ListCheckpoints",
                vec![get(
                    "/api/runtime/workflows/{id}/instances/{instanceId}/checkpoints",
                )],
            ),
            ("GetTenantMetrics", vec![get("/api/runtime/metrics/tenant")]),
            // Validating is asked by the validate route, and by saving a
            // workflow (the API and MCP's authoring tools).
            (
                "ValidateClosure",
                vec![
                    post("/api/runtime/workflows/graph/validate"),
                    WorkflowCreate,
                    WorkflowUpdate,
                ],
            ),
            (
                "OpenConversationWith",
                vec![
                    permission_for(
                        &Method::PUT,
                        "/api/runtime/workflows/{id}/conversations/{key}",
                    )
                    .unwrap(),
                ],
            ),
            (
                "ConversationOf",
                vec![get("/api/runtime/workflows/{id}/conversations/{key}")],
            ),
            (
                "SendConversationMessage",
                vec![post(
                    "/api/runtime/workflows/{id}/conversations/{key}/messages",
                )],
            ),
            (
                "ConversationEvents",
                vec![get(
                    "/api/runtime/workflows/{id}/conversations/{key}/events",
                )],
            ),
            (
                "DeleteConversation",
                vec![
                    permission_for(
                        &Method::DELETE,
                        "/api/runtime/workflows/{id}/conversations/{key}",
                    )
                    .unwrap(),
                ],
            ),
            (
                "PendingInputPageOf",
                vec![get(
                    "/api/runtime/workflows/{workflowId}/instances/{instanceId}/pending-input",
                )],
            ),
            (
                "ListInstanceActions",
                vec![get(
                    "/api/runtime/workflows/{workflowId}/instances/{instanceId}/actions",
                )],
            ),
            (
                "ListWorkflowActions",
                vec![get("/api/runtime/workflows/{workflowId}/actions")],
            ),
            (
                "CompilationProgressOf",
                vec![get(
                    "/api/runtime/workflows/{id}/versions/{version}/compilation-progress",
                )],
            ),
            // Compiling is asked by the compile route, and by saving a
            // workflow (the API and MCP's authoring tools).
            (
                "GetImage",
                vec![
                    post("/api/runtime/workflows/{id}/versions/{version}/compile"),
                    WorkflowCreate,
                    WorkflowUpdate,
                ],
            ),
            (
                "WaitForCompilation",
                vec![
                    post("/api/runtime/workflows/{id}/versions/{version}/compile"),
                    WorkflowCreate,
                    WorkflowUpdate,
                ],
            ),
            (
                "IsCompilationPending",
                vec![
                    post("/api/runtime/workflows/{id}/versions/{version}/compile"),
                    WorkflowCreate,
                    WorkflowUpdate,
                ],
            ),
            // A workflow's connections are checked where it is validated and
            // where it is saved.
            (
                "TenantConnections",
                vec![
                    post("/api/runtime/workflows/graph/validate"),
                    post("/api/runtime/workflows/{workflowId}/validate-mappings"),
                    post("/api/runtime/workflows/create"),
                    post("/api/runtime/workflows/{id}/update"),
                ],
            ),
        ];
        for (name, routes) in asked_by {
            let needs = wire::NEEDS
                .iter()
                .find(|(operation, _)| operation == name)
                .unwrap_or_else(|| panic!("no operation {name}"))
                .1;
            let mut expected = routes.clone();
            expected.sort_by_key(|permission| permission.as_str());
            expected.dedup();
            let mut named = needs.to_vec();
            named.sort_by_key(|permission| permission.as_str());
            assert_eq!(named, expected, "{name}");
        }
    }

    /// An operation that names its caller is answered for the caller the
    /// engine found, not the one the API wrote.
    #[tokio::test]
    async fn an_operation_names_the_caller_the_engine_found() {
        let mut operation = wire::SubmitWorkflowAction {
            auth: {
                let mut forged = caller("acme", "mallory");
                forged.role = Some(crate::authz::Role::Owner);
                forged
            },
            workflow_id: "w".to_owned(),
            instance_id: "i".to_owned(),
            request_id: "r".to_owned(),
            operation_id: "o".to_owned(),
            payload: serde_json::Value::Null,
        };
        let mut found = caller("acme", "ann");
        found.role = Some(crate::authz::Role::Viewer);
        wire::Operation::of_caller(&mut operation, &found);
        assert_eq!(operation.auth.user_id, "ann");
        assert_eq!(operation.auth.role, Some(crate::authz::Role::Viewer));
    }

    /// What anyone may send is not read past what the engine takes of it.
    #[tokio::test]
    async fn a_body_too_large_for_its_part_is_not_handed_over() {
        let engine = api_side(engine_side());
        let oversized = vec![b'x'; 64 * 1024 + 1];
        let request = http::Request::post("/api/oauth/provider/callback")
            .body(Body::from(oversized.clone()))
            .unwrap();
        let answer = engine.serve(Served::OAuthCallbacks, request).await;
        assert_eq!(answer.status(), StatusCode::PAYLOAD_TOO_LARGE);
        // The same body is one a signed-in caller's request may carry.
        let request = http::Request::post("/api/runtime/triggers/t-1")
            .body(Body::from(oversized))
            .unwrap();
        let answer = asked_by("ann@acme", engine.serve(Served::Triggers, request)).await;
        assert_eq!(answer.status(), StatusCode::OK);
        assert!(handed_over_limit(Served::Events) <= HANDED_OVER_LIMIT);
    }

    #[tokio::test]
    async fn an_operation_is_answered_as_the_engine_in_this_process_answers_it() {
        let local = engine_side();
        let remote = api_side(local.clone());
        // An error with its fields, and a plain answer.
        assert_eq!(
            format!(
                "{:?}",
                asked_by("ann@acme", remote.stop("acme", "run-1")).await
            ),
            format!("{:?}", local.stop("acme", "run-1").await),
        );
        assert_eq!(
            format!(
                "{:?}",
                asked_by("ann@acme", remote.tenant_connections("acme")).await
            ),
            format!("{:?}", local.tenant_connections("acme").await),
        );
        assert_eq!(
            format!(
                "{:?}",
                asked_by("ann@acme", remote.get_instance_info("acme", "run-1")).await
            ),
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

        let engine = listening(engine_side());
        let by_ann = presented_header_of(&presented("ann@acme", None));
        let ask = |method: http::Method, path: &str, body: &'static str| {
            let request = http::Request::builder()
                .method(method)
                .uri(path)
                .header(wire::PRESENTED, &by_ann)
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

    fn presented_header_of(presented: &crate::auth::presented::Presented) -> String {
        base64::engine::general_purpose::STANDARD.encode(serde_json::to_vec(presented).unwrap())
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
            listening(engine_side()),
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
            format!(
                "{:?}",
                asked_by("ann@acme", api_role.tenant_connections("acme")).await
            ),
            "Err(\"this engine holds no connections\")"
        );
        // The certificate lets the API ask; it does not let it say for whom.
        let unasked = format!("{:?}", api_role.tenant_connections("acme").await);
        assert!(unasked.contains("401"), "{unasked}");
        let request = http::Request::post("/api/runtime/triggers/t-1?x=1")
            .body(Body::from("over the wire"))
            .unwrap();
        let (status, seen) = asked_by("ann@acme", async {
            text(api(api_role).oneshot(request).await.unwrap()).await
        })
        .await;
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
