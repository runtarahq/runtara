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

pub mod approvals;
pub mod listener;
pub mod remote;
mod wire;

use std::future::Future;
use std::sync::Arc;

use async_trait::async_trait;
use runtara_core::persistence::inputs::{
    InputError, InputReceipt, InputRequest, InputRequestPage, InputResult,
};
use runtara_store_postgres::tenant_scope::scoped;
use serde_json::Value;

use crate::api::dto::agent_testing::TestEngine;
use crate::api::dto::executions::{ExecutionFilters, ExecutionSummary};
use crate::api::dto::operations::OperationQueue;
use crate::api::dto::trigger_event::TriggerEvent;
use crate::api::dto::triggers::InvocationTrigger;
use crate::api::dto::workflows::{PageWorkflowInstanceHistoryDto, WorkflowInstanceDto};
use crate::api::services::agent_testing::{
    AgentTestingService, ServiceError as AgentTestError, TestResult,
};
use crate::api::services::compilation::{
    CompilationResultDto, CompilationService, ServiceError as CompilationError,
};
use crate::api::services::pending_inputs::PendingInputPage;
use crate::api::services::workflow_runtime::{
    WorkflowActionReceipt, WorkflowRuntimeAction, WorkflowRuntimeActionPage, WorkflowRuntimeError,
};
use crate::api::utils::connection_validation::ConnectionRef;
use crate::product_events::ProductEvent;
use crate::runtime_client::{
    GetTenantMetricsOptions, InstanceInfo, RuntimeClient, RuntimeError, TenantMetricsResult,
};
use crate::runtime_types::{
    ImageSummary, ListCheckpointsResult, ListEventsOptions, ListEventsResult, ListInstancesOptions,
    ListStepSummariesOptions, ListStepSummariesResult, ScopeInfo,
};
use crate::valkey::compilation_progress::CompilationProgress;
use crate::valkey::compilation_queue::CompilationQueueError;
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
    /// Agents: what there are, what each can do, and running one on request.
    Agents,
    /// Triggers: registering one may register a webhook at its provider.
    Triggers,
    /// A trigger's HTTP events, each checked against its connection, from
    /// whoever sends them.
    Events,
    /// The same, asked by a caller the API authenticated, whose role is held
    /// to what it may run.
    EventsForCallers,
}

impl Served {
    /// Every part.
    pub(crate) const ALL: &'static [Served] = &[
        Served::Connections,
        Served::OAuthCallbacks,
        Served::ObjectModel,
        Served::Channels,
        Served::Agents,
        Served::Triggers,
        Served::Events,
        Served::EventsForCallers,
    ];

    /// The part's name, as it is written between two processes.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Served::Connections => "connections",
            Served::OAuthCallbacks => "o-auth-callbacks",
            Served::ObjectModel => "object-model",
            Served::Channels => "channels",
            Served::Agents => "agents",
            Served::Triggers => "triggers",
            Served::Events => "events",
            Served::EventsForCallers => "events-for-callers",
        }
    }

    /// Whether the part is served to a caller the API authenticated. The
    /// others are sent by anyone, and say whose they are themselves: a
    /// provider's redirect, a webhook.
    pub(crate) fn has_a_caller(self) -> bool {
        match self {
            Served::Connections
            | Served::ObjectModel
            | Served::Agents
            | Served::Triggers
            | Served::EventsForCallers => true,
            Served::OAuthCallbacks | Served::Channels | Served::Events => false,
        }
    }

    /// The part of that name.
    pub(crate) fn named(name: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|part| part.name() == name)
    }
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

    // ---- Workflows published as agents -----------------------------------

    /// The agents a tenant published from its workflows, which its other
    /// workflows may use beside the built-in ones.
    async fn workflow_agents(&self, tenant_id: &str) -> Vec<runtara_dsl::agent_meta::AgentInfo>;

    /// Takes a published workflow-agent out of use, when its workflow is
    /// deleted.
    async fn unstage_workflow_agent(&self, tenant_id: &str, slug: &str) -> Result<(), String>;

    /// The API key whose hash is `key_hash`, whichever tenant's it is, if it
    /// is still good. The key is all a caller presented, so its row is looked
    /// for across tenants, which only the engine may do.
    async fn api_key_by_hash(
        &self,
        key_hash: &str,
    ) -> Result<crate::api::handlers::api_keys::ApiKey, String>;

    /// Makes `version` the one that runs when a run names none, on the
    /// engine's own record. `Ok(false)` if the workflow has no such version.
    async fn set_current_version(
        &self,
        tenant_id: &str,
        workflow_id: &str,
        version: i32,
    ) -> Result<bool, String>;

    /// Forgets what a version was compiled to, so that it is compiled again
    /// before it next runs: its definition was changed, or a rebuild was
    /// asked for. The record of compilations is the engine's.
    async fn invalidate_compilation(
        &self,
        tenant_id: &str,
        workflow_id: &str,
        version: i32,
    ) -> Result<(), String>;

    // ---- Connections, as a workflow names them ----------------------------

    /// The connections a tenant has: what a workflow's steps may name. Their
    /// ids, kinds and titles, and nothing of what they hold.
    async fn tenant_connections(&self, tenant_id: &str) -> Result<Vec<ConnectionRef>, String>;

    /// Takes down, at their providers, the webhooks of chat channel triggers
    /// that were removed. Best effort: what fails is logged.
    async fn reconcile_channel_webhooks(&self, tenant_id: &str, removed: &[InvocationTrigger]);

    // ---- Compilation, through the queue -------------------------------------

    /// Whether compiles are queued for a worker, and not done as they are
    /// asked for.
    fn queues_compilations(&self) -> bool;

    /// Queues a compile of a version. `false` if one was already queued.
    /// `compiled` is the event to record when it is done, for a compile a
    /// caller asked for.
    async fn enqueue_compilation(
        &self,
        tenant_id: &str,
        workflow_id: &str,
        version: i32,
        force_recompile: bool,
        compiled: Option<ProductEvent>,
    ) -> Result<bool, CompilationQueueError>;

    /// Waits for a queued compile to be done. `false` if it is not in time.
    async fn wait_for_compilation(
        &self,
        tenant_id: &str,
        workflow_id: &str,
        version: i32,
        timeout: std::time::Duration,
    ) -> Result<bool, CompilationQueueError>;

    async fn is_compilation_pending(
        &self,
        tenant_id: &str,
        workflow_id: &str,
        version: i32,
    ) -> Result<bool, CompilationQueueError>;

    /// Says a compile is queued, for whoever watches its progress.
    async fn mark_compilation_queued(&self, tenant_id: &str, workflow_id: &str, version: i32);

    /// How far a compile has got, while it is queued or under way.
    async fn compilation_progress(
        &self,
        tenant_id: &str,
        workflow_id: &str,
        version: i32,
    ) -> Option<CompilationProgress>;

    // ---- Running an agent on request ---------------------------------------

    /// Whether this engine runs an agent's capability on request at all.
    fn tests_agents(&self) -> bool;

    /// Runs one capability of an agent with the given input, with a
    /// connection's credentials if it names one.
    async fn test_agent(
        &self,
        tenant_id: &str,
        agent_name: &str,
        capability_id: &str,
        input: Value,
        connection_id: Option<String>,
        engine: TestEngine,
    ) -> Result<TestResult, AgentTestError>;

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
    /// Runs an agent's capability on request, if this server allows it.
    agent_testing: Option<AgentTestingService>,
    /// Connections and their sealed secrets.
    connections: Option<Arc<runtara_connections::ConnectionsFacade>>,
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
            agent_testing: None,
            connections: None,
            served: std::sync::RwLock::default(),
        }
    }

    /// With the tenants' connections.
    #[must_use]
    pub fn with_connections(
        mut self,
        connections: Arc<runtara_connections::ConnectionsFacade>,
    ) -> Self {
        self.connections = Some(connections);
        self
    }

    /// Where compiles are queued, if they are: Valkey, as the worker that
    /// takes them from the queue reads it.
    fn compile_queue(&self) -> Option<String> {
        crate::valkey::ValkeyConfig::from_env().map(|config| config.connection_url())
    }

    fn queued_compiles(&self) -> Result<String, CompilationQueueError> {
        self.compile_queue().ok_or_else(|| {
            CompilationQueueError::ConnectionError(
                "compiles are not queued on this server".to_owned(),
            )
        })
    }

    /// With the service that runs an agent's capability on request.
    #[must_use]
    pub fn with_agent_testing(mut self, agent_testing: Option<AgentTestingService>) -> Self {
        self.agent_testing = agent_testing;
        self
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

    /// An engine with no workflow runtime, as a server started without a
    /// runtime database has: it answers for what needs none (a tenant's
    /// connections, the compile queue), and refuses what needs a run.
    pub fn without_runtime(pool: sqlx::PgPool) -> Self {
        let (events, _dropped) = tokio::sync::mpsc::channel(1);
        let executions = Arc::new(ExecutionEngine::new(
            pool.clone(),
            Arc::new(crate::api::repositories::workflows::WorkflowRepository::new(pool.clone())),
            None,
            None,
            crate::product_events::ProductEventSink::new(events),
        ));
        Self::new(None, executions, pool)
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
        approvals::approve_as_asked(tenant_id, workflow_id, version)
            .await
            .map_err(CompilationError::DatabaseError)?;
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
        approvals::approve_as_asked(tenant_id, workflow_id, version)
            .await
            .map_err(CompilationError::DatabaseError)?;
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

    async fn workflow_agents(&self, tenant_id: &str) -> Vec<runtara_dsl::agent_meta::AgentInfo> {
        crate::workflow_agents::staged_tenant_agents(tenant_id)
    }

    async fn unstage_workflow_agent(&self, tenant_id: &str, slug: &str) -> Result<(), String> {
        crate::workflow_agents::unstage(tenant_id, slug)
            .await
            .map_err(|error| error.to_string())
    }

    async fn api_key_by_hash(
        &self,
        key_hash: &str,
    ) -> Result<crate::api::handlers::api_keys::ApiKey, String> {
        // Within a tenant already named, only that tenant's keys are found.
        runtara_store_postgres::tenant_scope::locating(
            crate::api::handlers::api_keys::validate_api_key_by_hash(&self.pool, key_hash),
        )
        .await
    }

    async fn set_current_version(
        &self,
        tenant_id: &str,
        workflow_id: &str,
        version: i32,
    ) -> Result<bool, String> {
        let repository =
            crate::api::repositories::workflows::WorkflowRepository::new(self.pool.clone());
        // The engine's record first: it is the one a run goes by. The server
        // database's is what the API shows.
        match repository
            .version_exists(tenant_id, workflow_id, version)
            .await
        {
            Ok(true) => {}
            Ok(false) => return Ok(false),
            Err(error) => return Err(error.to_string()),
        }
        if let Some(approvals) = approvals::installed() {
            let by = approvals::asker().unwrap_or_else(|| "unknown".to_owned());
            approvals
                .set_current(tenant_id, workflow_id, version, &by)
                .await?;
        }
        match repository
            .set_current_version(tenant_id, workflow_id, version)
            .await
        {
            Ok(()) => Ok(true),
            Err(sqlx::Error::RowNotFound) => Ok(false),
            Err(error) => Err(error.to_string()),
        }
    }

    async fn invalidate_compilation(
        &self,
        tenant_id: &str,
        workflow_id: &str,
        version: i32,
    ) -> Result<(), String> {
        crate::api::repositories::workflows::WorkflowRepository::new(self.pool.clone())
            .invalidate_compilation(tenant_id, workflow_id, version)
            .await
            .map_err(|error| error.to_string())
    }

    async fn tenant_connections(&self, tenant_id: &str) -> Result<Vec<ConnectionRef>, String> {
        let Some(connections) = &self.connections else {
            return Err("this engine holds no connections".to_owned());
        };
        Ok(connections
            .list_connections(tenant_id, None, None)
            .await
            .map_err(|error| error.to_string())?
            .into_iter()
            .map(|connection| ConnectionRef {
                id: connection.id,
                integration_id: connection.integration_id,
                title: connection.title,
            })
            .collect())
    }

    async fn reconcile_channel_webhooks(&self, tenant_id: &str, removed: &[InvocationTrigger]) {
        let Some(connections) = &self.connections else {
            return;
        };
        crate::api::services::webhook_manager::reconcile_channel_webhooks(
            &self.pool,
            &crate::api::services::webhook_manager::WebhookManager::new(connections.clone()),
            removed,
            tenant_id,
        )
        .await;
    }

    fn queues_compilations(&self) -> bool {
        self.compile_queue().is_some()
    }

    async fn enqueue_compilation(
        &self,
        tenant_id: &str,
        workflow_id: &str,
        version: i32,
        force_recompile: bool,
        compiled: Option<ProductEvent>,
    ) -> Result<bool, CompilationQueueError> {
        let queue = self.queued_compiles()?;
        approvals::approve_as_asked(tenant_id, workflow_id, version)
            .await
            .map_err(CompilationQueueError::ConnectionError)?;
        match compiled {
            Some(compiled) => {
                crate::workers::compilation_worker::enqueue_compilation_with_event(
                    &queue,
                    tenant_id,
                    workflow_id,
                    version,
                    force_recompile,
                    compiled,
                )
                .await
            }
            None => {
                crate::workers::compilation_worker::enqueue_compilation(
                    &queue,
                    tenant_id,
                    workflow_id,
                    version,
                    force_recompile,
                )
                .await
            }
        }
    }

    async fn wait_for_compilation(
        &self,
        tenant_id: &str,
        workflow_id: &str,
        version: i32,
        timeout: std::time::Duration,
    ) -> Result<bool, CompilationQueueError> {
        crate::workers::compilation_worker::wait_for_compilation(
            &self.queued_compiles()?,
            tenant_id,
            workflow_id,
            version,
            timeout,
        )
        .await
    }

    async fn is_compilation_pending(
        &self,
        tenant_id: &str,
        workflow_id: &str,
        version: i32,
    ) -> Result<bool, CompilationQueueError> {
        crate::workers::compilation_worker::is_compilation_pending(
            &self.queued_compiles()?,
            tenant_id,
            workflow_id,
            version,
        )
        .await
    }

    async fn mark_compilation_queued(&self, tenant_id: &str, workflow_id: &str, version: i32) {
        let Some(queue) = self.compile_queue() else {
            return;
        };
        if let Ok(manager) = crate::valkey::get_or_create_manager(&queue).await {
            crate::valkey::compilation_progress::mark_queued(
                &manager,
                tenant_id,
                workflow_id,
                version,
            )
            .await;
        }
    }

    async fn compilation_progress(
        &self,
        tenant_id: &str,
        workflow_id: &str,
        version: i32,
    ) -> Option<CompilationProgress> {
        let manager = crate::valkey::get_or_create_manager(&self.compile_queue()?)
            .await
            .ok()?;
        crate::valkey::compilation_progress::read_progress(
            &manager,
            tenant_id,
            workflow_id,
            version,
        )
        .await
    }

    fn tests_agents(&self) -> bool {
        self.agent_testing.is_some()
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
        let Some(service) = &self.agent_testing else {
            return Err(AgentTestError::NotEnabled);
        };
        service
            .test_agent(
                tenant_id,
                agent_name,
                capability_id,
                input,
                connection_id,
                engine,
            )
            .await
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
        // The request is answered in the scope of the tenant it is for, as
        // every other call is: the tenant the API authenticated its caller
        // for, or the one its path named. The scope is entered here, on the
        // engine's side, and not taken from whatever task handed it over. A
        // request that names neither (a webhook known only by an id) has its
        // tenant found by the part that serves it.
        let tenant = request
            .extensions()
            .get::<crate::auth::AuthContext>()
            .map(|caller| caller.org_id.clone())
            .or_else(|| {
                request
                    .extensions()
                    .get::<crate::middleware::tenant_path::RequestedTenant>()
                    .map(|named| named.0.clone())
            });
        let request_asker = request
            .extensions()
            .get::<crate::auth::AuthContext>()
            .map(|caller| caller.user_id.clone());
        let Some(router) = router else {
            return axum::http::StatusCode::NOT_FOUND.into_response();
        };
        let answered = async move {
            match router.oneshot(request).await {
                Ok(response) => response,
                Err(never) => match never {},
            }
        };
        // What a caller asks of a part the engine serves, it asks as itself.
        let asker = request_asker;
        let answered = async move {
            match asker {
                Some(asker) => approvals::asked_by(asker, answered).await,
                None => answered.await,
            }
        };
        match tenant {
            Some(tenant) => scoped(tenant, answered).await,
            None => answered.await,
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
    Arc::new(LocalEngine::without_runtime(pool))
}

#[cfg(test)]
mod boundary {
    //! Code that takes requests reaches the engine through [`super::Engine`]
    //! and nothing else. The server's state does not hand out the runtime
    //! client or the execution engine, so a handler cannot be given them; this
    //! keeps one from naming them some other way.

    use std::path::{Path, PathBuf};

    /// What only the engine's side may name: the engine's own types, and what
    /// reaches a connection's credentials or the engine's queues.
    const ENGINE_ONLY: &[&str] = &[
        "RuntimeClient",
        "ExecutionEngine",
        "EnvironmentClient",
        "ConnectionsFacade",
        "SealedCredentials",
        "WebhookManager",
        "webhook_verification::",
        "AgentTestingService",
        "AgentsService",
        "ComponentDispatcher",
        "compilation_worker::",
        "compilation_queue::",
        "compilation_progress::",
    ];

    /// Handlers of the parts of the API the engine serves ([`super::Served`]):
    /// they live beside the API's handlers and run on the engine's side.
    const SERVED_BY_THE_ENGINE: &[&str] = &[
        "api/handlers/events.rs",
        "api/handlers/workflows_sync.rs",
        "api/handlers/triggers.rs",
        "api/handlers/object_model.rs",
        "api/handlers/csv_import_export.rs",
        "api/handlers/operators.rs",
        "api/handlers/agent_testing.rs",
    ];

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
            if name.ends_with("_tests.rs")
                || name.ends_with("/tests.rs")
                || SERVED_BY_THE_ENGINE
                    .iter()
                    .any(|served| name.ends_with(served))
            {
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

    /// The engine answers a forwarded request in the scope of the tenant the
    /// API authenticated its caller for, entered on the engine's side.
    #[tokio::test]
    async fn a_forwarded_request_is_answered_in_its_callers_tenant() {
        let engine = local_without_runtime();
        engine.serves(
            Served::Triggers,
            axum::Router::new().route(
                "/scope",
                get(|| async {
                    runtara_store_postgres::tenant_scope::current()
                        .unwrap_or_else(|| "no tenant".to_owned())
                }),
            ),
        );
        let engine: Arc<dyn Engine> = engine;
        let ask = |caller: Option<&str>| {
            let mut request = Request::get("/scope").body(Body::empty()).unwrap();
            if let Some(tenant) = caller {
                request
                    .extensions_mut()
                    .insert(crate::auth::AuthContext::new(
                        tenant.to_owned(),
                        "someone".to_owned(),
                        crate::auth::AuthMethod::Unauthenticated,
                    ));
            }
            let engine = engine.clone();
            async move { text(engine.serve(Served::Triggers, request).await).await.1 }
        };
        // Asked from a task in no scope at all, as a request from another
        // process would be.
        assert_eq!(ask(Some("acme")).await, "acme");
        assert_eq!(ask(None).await, "no tenant");
        // And not the scope of whoever handed it over.
        assert_eq!(scoped("other", ask(Some("acme"))).await, "acme");
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
