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
use crate::api::services::pending_inputs::PendingInputPage;
use crate::api::services::workflow_runtime::{
    WorkflowActionReceipt, WorkflowRuntimeAction, WorkflowRuntimeActionPage, WorkflowRuntimeError,
};
use crate::runtime_client::{
    GetTenantMetricsOptions, InstanceInfo, RuntimeClient, RuntimeError, TenantMetricsResult,
};
use crate::runtime_types::{
    ListCheckpointsResult, ListEventsOptions, ListEventsResult, ListInstancesOptions,
    ListStepSummariesOptions, ListStepSummariesResult, ScopeInfo,
};
use crate::workers::execution_engine::{
    ExecutionEngine, ExecutionError, PauseOutcome, QueueRequest, QueuedExecution, ResumeOutcome,
    StopOutcome, SyncExecution, SyncRequest,
};
use crate::workers::execution_outbox::EnqueuedExecution;
use crate::workers::runtara_dto::ExecutionWithMetadata;

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
}

/// The engine in this process.
pub struct LocalEngine {
    /// The workflow runtime, if this server has one.
    runtime: Option<Arc<RuntimeClient>>,
    executions: Arc<ExecutionEngine>,
    /// The server database, which the engine reads definitions from.
    pool: sqlx::PgPool,
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
}
