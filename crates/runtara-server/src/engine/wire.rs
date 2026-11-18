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
use crate::authz::Permission;
use crate::workers::execution_engine::TriggerSource;

/// The path of an operation is this, then its name.
pub(crate) const OPERATIONS: &str = "/engine/v1/operations/";
/// The path of a request to a part of the API the engine serves is this, the
/// part's name, then the request's own path.
pub(crate) const SERVED: &str = "/engine/v1/served/";

/// What the caller presented to the API ([`crate::auth::presented`]), on
/// every call made for one. The engine verifies it itself.
pub(crate) const PRESENTED: &str = "x-runtara-presented";
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

    /// What the caller's role and credential must allow, any one of them,
    /// for the operation to be done. Empty for an operation that reads: any
    /// member of the tenant may ask it.
    const NEEDS: &'static [Permission];

    /// Makes the caller the operation names the one the engine found. Only an
    /// operation that names its caller does anything.
    fn of_caller(&mut self, caller: &AuthContext);

    fn ask(self, engine: &dyn Engine) -> impl Future<Output = Self::Answer> + Send;
}

/// Why an operation was not answered.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Refused {
    /// Its body is not the operation, or the answer could not be written.
    Unreadable(String),
    /// It is for a tenant and came with nothing presented.
    NoCaller,
    /// It is for a tenant its caller does not act for.
    NotTheCallers,
    /// The caller's role, or what its credential is held to, does not allow
    /// it.
    NotPermitted,
}

/// What the engine says of itself, which does not change while it runs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct EngineFacts {
    pub available: bool,
    pub queues_compilations: bool,
    pub tests_agents: bool,
    /// The trusted built-in versions it runs, by which the API tells whether
    /// a compiled workflow can still run.
    pub trusted_pins: Vec<String>,
    /// What it compiles by, by which the API tells whether a compiled
    /// workflow is current.
    pub compiles_by: crate::api::repositories::workflows::CompilesBy,
}

/// Whether `caller` may do what needs any one of `needs`: by its role, as
/// the API's route gate decides it, and by what its credential is held to.
fn permitted(caller: &AuthContext, needs: &[Permission]) -> bool {
    if needs.is_empty() {
        return true;
    }
    let policy = crate::auth::membership_policy();
    needs.iter().any(|permission| {
        caller
            .api_key_scope
            .permits(Some(*permission), &axum::http::Method::POST)
            && crate::middleware::authorization::require_permission(
                policy,
                caller.role,
                *permission,
            )
            .is_ok()
    })
}

macro_rules! operations {
    ($(
        $name:ident { $($field:ident : $ty:ty),* $(,)? } -> $answer:ty,
        $(needs [$($needs:ident),+],)?
        |$engine:ident, $op:ident| for $tenant:expr $(, as $caller:ident)? => $call:expr;
    )*) => {
        $(
            #[derive(Serialize, Deserialize)]
            pub(crate) struct $name { $(pub $field: $ty,)* }

            impl Operation for $name {
                const NAME: &'static str = stringify!($name);
                type Answer = $answer;
                const NEEDS: &'static [Permission] = &[$($(Permission::$needs),+)?];

                fn tenant(&self) -> Option<&str> {
                    let $op = self;
                    $tenant
                }

                fn of_caller(&mut self, _caller: &AuthContext) {
                    $(self.$caller = _caller.clone();)?
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

        /// Has `engine` answer the operation `name` sent as `body` for
        /// `caller`, whom the engine verified. `None` if there is no such
        /// operation.
        ///
        /// An operation for a tenant is answered only for a caller who acts
        /// for that tenant, in that tenant's scope.
        pub(crate) async fn answer(
            engine: &dyn Engine,
            name: &str,
            body: &[u8],
            caller: Option<&AuthContext>,
        ) -> Option<Result<Vec<u8>, Refused>> {
            match name {
                $(stringify!($name) => Some(match serde_json::from_slice::<$name>(body) {
                    Err(error) => Err(Refused::Unreadable(error.to_string())),
                    Ok(mut operation) => {
                        let answer = match operation.tenant().map(str::to_owned) {
                            Some(tenant) => {
                                match caller {
                                    None => return Some(Err(Refused::NoCaller)),
                                    Some(caller) if caller.org_id != tenant => {
                                        return Some(Err(Refused::NotTheCallers));
                                    }
                                    Some(caller) if !permitted(caller, <$name>::NEEDS) => {
                                        return Some(Err(Refused::NotPermitted));
                                    }
                                    Some(caller) => operation.of_caller(caller),
                                }
                                let asker = caller.map(|caller| caller.user_id.clone());
                                scoped(tenant, async {
                                    match asker {
                                        Some(asker) => {
                                            approvals::asked_by(asker, operation.ask(engine)).await
                                        }
                                        None => operation.ask(engine).await,
                                    }
                                })
                                .await
                            }
                            None => operation.ask(engine).await,
                        };
                        serde_json::to_vec(&answer)
                            .map_err(|error| Refused::Unreadable(error.to_string()))
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
        trusted_pins: crate::api::repositories::workflows::installed_trusted_pins(),
        compiles_by: crate::api::repositories::workflows::own_compiles_by(),
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
    needs [WorkflowExecute],
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
    needs [WorkflowExecute],
    |engine, op| for Some(&op.tenant_id) => engine.run_sync(SyncRequest {
        run_label: op.run_label,
        tenant_id: &op.tenant_id,
        workflow_id: &op.workflow_id,
        version: op.version,
        inputs: op.inputs,
    }).await;

    EnqueueTriggerEvent { tenant_id: String, event: TriggerEvent, idempotency_key: String }
        -> Result<EnqueuedExecution, ExecutionError>,
    needs [WorkflowExecute],
    |engine, op| for Some(&op.tenant_id) =>
        engine.enqueue_trigger_event(&op.tenant_id, op.event, op.idempotency_key).await;

    Replay { tenant_id: String, original_instance_id: String }
        -> Result<QueuedExecution, ExecutionError>,
    needs [WorkflowExecute],
    |engine, op| for Some(&op.tenant_id) =>
        engine.replay(&op.tenant_id, &op.original_instance_id).await;

    // ---- Lifecycle ------------------------------------------------------------
    Stop { tenant_id: String, instance_id: String } -> Result<StopOutcome, ExecutionError>,
    needs [WorkflowExecute],
    |engine, op| for Some(&op.tenant_id) => engine.stop(&op.tenant_id, &op.instance_id).await;

    Pause { tenant_id: String, instance_id: String } -> Result<PauseOutcome, ExecutionError>,
    needs [WorkflowExecute],
    |engine, op| for Some(&op.tenant_id) => engine.pause(&op.tenant_id, &op.instance_id).await;

    Resume { tenant_id: String, instance_id: String } -> Result<ResumeOutcome, ExecutionError>,
    needs [WorkflowExecute],
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
    needs [WorkflowExecute],
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
    needs [WorkflowExecute],
    |engine, op| for Some(&op.auth.org_id), as auth => engine
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
    needs [WorkflowExecute],
    |engine, op| for Some(&op.auth.org_id), as auth => engine
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
    needs [WorkflowExecute],
    |engine, op| for Some(&op.auth.org_id), as auth => engine
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
    needs [WorkflowExecute, WorkflowCreate, WorkflowUpdate],
    |engine, op| for Some(&op.tenant_id) => engine
        .compile_workflow(&op.tenant_id, &op.workflow_id, op.version, op.force_recompile)
        .await;

    PublishWorkflowAgent { tenant_id: String, workflow_id: String, version: i32, slug: String }
        -> Result<Value, CompilationError>,
    needs [WorkflowExecute, WorkflowCreate, WorkflowUpdate],
    |engine, op| for Some(&op.tenant_id) => engine
        .publish_workflow_agent(&op.tenant_id, &op.workflow_id, op.version, op.slug)
        .await;

    GetImage { tenant_id: String, image_id: String } -> Result<Option<ImageSummary>, RuntimeError>,
    |engine, op| for Some(&op.tenant_id) => engine.get_image(&op.tenant_id, &op.image_id).await;

    // ---- Connections, as a workflow names them -----------------------------------
    TenantConnections { tenant_id: String } -> Result<Vec<ConnectionRef>, String>,
    |engine, op| for Some(&op.tenant_id) => engine.tenant_connections(&op.tenant_id).await;

    ReconcileChannelWebhooks { tenant_id: String, removed: Vec<InvocationTrigger> } -> (),
    needs [WorkflowDelete],
    |engine, op| for Some(&op.tenant_id) =>
        engine.reconcile_channel_webhooks(&op.tenant_id, &op.removed).await;

    WorkflowAgents { tenant_id: String } -> Vec<runtara_dsl::agent_meta::AgentInfo>,
    |engine, op| for Some(&op.tenant_id) => engine.workflow_agents(&op.tenant_id).await;

    UnstageWorkflowAgent { tenant_id: String, slug: String } -> Result<(), String>,
    needs [WorkflowDelete],
    |engine, op| for Some(&op.tenant_id) =>
        engine.unstage_workflow_agent(&op.tenant_id, &op.slug).await;

    SetCurrentVersion { tenant_id: String, workflow_id: String, version: i32 }
        -> Result<bool, String>,
    needs [WorkflowUpdate, WorkflowCreate],
    |engine, op| for Some(&op.tenant_id) => engine
        .set_current_version(&op.tenant_id, &op.workflow_id, op.version)
        .await;

    InvalidateCompilation { tenant_id: String, workflow_id: String, version: i32 }
        -> Result<(), String>,
    needs [WorkflowExecute, WorkflowCreate, WorkflowUpdate],
    |engine, op| for Some(&op.tenant_id) => engine
        .invalidate_compilation(&op.tenant_id, &op.workflow_id, op.version)
        .await;

    // ---- Compilation, through the queue ------------------------------------------
    EnqueueCompilation {
        tenant_id: String,
        workflow_id: String,
        version: i32,
        force_recompile: bool,
        compiled: Option<ProductEvent>,
    } -> Result<bool, CompilationQueueError>,
    needs [WorkflowExecute, WorkflowCreate, WorkflowUpdate],
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
    needs [WorkflowExecute, WorkflowCreate, WorkflowUpdate],
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
    needs [WorkflowExecute],
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
