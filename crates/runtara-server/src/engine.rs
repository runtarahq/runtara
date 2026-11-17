// Copyright (C) 2026 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! What the API asks of the engine.
//!
//! The server is two things in one process: the API, which takes requests
//! (handlers, chat channels, MCP), and the engine, which owns durable
//! execution (instances, checkpoints, signals, launches, and everything done
//! with a connection's credentials). [`Engine`] is the whole of what the
//! first may ask of the second. Code that takes requests holds an
//! `Arc<dyn Engine>` and nothing else of the engine: not the runtime client,
//! not the execution engine, not seal.
//!
//! Today the one implementation, [`LocalEngine`], calls the engine in this
//! process. The interface is kept to what can cross a process boundary, so
//! that the API and the engine can later run apart (see
//! `docs/separating-concerns.md`):
//!
//! - every call names the tenant it is for, instead of relying on the scope
//!   of the task that makes it, which does not leave the process;
//! - nothing is handed across but values: no pool, no client, no callback.

use std::sync::Arc;

use async_trait::async_trait;
use runtara_core::persistence::inputs::{
    InputError, InputReceipt, InputRequest, InputRequestPage, InputResult,
};
use runtara_store_postgres::tenant_scope::scoped;
use serde_json::Value;

use crate::api::dto::executions::{ExecutionFilters, ExecutionSummary};
use crate::api::dto::operations::OperationQueue;
use crate::api::dto::trigger_event::TriggerEvent;
use crate::api::dto::workflows::{PageWorkflowInstanceHistoryDto, WorkflowInstanceDto};
use crate::api::services::compilation::{
    CompilationResultDto, CompilationService, ServiceError as CompilationError,
};
use crate::api::services::pending_inputs::PendingInputPage;
use crate::api::services::workflow_runtime::{
    WorkflowActionReceipt, WorkflowRuntimeAction, WorkflowRuntimeActionPage, WorkflowRuntimeError,
};
use crate::runtime_client::{
    GetTenantMetricsOptions, InstanceInfo, RuntimeClient, RuntimeError, TenantMetricsResult,
};
use crate::runtime_types::{
    ImageSummary, ListCheckpointsResult, ListEventsOptions, ListEventsResult, ListInstancesOptions,
    ListStepSummariesOptions, ListStepSummariesResult, ScopeInfo,
};
use crate::workers::execution_engine::{
    ExecutionEngine, ExecutionError, PauseOutcome, QueueRequest, QueuedExecution, ResumeOutcome,
    StopOutcome, SyncExecution, SyncRequest,
};
use crate::workers::execution_outbox::EnqueuedExecution;
use crate::workers::runtara_dto::ExecutionWithMetadata;

/// A part of the HTTP API the engine serves itself, because answering it uses
/// a connection's credentials: the API hands the engine the request as it
/// arrived, once it has authenticated it, and returns what the engine answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Served {
    /// Connections: their records, types, OAuth authorization and rate limits.
    Connections,
    /// The OAuth redirect a provider sends a user back to.
    OAuthCallbacks,
    /// A tenant's object model: schemas, instances, SQL and CSV.
    ObjectModel,
    /// Webhooks of chat channels, each checked against its connection.
    Channels,
}

/// What the API asks of the engine. See the module.
#[async_trait]
pub trait Engine: Send + Sync {
    /// Whether the engine runs workflows at all. A server started without a
    /// runtime database answers `false`, and what needs a run is refused.
    fn available(&self) -> bool;

    // ---- Launch -----------------------------------------------------------

    /// Accepts a run of a workflow, to be started as capacity allows.
    async fn queue(&self, request: QueueRequest<'_>) -> Result<QueuedExecution, ExecutionError>;

    /// Accepts a run and waits for its result.
    async fn run_sync(&self, request: SyncRequest<'_>) -> Result<SyncExecution, ExecutionError>;

    /// Accepts the run a trigger's event asks for.
    async fn enqueue_trigger_event(
        &self,
        tenant_id: &str,
        event: TriggerEvent,
        idempotency_key: String,
    ) -> Result<EnqueuedExecution, ExecutionError>;

    /// Accepts a new run with the inputs of an earlier one.
    async fn replay(
        &self,
        tenant_id: &str,
        original_instance_id: &str,
    ) -> Result<QueuedExecution, ExecutionError>;

    // ---- Lifecycle --------------------------------------------------------

    async fn stop(&self, tenant_id: &str, instance_id: &str)
    -> Result<StopOutcome, ExecutionError>;

    async fn pause(
        &self,
        tenant_id: &str,
        instance_id: &str,
    ) -> Result<PauseOutcome, ExecutionError>;

    async fn resume(
        &self,
        tenant_id: &str,
        instance_id: &str,
    ) -> Result<ResumeOutcome, ExecutionError>;

    // ---- Runs, as the API shows them --------------------------------------

    async fn get_execution(
        &self,
        tenant_id: &str,
        instance_id: &str,
    ) -> Result<WorkflowInstanceDto, ExecutionError>;

    async fn get_execution_with_metadata(
        &self,
        workflow_id: &str,
        instance_id: &str,
        tenant_id: &str,
    ) -> Result<ExecutionWithMetadata, ExecutionError>;

    async fn list_executions(
        &self,
        tenant_id: &str,
        workflow_id: &str,
        page: Option<i32>,
        size: Option<i32>,
    ) -> Result<PageWorkflowInstanceHistoryDto, ExecutionError>;

    async fn list_all_executions(
        &self,
        tenant_id: &str,
        page: Option<i32>,
        size: Option<i32>,
        filters: ExecutionFilters,
    ) -> Result<PageWorkflowInstanceHistoryDto, ExecutionError>;

    async fn execution_summary(
        &self,
        tenant_id: &str,
        filters: ExecutionFilters,
    ) -> Result<ExecutionSummary, ExecutionError>;

    async fn execution_listing_options(
        &self,
        tenant_id: &str,
        page: i32,
        size: i32,
        filters: &ExecutionFilters,
    ) -> Result<ListInstancesOptions, ExecutionError>;

    /// The run, if it is this workflow's and this tenant's.
    async fn authorize_execution(
        &self,
        workflow_id: &str,
        instance_id: &str,
        tenant_id: &str,
    ) -> Result<InstanceInfo, ExecutionError>;

    // ---- A run's own records ----------------------------------------------

    async fn get_instance_info(
        &self,
        tenant_id: &str,
        instance_id: &str,
    ) -> Result<InstanceInfo, RuntimeError>;

    async fn list_events(
        &self,
        tenant_id: &str,
        instance_id: &str,
        options: Option<ListEventsOptions>,
    ) -> Result<ListEventsResult, RuntimeError>;

    async fn list_step_summaries(
        &self,
        tenant_id: &str,
        instance_id: &str,
        options: Option<ListStepSummariesOptions>,
    ) -> Result<ListStepSummariesResult, RuntimeError>;

    async fn get_scope_ancestors(
        &self,
        tenant_id: &str,
        instance_id: &str,
        scope_id: &str,
    ) -> Result<Vec<ScopeInfo>, RuntimeError>;

    async fn list_checkpoints(
        &self,
        tenant_id: &str,
        instance_id: &str,
        limit: Option<u32>,
        offset: Option<u32>,
    ) -> Result<ListCheckpointsResult, RuntimeError>;

    /// A tenant's runs over time. `options` names the tenant.
    async fn get_tenant_metrics(
        &self,
        options: GetTenantMetricsOptions,
    ) -> Result<TenantMetricsResult, RuntimeError>;

    async fn operation_requests(
        &self,
        tenant_id: &str,
        workflow_id: &str,
        key: &str,
        listing: &ListInstancesOptions,
        fields: &[String],
    ) -> Result<runtara_environment::operations::RequestPage, RuntimeError>;

    // ---- Input a run waits for --------------------------------------------

    async fn list_input_requests(
        &self,
        tenant_id: &str,
        instances: &[String],
        offset: u64,
        limit: u32,
    ) -> InputResult<InputRequestPage>;

    async fn get_input_request(
        &self,
        tenant_id: &str,
        instance_id: &str,
        request: &str,
    ) -> InputResult<InputRequest>;

    async fn submit_input_response(
        &self,
        tenant_id: &str,
        instance_id: &str,
        request: &str,
        operation: &str,
        payload: &Value,
    ) -> InputResult<InputReceipt>;

    /// An answer from a signed-in caller, held to what that caller may answer.
    async fn submit_authenticated_input_response(
        &self,
        auth: &crate::auth::AuthContext,
        instance_id: &str,
        request: &str,
        operation: &str,
        payload: &Value,
    ) -> InputResult<InputReceipt>;

    /// What a run is waiting to be told, as the API shows it.
    async fn pending_input_page(
        &self,
        tenant_id: &str,
        instance_id: &str,
    ) -> InputResult<PendingInputPage>;

    // ---- Actions a workflow offers while it waits ---------------------------

    async fn list_instance_actions(
        &self,
        tenant_id: &str,
        workflow_id: &str,
        instance_id: &str,
    ) -> Result<Vec<WorkflowRuntimeAction>, WorkflowRuntimeError>;

    async fn list_workflow_actions(
        &self,
        tenant_id: &str,
        workflow_id: &str,
        page: Option<i32>,
        size: Option<i32>,
    ) -> Result<WorkflowRuntimeActionPage, WorkflowRuntimeError>;

    async fn submit_workflow_action(
        &self,
        auth: &crate::auth::AuthContext,
        workflow_id: &str,
        instance_id: &str,
        request_id: &str,
        operation_id: &str,
        payload: &Value,
    ) -> Result<WorkflowActionReceipt, WorkflowRuntimeError>;

    async fn submit_authenticated_input(
        &self,
        auth: &crate::auth::AuthContext,
        instance_id: &str,
        request_id: &str,
        operation_id: &str,
        payload: &Value,
    ) -> Result<WorkflowActionReceipt, WorkflowRuntimeError>;

    /// The operation queues a tenant's workflows have work in.
    async fn operation_queues(
        &self,
        tenant_id: &str,
    ) -> Result<Vec<OperationQueue>, WorkflowRuntimeError>;

    // ---- Compilation --------------------------------------------------------

    /// Compiles a version of a workflow and registers what it built, now.
    async fn compile_workflow(
        &self,
        tenant_id: &str,
        workflow_id: &str,
        version: i32,
        force_recompile: bool,
    ) -> Result<CompilationResultDto, CompilationError>;

    /// Compiles a version of a workflow as an agent other workflows may use.
    async fn publish_workflow_agent(
        &self,
        tenant_id: &str,
        workflow_id: &str,
        version: i32,
        slug: String,
    ) -> Result<Value, CompilationError>;

    /// What was built for an image, if the engine has it.
    async fn get_image(
        &self,
        tenant_id: &str,
        image_id: &str,
    ) -> Result<Option<ImageSummary>, RuntimeError>;

    // ---- Parts of the API the engine serves --------------------------------

    /// Answers a request for `part` of the API. The request carries what the
    /// API established about its caller. A part this engine does not serve
    /// is not found.
    async fn serve(
        &self,
        part: Served,
        request: axum::extract::Request,
    ) -> axum::response::Response;
}

/// The engine in this process.
pub struct LocalEngine {
    /// The workflow runtime, if this server has one.
    runtime: Option<Arc<RuntimeClient>>,
    executions: Arc<ExecutionEngine>,
    /// The server database, which the engine reads definitions from.
    pool: sqlx::PgPool,
    /// The agents a compiled workflow may use.
    agent_catalog: Option<Arc<runtara_dsl::agent_meta::AgentCatalog>>,
    /// The parts of the API this engine serves, each a router of its own.
    /// They are built after the engine is, from what the engine holds.
    served: std::sync::RwLock<std::collections::HashMap<Served, axum::Router>>,
}

impl LocalEngine {
    pub fn new(
        runtime: Option<Arc<RuntimeClient>>,
        executions: Arc<ExecutionEngine>,
        pool: sqlx::PgPool,
    ) -> Self {
        Self {
            runtime,
            executions,
            pool,
            agent_catalog: None,
            served: std::sync::RwLock::default(),
        }
    }

    /// Has this engine serve `part` of the API with `router`.
    pub fn serves(&self, part: Served, router: axum::Router) {
        self.served
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(part, router);
    }

    /// With the agents this server loaded, which a compile checks a workflow
    /// against.
    #[must_use]
    pub fn with_agent_catalog(
        mut self,
        agent_catalog: Arc<runtara_dsl::agent_meta::AgentCatalog>,
    ) -> Self {
        self.agent_catalog = Some(agent_catalog);
        self
    }

    fn compiler(&self) -> CompilationService {
        let repository = Arc::new(
            crate::api::repositories::workflows::WorkflowRepository::new(self.pool.clone()),
        );
        let service = CompilationService::new(repository, self.runtime.clone())
            .with_direct_compilation(
                crate::api::services::compilation::direct_compilation_settings_from_config(),
            );
        match &self.agent_catalog {
            Some(agent_catalog) => service.with_agent_catalog(agent_catalog.clone()),
            None => service,
        }
    }

    fn actions(&self) -> Result<&RuntimeClient, WorkflowRuntimeError> {
        self.runtime
            .as_deref()
            .ok_or(WorkflowRuntimeError::RuntimeUnavailable)
    }

    fn runtime(&self) -> Result<&RuntimeClient, RuntimeError> {
        self.runtime.as_deref().ok_or_else(|| {
            RuntimeError::ConnectionFailed("the workflow runtime is not available".to_owned())
        })
    }

    fn inputs(&self) -> InputResult<&RuntimeClient> {
        self.runtime
            .as_deref()
            .ok_or_else(|| InputError::Storage("the workflow runtime is not available".to_owned()))
    }
}

#[async_trait]
impl Engine for LocalEngine {
    fn available(&self) -> bool {
        self.runtime.is_some()
    }

    async fn queue(&self, request: QueueRequest<'_>) -> Result<QueuedExecution, ExecutionError> {
        self.executions.queue(request).await
    }

    async fn run_sync(&self, request: SyncRequest<'_>) -> Result<SyncExecution, ExecutionError> {
        self.executions.run_sync(request).await
    }

    async fn enqueue_trigger_event(
        &self,
        tenant_id: &str,
        event: TriggerEvent,
        idempotency_key: String,
    ) -> Result<EnqueuedExecution, ExecutionError> {
        self.executions
            .enqueue_trigger_event(tenant_id, event, idempotency_key)
            .await
    }

    async fn replay(
        &self,
        tenant_id: &str,
        original_instance_id: &str,
    ) -> Result<QueuedExecution, ExecutionError> {
        self.executions
            .replay(tenant_id, original_instance_id)
            .await
    }

    async fn stop(
        &self,
        tenant_id: &str,
        instance_id: &str,
    ) -> Result<StopOutcome, ExecutionError> {
        self.executions.stop(tenant_id, instance_id).await
    }

    async fn pause(
        &self,
        tenant_id: &str,
        instance_id: &str,
    ) -> Result<PauseOutcome, ExecutionError> {
        self.executions.pause(tenant_id, instance_id).await
    }

    async fn resume(
        &self,
        tenant_id: &str,
        instance_id: &str,
    ) -> Result<ResumeOutcome, ExecutionError> {
        self.executions.resume(tenant_id, instance_id).await
    }

    async fn get_execution(
        &self,
        tenant_id: &str,
        instance_id: &str,
    ) -> Result<WorkflowInstanceDto, ExecutionError> {
        self.executions.get_execution(tenant_id, instance_id).await
    }

    async fn get_execution_with_metadata(
        &self,
        workflow_id: &str,
        instance_id: &str,
        tenant_id: &str,
    ) -> Result<ExecutionWithMetadata, ExecutionError> {
        self.executions
            .get_execution_with_metadata(workflow_id, instance_id, tenant_id)
            .await
    }

    async fn list_executions(
        &self,
        tenant_id: &str,
        workflow_id: &str,
        page: Option<i32>,
        size: Option<i32>,
    ) -> Result<PageWorkflowInstanceHistoryDto, ExecutionError> {
        self.executions
            .list_executions(tenant_id, workflow_id, page, size)
            .await
    }

    async fn list_all_executions(
        &self,
        tenant_id: &str,
        page: Option<i32>,
        size: Option<i32>,
        filters: ExecutionFilters,
    ) -> Result<PageWorkflowInstanceHistoryDto, ExecutionError> {
        self.executions
            .list_all_executions(tenant_id, page, size, filters)
            .await
    }

    async fn execution_summary(
        &self,
        tenant_id: &str,
        filters: ExecutionFilters,
    ) -> Result<ExecutionSummary, ExecutionError> {
        self.executions.execution_summary(tenant_id, filters).await
    }

    async fn execution_listing_options(
        &self,
        tenant_id: &str,
        page: i32,
        size: i32,
        filters: &ExecutionFilters,
    ) -> Result<ListInstancesOptions, ExecutionError> {
        self.executions
            .execution_listing_options(tenant_id, page, size, filters)
            .await
    }

    async fn authorize_execution(
        &self,
        workflow_id: &str,
        instance_id: &str,
        tenant_id: &str,
    ) -> Result<InstanceInfo, ExecutionError> {
        self.executions
            .authorize_execution(workflow_id, instance_id, tenant_id)
            .await
    }

    // A run's own records are read by its id alone, so it is the tenant's
    // scope that keeps another tenant's run from being found. The caller
    // names the tenant; the scope is entered here, on the engine's side.

    async fn get_instance_info(
        &self,
        tenant_id: &str,
        instance_id: &str,
    ) -> Result<InstanceInfo, RuntimeError> {
        let runtime = self.runtime()?;
        scoped(tenant_id, runtime.get_instance_info(instance_id)).await
    }

    async fn list_events(
        &self,
        tenant_id: &str,
        instance_id: &str,
        options: Option<ListEventsOptions>,
    ) -> Result<ListEventsResult, RuntimeError> {
        let runtime = self.runtime()?;
        scoped(tenant_id, runtime.list_events(instance_id, options)).await
    }

    async fn list_step_summaries(
        &self,
        tenant_id: &str,
        instance_id: &str,
        options: Option<ListStepSummariesOptions>,
    ) -> Result<ListStepSummariesResult, RuntimeError> {
        let runtime = self.runtime()?;
        scoped(tenant_id, runtime.list_step_summaries(instance_id, options)).await
    }

    async fn get_scope_ancestors(
        &self,
        tenant_id: &str,
        instance_id: &str,
        scope_id: &str,
    ) -> Result<Vec<ScopeInfo>, RuntimeError> {
        let runtime = self.runtime()?;
        scoped(
            tenant_id,
            runtime.get_scope_ancestors(instance_id, scope_id),
        )
        .await
    }

    async fn list_checkpoints(
        &self,
        tenant_id: &str,
        instance_id: &str,
        limit: Option<u32>,
        offset: Option<u32>,
    ) -> Result<ListCheckpointsResult, RuntimeError> {
        let runtime = self.runtime()?;
        scoped(
            tenant_id,
            runtime.list_checkpoints(instance_id, limit, offset),
        )
        .await
    }

    async fn get_tenant_metrics(
        &self,
        options: GetTenantMetricsOptions,
    ) -> Result<TenantMetricsResult, RuntimeError> {
        self.runtime()?.get_tenant_metrics(options).await
    }

    async fn operation_requests(
        &self,
        tenant_id: &str,
        workflow_id: &str,
        key: &str,
        listing: &ListInstancesOptions,
        fields: &[String],
    ) -> Result<runtara_environment::operations::RequestPage, RuntimeError> {
        self.runtime()?
            .operation_requests(tenant_id, workflow_id, key, listing, fields)
            .await
    }

    async fn list_input_requests(
        &self,
        tenant_id: &str,
        instances: &[String],
        offset: u64,
        limit: u32,
    ) -> InputResult<InputRequestPage> {
        self.inputs()?
            .list_input_requests(tenant_id, instances, offset, limit)
            .await
    }

    async fn get_input_request(
        &self,
        tenant_id: &str,
        instance_id: &str,
        request: &str,
    ) -> InputResult<InputRequest> {
        self.inputs()?
            .get_input_request(tenant_id, instance_id, request)
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
        self.inputs()?
            .submit_input_response(tenant_id, instance_id, request, operation, payload)
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
        self.inputs()?
            .submit_authenticated_input_response(auth, instance_id, request, operation, payload)
            .await
    }

    async fn pending_input_page(
        &self,
        tenant_id: &str,
        instance_id: &str,
    ) -> InputResult<PendingInputPage> {
        crate::api::services::pending_inputs::pending_input_page(
            self.inputs()?,
            tenant_id,
            instance_id,
        )
        .await
    }

    async fn list_instance_actions(
        &self,
        tenant_id: &str,
        workflow_id: &str,
        instance_id: &str,
    ) -> Result<Vec<WorkflowRuntimeAction>, WorkflowRuntimeError> {
        crate::api::services::workflow_runtime::list_instance_actions(
            self.actions()?,
            tenant_id,
            workflow_id,
            instance_id,
        )
        .await
    }

    async fn list_workflow_actions(
        &self,
        tenant_id: &str,
        workflow_id: &str,
        page: Option<i32>,
        size: Option<i32>,
    ) -> Result<WorkflowRuntimeActionPage, WorkflowRuntimeError> {
        crate::api::services::workflow_runtime::list_workflow_actions(
            &self.executions,
            self.actions()?,
            tenant_id,
            workflow_id,
            page,
            size,
        )
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
        crate::api::services::workflow_runtime::submit_workflow_action(
            &self.executions,
            self.actions()?,
            &self.pool,
            auth,
            workflow_id,
            instance_id,
            request_id,
            operation_id,
            payload,
        )
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
        crate::api::services::workflow_runtime::submit_authenticated_input(
            self.actions()?,
            &self.pool,
            auth,
            instance_id,
            request_id,
            operation_id,
            payload,
        )
        .await
    }

    async fn operation_queues(
        &self,
        tenant_id: &str,
    ) -> Result<Vec<OperationQueue>, WorkflowRuntimeError> {
        crate::api::services::operations::discover_queues(
            &crate::api::repositories::operations::OperationsRepository::new(self.pool.clone()),
            self.actions()?,
            tenant_id,
        )
        .await
    }

    async fn compile_workflow(
        &self,
        tenant_id: &str,
        workflow_id: &str,
        version: i32,
        force_recompile: bool,
    ) -> Result<CompilationResultDto, CompilationError> {
        self.compiler()
            .compile_workflow(tenant_id, workflow_id, version, force_recompile)
            .await
    }

    async fn publish_workflow_agent(
        &self,
        tenant_id: &str,
        workflow_id: &str,
        version: i32,
        slug: String,
    ) -> Result<Value, CompilationError> {
        self.compiler()
            .publish_workflow_agent(tenant_id, workflow_id, version, slug)
            .await
    }

    async fn get_image(
        &self,
        tenant_id: &str,
        image_id: &str,
    ) -> Result<Option<ImageSummary>, RuntimeError> {
        self.runtime()?.get_image(image_id, tenant_id).await
    }

    async fn serve(
        &self,
        part: Served,
        request: axum::extract::Request,
    ) -> axum::response::Response {
        use axum::response::IntoResponse;
        use tower::ServiceExt;
        let router = self
            .served
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&part)
            .cloned();
        // The engine's router routes the request as it arrived: by its whole
        // path, and with no trace of how the API routed it here, which its
        // own layers and handlers would otherwise take for their own.
        let mut request = request;
        if let Some(axum::extract::OriginalUri(uri)) = request
            .extensions_mut()
            .remove::<axum::extract::OriginalUri>()
        {
            *request.uri_mut() = uri;
        }
        request
            .extensions_mut()
            .remove::<axum::extract::MatchedPath>();
        request
            .extensions_mut()
            .remove::<axum::extract::NestedPath>();
        match router {
            Some(router) => match router.oneshot(request).await {
                Ok(response) => response,
                Err(never) => match never {},
            },
            None => axum::http::StatusCode::NOT_FOUND.into_response(),
        }
    }
}

/// The API's side of a part the engine serves, to be nested under the part's
/// path: every request under it goes to the engine, and the engine's answer
/// goes back. Authentication and the other request layers are put around this
/// router, as they were around the routes it stands for.
///
/// It is a fallback and not a route, so that it adds no path parameter of its
/// own to the request it hands over.
pub fn forwarded(engine: Arc<dyn Engine>, part: Served) -> axum::Router {
    axum::Router::new().fallback(move |request: axum::extract::Request| {
        let engine = engine.clone();
        async move { engine.serve(part, request).await }
    })
}

/// An engine with no runtime, for a test of code that holds one and asks it
/// nothing a runtime answers.
#[cfg(test)]
pub(crate) fn without_runtime() -> Arc<dyn Engine> {
    local_without_runtime()
}

#[cfg(test)]
fn local_without_runtime() -> Arc<LocalEngine> {
    let pool = sqlx::postgres::PgPoolOptions::new()
        .connect_lazy("postgres://unused/unused")
        .expect("a lazy pool connects to nothing");
    let (events, _dropped) = tokio::sync::mpsc::channel(1);
    let executions = Arc::new(ExecutionEngine::new(
        pool.clone(),
        Arc::new(crate::api::repositories::workflows::WorkflowRepository::new(pool.clone())),
        None,
        None,
        crate::product_events::ProductEventSink::new(events),
    ));
    Arc::new(LocalEngine::new(None, executions, pool))
}

#[cfg(test)]
mod boundary {
    //! Code that takes requests reaches the engine through [`super::Engine`]
    //! and nothing else. The server's state does not hand out the runtime
    //! client or the execution engine, so a handler cannot be given them; this
    //! keeps one from naming them some other way.

    use std::path::{Path, PathBuf};

    /// The engine's own types, which only the engine's side may name.
    const ENGINE_ONLY: &[&str] = &["RuntimeClient", "ExecutionEngine", "EnvironmentClient"];

    fn sources(dir: &Path, found: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).expect("a source directory") {
            let path = entry.expect("a directory entry").path();
            if path.is_dir() {
                sources(&path, found);
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                found.push(path);
            }
        }
    }

    #[test]
    fn code_that_takes_requests_names_none_of_the_engines_own_types() {
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = Vec::new();
        for dir in ["api/handlers", "mcp"] {
            sources(&src.join(dir), &mut files);
        }
        assert!(files.len() > 20, "the handlers and MCP tools are read");
        let mut named = Vec::new();
        for file in files {
            let name = file.to_string_lossy().into_owned();
            if name.ends_with("_tests.rs") || name.ends_with("/tests.rs") {
                continue;
            }
            let text = std::fs::read_to_string(&file).expect("a source file");
            // A file's own tests may build an engine to test against.
            let code = text.split("#[cfg(test)]").next().unwrap_or_default();
            for (number, line) in code.lines().enumerate() {
                if line.trim_start().starts_with("//") {
                    continue;
                }
                for forbidden in ENGINE_ONLY {
                    if line.contains(forbidden) {
                        named.push(format!("{name}:{}: {forbidden}", number + 1));
                    }
                }
            }
        }
        assert!(
            named.is_empty(),
            "code that takes requests asks the engine through `Engine`, not:\n{}",
            named.join("\n")
        );
    }
}

#[cfg(test)]
mod serving {
    use super::*;
    use axum::body::Body;
    use axum::extract::{MatchedPath, Path};
    use axum::http::{Request, StatusCode};
    use axum::routing::get;
    use tower::ServiceExt;

    async fn text(response: axum::response::Response) -> (StatusCode, String) {
        let status = response.status();
        let body = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap();
        (status, String::from_utf8(body.to_vec()).unwrap())
    }

    /// The API's router: everything under `/api/runtime` is the engine's.
    fn api(engine: Arc<dyn Engine>) -> axum::Router {
        axum::Router::new()
            .route("/api/runtime/own", get(|| async { "the API's own route" }))
            .nest("/api/runtime", forwarded(engine, Served::Connections))
    }

    #[tokio::test]
    async fn the_engine_routes_a_forwarded_request_as_it_arrived() {
        let engine = local_without_runtime();
        // What the engine's handler sees of the request: its own path
        // parameter and no other, and the route its own router matched, which
        // is what a layer that decides by route goes by.
        engine.serves(
            Served::Connections,
            axum::Router::new().nest(
                "/api/runtime",
                axum::Router::new().route(
                    "/things/{id}",
                    get(|Path(id): Path<String>, matched: MatchedPath| async move {
                        format!("{id} at {}", matched.as_str())
                    }),
                ),
            ),
        );
        let api = api(engine);

        let forwarded = api
            .clone()
            .oneshot(
                Request::get("/api/runtime/things/7")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            text(forwarded).await,
            (StatusCode::OK, "7 at /api/runtime/things/{id}".to_owned())
        );

        // What the engine does not have is not found there, by the engine.
        let unknown = api
            .clone()
            .oneshot(
                Request::get("/api/runtime/nothing")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(unknown.status(), StatusCode::NOT_FOUND);

        // The API's own routes are still its own.
        let own = api
            .oneshot(
                Request::get("/api/runtime/own")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            text(own).await,
            (StatusCode::OK, "the API's own route".to_owned())
        );
    }

    #[tokio::test]
    async fn a_part_the_engine_does_not_serve_is_not_found() {
        let engine: Arc<dyn Engine> = local_without_runtime();
        let response = engine
            .serve(
                Served::Channels,
                Request::post("/api/runtime/events/webhook/slack/c-1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
}
