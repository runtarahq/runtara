// Copyright (C) 2026 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! What crosses between the API and the engine when they are two processes:
//! every operation of [`Engine`] as a named message with its answer.
//!
//! Each operation is written once here. Its name is its path, its fields are
//! its JSON body, and its answer is what the engine's method returns, written
//! as JSON. The engine answers one in the scope of the tenant it names.

use std::future::Future;

use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;
use uuid::Uuid;

use super::*;
use crate::auth::AuthContext;
use crate::workers::execution_engine::TriggerSource;

/// The path of an operation is this, then its name.
pub(crate) const OPERATIONS: &str = "/engine/v1/operations/";
/// The path of a request to a part of the API the engine serves is this, the
/// part's name, then the request's own path.
pub(crate) const SERVED: &str = "/engine/v1/served/";

/// The caller as the API authenticated it, on a request it hands over.
pub(crate) const CALLER: &str = "x-runtara-caller";
/// The tenant a request's path named, on a request it hands over.
pub(crate) const REQUESTED_TENANT: &str = "x-runtara-requested-tenant";
/// What a request came through (the API, MCP), on a request it hands over.
pub(crate) const EVENT_SOURCE: &str = "x-runtara-event-source";

/// One operation of the engine, as it is sent.
pub(crate) trait Operation: Serialize + DeserializeOwned + Send + 'static {
    const NAME: &'static str;
    type Answer: Serialize + DeserializeOwned + Send;

    /// The tenant it is for, whose scope the engine answers it in.
    fn tenant(&self) -> Option<&str>;

    fn ask(self, engine: &dyn Engine) -> impl Future<Output = Self::Answer> + Send;
}

/// What the engine says of itself, which does not change while it runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct EngineFacts {
    pub available: bool,
    pub queues_compilations: bool,
    pub tests_agents: bool,
}

macro_rules! operations {
    ($(
        $name:ident { $($field:ident : $ty:ty),* $(,)? } -> $answer:ty,
        |$engine:ident, $op:ident| for $tenant:expr => $call:expr;
    )*) => {
        $(
            #[derive(Serialize, Deserialize)]
            pub(crate) struct $name { $(pub $field: $ty,)* }

            impl Operation for $name {
                const NAME: &'static str = stringify!($name);
                type Answer = $answer;

                fn tenant(&self) -> Option<&str> {
                    let $op = self;
                    $tenant
                }

                async fn ask(self, $engine: &dyn Engine) -> $answer {
                    let $op = self;
                    $call
                }
            }
        )*

        /// The name of every operation.
        #[cfg(test)]
        pub(crate) const NAMES: &[&str] = &[$(stringify!($name)),*];

        /// Has `engine` answer the operation `name` sent as `body`. `None` if
        /// there is no such operation; `Err` if the body is not one, or the
        /// answer cannot be written.
        pub(crate) async fn answer(
            engine: &dyn Engine,
            name: &str,
            body: &[u8],
        ) -> Option<Result<Vec<u8>, String>> {
            match name {
                $(stringify!($name) => Some(match serde_json::from_slice::<$name>(body) {
                    Err(error) => Err(error.to_string()),
                    Ok(operation) => {
                        let answer = match operation.tenant().map(str::to_owned) {
                            Some(tenant) => scoped(tenant, operation.ask(engine)).await,
                            None => operation.ask(engine).await,
                        };
                        serde_json::to_vec(&answer).map_err(|error| error.to_string())
                    }
                }),)*
                _ => None,
            }
        }
    };
}

operations! {
    Facts {} -> EngineFacts,
    |engine, _op| for None => EngineFacts {
        available: engine.available(),
        queues_compilations: engine.queues_compilations(),
        tests_agents: engine.tests_agents(),
    };

    // ---- Launch ---------------------------------------------------------------
    Queue {
        run_label: Option<String>,
        tenant_id: String,
        workflow_id: String,
        version: Option<i32>,
        inputs: Value,
        debug: bool,
        correlation_id: Option<String>,
        idempotency_key: Option<String>,
        trigger_source: TriggerSource,
        instance_id: Option<Uuid>,
    } -> Result<QueuedExecution, ExecutionError>,
    |engine, op| for Some(&op.tenant_id) => engine.queue(QueueRequest {
        run_label: op.run_label,
        tenant_id: &op.tenant_id,
        workflow_id: &op.workflow_id,
        version: op.version,
        inputs: op.inputs,
        debug: op.debug,
        correlation_id: op.correlation_id,
        idempotency_key: op.idempotency_key,
        trigger_source: op.trigger_source,
        instance_id: op.instance_id,
    }).await;

    RunSync {
        run_label: Option<String>,
        tenant_id: String,
        workflow_id: String,
        version: Option<i32>,
        inputs: Value,
    } -> Result<SyncExecution, ExecutionError>,
    |engine, op| for Some(&op.tenant_id) => engine.run_sync(SyncRequest {
        run_label: op.run_label,
        tenant_id: &op.tenant_id,
        workflow_id: &op.workflow_id,
        version: op.version,
        inputs: op.inputs,
    }).await;

    EnqueueTriggerEvent { tenant_id: String, event: TriggerEvent, idempotency_key: String }
        -> Result<EnqueuedExecution, ExecutionError>,
    |engine, op| for Some(&op.tenant_id) =>
        engine.enqueue_trigger_event(&op.tenant_id, op.event, op.idempotency_key).await;

    Replay { tenant_id: String, original_instance_id: String }
        -> Result<QueuedExecution, ExecutionError>,
    |engine, op| for Some(&op.tenant_id) =>
        engine.replay(&op.tenant_id, &op.original_instance_id).await;

    // ---- Lifecycle ------------------------------------------------------------
    Stop { tenant_id: String, instance_id: String } -> Result<StopOutcome, ExecutionError>,
    |engine, op| for Some(&op.tenant_id) => engine.stop(&op.tenant_id, &op.instance_id).await;

    Pause { tenant_id: String, instance_id: String } -> Result<PauseOutcome, ExecutionError>,
    |engine, op| for Some(&op.tenant_id) => engine.pause(&op.tenant_id, &op.instance_id).await;

    Resume { tenant_id: String, instance_id: String } -> Result<ResumeOutcome, ExecutionError>,
    |engine, op| for Some(&op.tenant_id) => engine.resume(&op.tenant_id, &op.instance_id).await;

    // ---- Runs, as the API shows them ----------------------------------------------
    GetExecution { tenant_id: String, instance_id: String }
        -> Result<WorkflowInstanceDto, ExecutionError>,
    |engine, op| for Some(&op.tenant_id) =>
        engine.get_execution(&op.tenant_id, &op.instance_id).await;

    GetExecutionWithMetadata { workflow_id: String, instance_id: String, tenant_id: String }
        -> Result<ExecutionWithMetadata, ExecutionError>,
    |engine, op| for Some(&op.tenant_id) => engine
        .get_execution_with_metadata(&op.workflow_id, &op.instance_id, &op.tenant_id)
        .await;

    ListExecutions { tenant_id: String, workflow_id: String, page: Option<i32>, size: Option<i32> }
        -> Result<PageWorkflowInstanceHistoryDto, ExecutionError>,
    |engine, op| for Some(&op.tenant_id) => engine
        .list_executions(&op.tenant_id, &op.workflow_id, op.page, op.size)
        .await;

    ListAllExecutions {
        tenant_id: String,
        page: Option<i32>,
        size: Option<i32>,
        filters: ExecutionFilters,
    } -> Result<PageWorkflowInstanceHistoryDto, ExecutionError>,
    |engine, op| for Some(&op.tenant_id) => engine
        .list_all_executions(&op.tenant_id, op.page, op.size, op.filters)
        .await;

    ExecutionSummaryOf { tenant_id: String, filters: ExecutionFilters }
        -> Result<ExecutionSummary, ExecutionError>,
    |engine, op| for Some(&op.tenant_id) =>
        engine.execution_summary(&op.tenant_id, op.filters).await;

    ExecutionListingOptions { tenant_id: String, page: i32, size: i32, filters: ExecutionFilters }
        -> Result<ListInstancesOptions, ExecutionError>,
    |engine, op| for Some(&op.tenant_id) => engine
        .execution_listing_options(&op.tenant_id, op.page, op.size, &op.filters)
        .await;

    AuthorizeExecution { workflow_id: String, instance_id: String, tenant_id: String }
        -> Result<InstanceInfo, ExecutionError>,
    |engine, op| for Some(&op.tenant_id) => engine
        .authorize_execution(&op.workflow_id, &op.instance_id, &op.tenant_id)
        .await;

    // ---- A run's own records ------------------------------------------------------
    GetInstanceInfo { tenant_id: String, instance_id: String } -> Result<InstanceInfo, RuntimeError>,
    |engine, op| for Some(&op.tenant_id) =>
        engine.get_instance_info(&op.tenant_id, &op.instance_id).await;

    ListEvents { tenant_id: String, instance_id: String, options: Option<ListEventsOptions> }
        -> Result<ListEventsResult, RuntimeError>,
    |engine, op| for Some(&op.tenant_id) =>
        engine.list_events(&op.tenant_id, &op.instance_id, op.options).await;

    ListStepSummaries {
        tenant_id: String,
        instance_id: String,
        options: Option<ListStepSummariesOptions>,
    } -> Result<ListStepSummariesResult, RuntimeError>,
    |engine, op| for Some(&op.tenant_id) =>
        engine.list_step_summaries(&op.tenant_id, &op.instance_id, op.options).await;

    GetScopeAncestors { tenant_id: String, instance_id: String, scope_id: String }
        -> Result<Vec<ScopeInfo>, RuntimeError>,
    |engine, op| for Some(&op.tenant_id) => engine
        .get_scope_ancestors(&op.tenant_id, &op.instance_id, &op.scope_id)
        .await;

    ListCheckpoints {
        tenant_id: String,
        instance_id: String,
        limit: Option<u32>,
        offset: Option<u32>,
    } -> Result<ListCheckpointsResult, RuntimeError>,
    |engine, op| for Some(&op.tenant_id) => engine
        .list_checkpoints(&op.tenant_id, &op.instance_id, op.limit, op.offset)
        .await;

    GetTenantMetrics { options: GetTenantMetricsOptions } -> Result<TenantMetricsResult, RuntimeError>,
    |engine, op| for Some(&op.options.tenant_id) => engine.get_tenant_metrics(op.options).await;

    OperationRequests {
        tenant_id: String,
        workflow_id: String,
        key: String,
        listing: ListInstancesOptions,
        fields: Vec<String>,
    } -> Result<runtara_environment::operations::RequestPage, RuntimeError>,
    |engine, op| for Some(&op.tenant_id) => engine
        .operation_requests(&op.tenant_id, &op.workflow_id, &op.key, &op.listing, &op.fields)
        .await;

    // ---- Input a run waits for ------------------------------------------------
    ListInputRequests { tenant_id: String, instances: Vec<String>, offset: u64, limit: u32 }
        -> InputResult<InputRequestPage>,
    |engine, op| for Some(&op.tenant_id) => engine
        .list_input_requests(&op.tenant_id, &op.instances, op.offset, op.limit)
        .await;

    GetInputRequest { tenant_id: String, instance_id: String, request: String }
        -> InputResult<InputRequest>,
    |engine, op| for Some(&op.tenant_id) => engine
        .get_input_request(&op.tenant_id, &op.instance_id, &op.request)
        .await;

    SubmitInputResponse {
        tenant_id: String,
        instance_id: String,
        request: String,
        operation: String,
        payload: Value,
    } -> InputResult<InputReceipt>,
    |engine, op| for Some(&op.tenant_id) => engine
        .submit_input_response(
            &op.tenant_id,
            &op.instance_id,
            &op.request,
            &op.operation,
            &op.payload,
        )
        .await;

    SubmitAuthenticatedInputResponse {
        auth: AuthContext,
        instance_id: String,
        request: String,
        operation: String,
        payload: Value,
    } -> InputResult<InputReceipt>,
    |engine, op| for Some(&op.auth.org_id) => engine
        .submit_authenticated_input_response(
            &op.auth,
            &op.instance_id,
            &op.request,
            &op.operation,
            &op.payload,
        )
        .await;

    PendingInputPageOf { tenant_id: String, instance_id: String } -> InputResult<PendingInputPage>,
    |engine, op| for Some(&op.tenant_id) =>
        engine.pending_input_page(&op.tenant_id, &op.instance_id).await;

    // ---- Actions a workflow offers while it waits ------------------------------
    ListInstanceActions { tenant_id: String, workflow_id: String, instance_id: String }
        -> Result<Vec<WorkflowRuntimeAction>, WorkflowRuntimeError>,
    |engine, op| for Some(&op.tenant_id) => engine
        .list_instance_actions(&op.tenant_id, &op.workflow_id, &op.instance_id)
        .await;

    ListWorkflowActions {
        tenant_id: String,
        workflow_id: String,
        page: Option<i32>,
        size: Option<i32>,
    } -> Result<WorkflowRuntimeActionPage, WorkflowRuntimeError>,
    |engine, op| for Some(&op.tenant_id) => engine
        .list_workflow_actions(&op.tenant_id, &op.workflow_id, op.page, op.size)
        .await;

    SubmitWorkflowAction {
        auth: AuthContext,
        workflow_id: String,
        instance_id: String,
        request_id: String,
        operation_id: String,
        payload: Value,
    } -> Result<WorkflowActionReceipt, WorkflowRuntimeError>,
    |engine, op| for Some(&op.auth.org_id) => engine
        .submit_workflow_action(
            &op.auth,
            &op.workflow_id,
            &op.instance_id,
            &op.request_id,
            &op.operation_id,
            &op.payload,
        )
        .await;

    SubmitAuthenticatedInput {
        auth: AuthContext,
        instance_id: String,
        request_id: String,
        operation_id: String,
        payload: Value,
    } -> Result<WorkflowActionReceipt, WorkflowRuntimeError>,
    |engine, op| for Some(&op.auth.org_id) => engine
        .submit_authenticated_input(
            &op.auth,
            &op.instance_id,
            &op.request_id,
            &op.operation_id,
            &op.payload,
        )
        .await;

    OperationQueues { tenant_id: String } -> Result<Vec<OperationQueue>, WorkflowRuntimeError>,
    |engine, op| for Some(&op.tenant_id) => engine.operation_queues(&op.tenant_id).await;

    // ---- Compilation ------------------------------------------------------------
    CompileWorkflow { tenant_id: String, workflow_id: String, version: i32, force_recompile: bool }
        -> Result<CompilationResultDto, CompilationError>,
    |engine, op| for Some(&op.tenant_id) => engine
        .compile_workflow(&op.tenant_id, &op.workflow_id, op.version, op.force_recompile)
        .await;

    PublishWorkflowAgent { tenant_id: String, workflow_id: String, version: i32, slug: String }
        -> Result<Value, CompilationError>,
    |engine, op| for Some(&op.tenant_id) => engine
        .publish_workflow_agent(&op.tenant_id, &op.workflow_id, op.version, op.slug)
        .await;

    GetImage { tenant_id: String, image_id: String } -> Result<Option<ImageSummary>, RuntimeError>,
    |engine, op| for Some(&op.tenant_id) => engine.get_image(&op.tenant_id, &op.image_id).await;

    // ---- Connections, as a workflow names them -----------------------------------
    TenantConnections { tenant_id: String } -> Result<Vec<ConnectionRef>, String>,
    |engine, op| for Some(&op.tenant_id) => engine.tenant_connections(&op.tenant_id).await;

    ReconcileChannelWebhooks { tenant_id: String, removed: Vec<InvocationTrigger> } -> (),
    |engine, op| for Some(&op.tenant_id) =>
        engine.reconcile_channel_webhooks(&op.tenant_id, &op.removed).await;

    // ---- Compilation, through the queue ------------------------------------------
    EnqueueCompilation {
        tenant_id: String,
        workflow_id: String,
        version: i32,
        force_recompile: bool,
        compiled: Option<ProductEvent>,
    } -> Result<bool, CompilationQueueError>,
    |engine, op| for Some(&op.tenant_id) => engine
        .enqueue_compilation(
            &op.tenant_id,
            &op.workflow_id,
            op.version,
            op.force_recompile,
            op.compiled,
        )
        .await;

    WaitForCompilation { tenant_id: String, workflow_id: String, version: i32, timeout_ms: u64 }
        -> Result<bool, CompilationQueueError>,
    |engine, op| for Some(&op.tenant_id) => engine
        .wait_for_compilation(
            &op.tenant_id,
            &op.workflow_id,
            op.version,
            std::time::Duration::from_millis(op.timeout_ms),
        )
        .await;

    IsCompilationPending { tenant_id: String, workflow_id: String, version: i32 }
        -> Result<bool, CompilationQueueError>,
    |engine, op| for Some(&op.tenant_id) => engine
        .is_compilation_pending(&op.tenant_id, &op.workflow_id, op.version)
        .await;

    MarkCompilationQueued { tenant_id: String, workflow_id: String, version: i32 } -> (),
    |engine, op| for Some(&op.tenant_id) => engine
        .mark_compilation_queued(&op.tenant_id, &op.workflow_id, op.version)
        .await;

    CompilationProgressOf { tenant_id: String, workflow_id: String, version: i32 }
        -> Option<CompilationProgress>,
    |engine, op| for Some(&op.tenant_id) => engine
        .compilation_progress(&op.tenant_id, &op.workflow_id, op.version)
        .await;

    // ---- Running an agent on request ----------------------------------------------
    TestAgent {
        tenant_id: String,
        agent_name: String,
        capability_id: String,
        input: Value,
        connection_id: Option<String>,
        engine: TestEngine,
    } -> Result<TestResult, AgentTestError>,
    |engine, op| for Some(&op.tenant_id) => engine
        .test_agent(
            &op.tenant_id,
            &op.agent_name,
            &op.capability_id,
            op.input,
            op.connection_id,
            op.engine,
        )
        .await;
}
