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
    /// for the operation to be answered: the permissions of the API's routes
    /// that ask it (`middleware::authorization::permission_for`). An
    /// operation several routes ask names each route's. Empty only where the
    /// API's own route for the same thing is open to every caller.
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
    /// It is not one this peer may ask: the API asks for callers, and intake
    /// asks the few things anyone may have done.
    NotThePeers,
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

/// The operations a conversation token may ask: its conversation's state,
/// messages, events and stop.
const ASKED_WITH_A_CONVERSATION_TOKEN: &[&str] = &[
    "ConversationOf",
    "SendConversationMessage",
    "ConversationEvents",
    "StopConversation",
];

/// `operation`, as a conversation token's holder may ask it: one of
/// [`ASKED_WITH_A_CONVERSATION_TOKEN`], of the grant's conversation, and a
/// message sent as the grant's sender whoever the API says sent it. `None`
/// if the token may not ask it.
fn held_to<O: Serialize + serde::de::DeserializeOwned>(
    grant: &crate::auth::ConversationGrant,
    name: &str,
    operation: &O,
) -> Option<O> {
    if !ASKED_WITH_A_CONVERSATION_TOKEN.contains(&name) {
        return None;
    }
    let mut asked = serde_json::to_value(operation).ok()?;
    if asked.get("workflow_id")?.as_str()? != grant.workflow_id
        || asked.get("key")?.as_str()? != grant.key
    {
        return None;
    }
    if let Some(message) = asked
        .get_mut("message")
        .and_then(serde_json::Value::as_object_mut)
    {
        match &grant.sender {
            Some(sender) => {
                message.insert("sender".into(), serde_json::to_value(sender).ok()?);
            }
            None => {
                message.remove("sender");
            }
        }
    }
    serde_json::from_value(asked).ok()
}

/// Who is on the other end of the engine's listener, by its certificate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Peer {
    /// The API role, which asks for the callers it authenticated.
    Api,
    /// The intake role, which receives what anyone may send.
    Intake,
}

/// The operations intake asks, and nobody else. Each does for intake what
/// the engine does for whoever sends the request intake received: there is
/// no caller behind it, and it gives intake nothing a sender does not have.
pub(crate) const ASKED_BY_INTAKE: &[&str] = &["RunSyncForAnyone", "OAuthRedirectForAnyone"];

macro_rules! operations {
    ($(
        $name:ident { $($field:ident : $ty:ty),* $(,)? } -> $answer:ty,
        needs [$($needs:ident),*],
        |$engine:ident, $op:ident| for $tenant:expr $(, as $caller:ident)? => $call:expr;
    )*) => {
        $(
            #[derive(Serialize, Deserialize)]
            pub(crate) struct $name { $(pub $field: $ty,)* }

            impl Operation for $name {
                const NAME: &'static str = stringify!($name);
                type Answer = $answer;
                const NEEDS: &'static [Permission] = &[$(Permission::$needs),*];

                fn tenant(&self) -> Option<&str> {
                    #[allow(unused_variables)]
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

        /// What each operation needs, by its name.
        #[cfg(test)]
        pub(crate) const NEEDS: &[(&str, &[Permission])] =
            &[$((stringify!($name), <$name as Operation>::NEEDS)),*];

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
            peer: Peer,
        ) -> Option<Result<Vec<u8>, Refused>> {
            let for_anyone = ASKED_BY_INTAKE.contains(&name);
            match name {
                $(stringify!($name) if for_anyone != (peer == Peer::Intake) => {
                    Some(Err(Refused::NotThePeers))
                })*
                $(stringify!($name) => Some(match serde_json::from_slice::<$name>(body) {
                    Err(error) => Err(Refused::Unreadable(error.to_string())),
                    Ok(mut operation) => {
                        let answer = match operation.tenant().map(str::to_owned) {
                            // What anyone may have done has no caller to
                            // verify: it is answered in its tenant's scope.
                            Some(tenant) if for_anyone => {
                                scoped(tenant, operation.ask(engine)).await
                            }
                            Some(tenant) => {
                                match caller {
                                    None => return Some(Err(Refused::NoCaller)),
                                    Some(caller) if caller.org_id != tenant => {
                                        return Some(Err(Refused::NotTheCallers));
                                    }
                                    Some(caller) if !permitted(caller, <$name>::NEEDS) => {
                                        return Some(Err(Refused::NotPermitted));
                                    }
                                    Some(caller) => {
                                        // A conversation token asks of its
                                        // conversation and nothing else.
                                        if let Some(grant) = &caller.conversation {
                                            match held_to(grant, stringify!($name), &operation) {
                                                Some(held) => operation = held,
                                                None => return Some(Err(Refused::NotPermitted)),
                                            }
                                        }
                                        operation.of_caller(caller)
                                    }
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
    needs [],
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

    // Finish an OAuth flow a provider sent a user back from: what the
    // redirect does for whoever follows it. Intake asks it, having received
    // one, and hands over what was asked (a `GET` of a path and its query)
    // and the browser's cookie, and nothing else of the request. No tenant is named: the redirect
    // names its own, and the engine answers it in that tenant's scope.
    OAuthRedirectForAnyone { path_and_query: String, cookie: Option<String> }
        -> Result<HandedBack, String>,
    needs [],
    |engine, op| for None =>
        engine.finish_oauth_redirect(op.path_and_query, op.cookie).await;

    // Run a workflow and wait, for anyone: what the synchronous trigger does
    // for whoever calls it with a workflow's id. Intake asks it, having
    // received such a call. No version is named: the engine runs what it
    // approved.
    RunSyncForAnyone {
        run_label: Option<String>,
        tenant_id: String,
        workflow_id: String,
        inputs: Value,
    } -> Result<SyncExecution, ExecutionError>,
    needs [],
    |engine, op| for Some(&op.tenant_id) => engine.run_sync(SyncRequest {
        run_label: op.run_label,
        tenant_id: &op.tenant_id,
        workflow_id: &op.workflow_id,
        version: None,
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
    needs [InvocationHistoryRead],
    |engine, op| for Some(&op.tenant_id) =>
        engine.get_execution(&op.tenant_id, &op.instance_id).await;

    GetExecutionWithMetadata { workflow_id: String, instance_id: String, tenant_id: String }
        -> Result<ExecutionWithMetadata, ExecutionError>,
    needs [InvocationHistoryRead],
    |engine, op| for Some(&op.tenant_id) => engine
        .get_execution_with_metadata(&op.workflow_id, &op.instance_id, &op.tenant_id)
        .await;

    ListExecutions { tenant_id: String, workflow_id: String, page: Option<i32>, size: Option<i32> }
        -> Result<PageWorkflowInstanceHistoryDto, ExecutionError>,
    needs [InvocationHistoryRead],
    |engine, op| for Some(&op.tenant_id) => engine
        .list_executions(&op.tenant_id, &op.workflow_id, op.page, op.size)
        .await;

    ListAllExecutions {
        tenant_id: String,
        page: Option<i32>,
        size: Option<i32>,
        filters: ExecutionFilters,
    } -> Result<PageWorkflowInstanceHistoryDto, ExecutionError>,
    needs [InvocationHistoryRead],
    |engine, op| for Some(&op.tenant_id) => engine
        .list_all_executions(&op.tenant_id, op.page, op.size, op.filters)
        .await;

    ExecutionSummaryOf { tenant_id: String, filters: ExecutionFilters }
        -> Result<ExecutionSummary, ExecutionError>,
    needs [InvocationHistoryRead],
    |engine, op| for Some(&op.tenant_id) =>
        engine.execution_summary(&op.tenant_id, op.filters).await;

    ExecutionListingOptions { tenant_id: String, page: i32, size: i32, filters: ExecutionFilters }
        -> Result<ListInstancesOptions, ExecutionError>,
    needs [InvocationHistoryRead],
    |engine, op| for Some(&op.tenant_id) => engine
        .execution_listing_options(&op.tenant_id, op.page, op.size, &op.filters)
        .await;

    AuthorizeExecution { workflow_id: String, instance_id: String, tenant_id: String }
        -> Result<InstanceInfo, ExecutionError>,
    needs [InvocationHistoryRead],
    |engine, op| for Some(&op.tenant_id) => engine
        .authorize_execution(&op.workflow_id, &op.instance_id, &op.tenant_id)
        .await;

    // ---- A run's own records ------------------------------------------------------
    GetInstanceInfo { tenant_id: String, instance_id: String } -> Result<InstanceInfo, RuntimeError>,
    needs [InvocationHistoryRead],
    |engine, op| for Some(&op.tenant_id) =>
        engine.get_instance_info(&op.tenant_id, &op.instance_id).await;

    ListEvents { tenant_id: String, instance_id: String, options: Option<ListEventsOptions> }
        -> Result<ListEventsResult, RuntimeError>,
    needs [InvocationHistoryRead],
    |engine, op| for Some(&op.tenant_id) =>
        engine.list_events(&op.tenant_id, &op.instance_id, op.options).await;

    ListStepSummaries {
        tenant_id: String,
        instance_id: String,
        options: Option<ListStepSummariesOptions>,
    } -> Result<ListStepSummariesResult, RuntimeError>,
    needs [InvocationHistoryRead],
    |engine, op| for Some(&op.tenant_id) =>
        engine.list_step_summaries(&op.tenant_id, &op.instance_id, op.options).await;

    GetScopeAncestors { tenant_id: String, instance_id: String, scope_id: String }
        -> Result<Vec<ScopeInfo>, RuntimeError>,
    needs [InvocationHistoryRead],
    |engine, op| for Some(&op.tenant_id) => engine
        .get_scope_ancestors(&op.tenant_id, &op.instance_id, &op.scope_id)
        .await;

    ListCheckpoints {
        tenant_id: String,
        instance_id: String,
        limit: Option<u32>,
        offset: Option<u32>,
    } -> Result<ListCheckpointsResult, RuntimeError>,
    needs [InvocationHistoryRead],
    |engine, op| for Some(&op.tenant_id) => engine
        .list_checkpoints(&op.tenant_id, &op.instance_id, op.limit, op.offset)
        .await;

    GetTenantMetrics { options: GetTenantMetricsOptions } -> Result<TenantMetricsResult, RuntimeError>,
    needs [AnalyticsRead],
    |engine, op| for Some(&op.options.tenant_id) => engine.get_tenant_metrics(op.options).await;

    OperationRequests {
        tenant_id: String,
        workflow_id: String,
        key: String,
        listing: ListInstancesOptions,
        fields: Vec<String>,
    } -> Result<runtara_environment::operations::RequestPage, RuntimeError>,
    needs [InvocationHistoryRead],
    |engine, op| for Some(&op.tenant_id) => engine
        .operation_requests(&op.tenant_id, &op.workflow_id, &op.key, &op.listing, &op.fields)
        .await;

    // ---- Input a run waits for ------------------------------------------------
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
    needs [InvocationHistoryRead],
    |engine, op| for Some(&op.tenant_id) =>
        engine.pending_input_page(&op.tenant_id, &op.instance_id).await;

    // ---- Conversations with a workflow -----------------------------------------
    OpenConversationWith {
        tenant_id: String,
        workflow_id: String,
        key: String,
        open: OpenConversation,
    } -> ConversationResult<ConversationState>,
    needs [WorkflowExecute],
    |engine, op| for Some(&op.tenant_id) => engine
        .open_conversation(&op.tenant_id, &op.workflow_id, &op.key, op.open)
        .await;

    ConversationOf { tenant_id: String, workflow_id: String, key: String }
        -> ConversationResult<ConversationState>,
    needs [InvocationHistoryRead],
    |engine, op| for Some(&op.tenant_id) =>
        engine.conversation(&op.tenant_id, &op.workflow_id, &op.key).await;

    SendConversationMessage {
        tenant_id: String,
        workflow_id: String,
        key: String,
        message: Message,
        // How long to wait for the message's turn, in milliseconds.
        wait_ms: u32,
    } -> ConversationResult<Sent>,
    needs [WorkflowExecute],
    |engine, op| for Some(&op.tenant_id) => engine
        .send_conversation_message(
            &op.tenant_id,
            &op.workflow_id,
            &op.key,
            op.message,
            op.wait_ms,
        )
        .await;

    ConversationEvents {
        tenant_id: String,
        workflow_id: String,
        key: String,
        read: ReadEvents,
    } -> ConversationResult<Vec<ConversationEvent>>,
    needs [InvocationHistoryRead],
    |engine, op| for Some(&op.tenant_id) => engine
        .conversation_events(&op.tenant_id, &op.workflow_id, &op.key, op.read)
        .await;

    ConversationsOf { tenant_id: String, workflow_id: String, list: ListConversations }
        -> ConversationResult<ConversationList>,
    needs [InvocationHistoryRead],
    |engine, op| for Some(&op.tenant_id) =>
        engine.conversations_of(&op.tenant_id, &op.workflow_id, op.list).await;

    StopConversation {
        tenant_id: String,
        workflow_id: String,
        key: String,
        clear_queue: bool,
    } -> ConversationResult<Stopped>,
    needs [WorkflowExecute],
    |engine, op| for Some(&op.tenant_id) => engine
        .stop_conversation(&op.tenant_id, &op.workflow_id, &op.key, op.clear_queue)
        .await;

    // A credential for one conversation, made by the caller, who it acts as.
    IssueConversationToken {
        auth: AuthContext,
        workflow_id: String,
        key: String,
        issue: crate::api::services::conversation_tokens::IssueToken,
    } -> ConversationResult<crate::api::services::conversation_tokens::IssuedToken>,
    needs [WorkflowExecute],
    |engine, op| for Some(&op.auth.org_id), as auth => engine
        .issue_conversation_token(op.auth, &op.workflow_id, &op.key, op.issue)
        .await;

    DeleteConversation { tenant_id: String, workflow_id: String, key: String }
        -> ConversationResult<bool>,
    needs [WorkflowExecute],
    |engine, op| for Some(&op.tenant_id) =>
        engine.delete_conversation(&op.tenant_id, &op.workflow_id, &op.key).await;

    // ---- Actions a workflow offers while it waits ------------------------------
    ListInstanceActions { tenant_id: String, workflow_id: String, instance_id: String }
        -> Result<Vec<WorkflowRuntimeAction>, WorkflowRuntimeError>,
    needs [InvocationHistoryRead],
    |engine, op| for Some(&op.tenant_id) => engine
        .list_instance_actions(&op.tenant_id, &op.workflow_id, &op.instance_id)
        .await;

    ListWorkflowActions {
        tenant_id: String,
        workflow_id: String,
        page: Option<i32>,
        size: Option<i32>,
    } -> Result<WorkflowRuntimeActionPage, WorkflowRuntimeError>,
    needs [InvocationHistoryRead],
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
    needs [InvocationHistoryRead],
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
    needs [WorkflowExecute, WorkflowCreate, WorkflowUpdate],
    |engine, op| for Some(&op.tenant_id) => engine.get_image(&op.tenant_id, &op.image_id).await;

    // ---- Connections, as a workflow names them -----------------------------------
    TenantConnections { tenant_id: String } -> Result<Vec<ConnectionRef>, String>,
    needs [WorkflowRead, WorkflowCreate, WorkflowUpdate],
    |engine, op| for Some(&op.tenant_id) => engine.tenant_connections(&op.tenant_id).await;

    // Validating a closure, as saving it does: asked by the validate route
    // and by every save.
    ValidateClosure {
        tenant_id: String,
        root_workflow_id: String,
        root: runtara_dsl::ExecutionGraph,
        children: Vec<runtara_workflows::ClosureChildGraph>,
    } -> Result<runtara_workflows::ClosureValidationReport, String>,
    needs [WorkflowRead, WorkflowCreate, WorkflowUpdate],
    |engine, op| for Some(&op.tenant_id) => engine
        .validate_closure(&op.tenant_id, &op.root_workflow_id, op.root, op.children)
        .await;

    // What agents a tenant published: the API lists agents to every caller
    // (its agent routes are ungated), so any member may ask.
    WorkflowAgents { tenant_id: String } -> Vec<runtara_dsl::agent_meta::AgentInfo>,
    needs [],
    |engine, op| for Some(&op.tenant_id) => engine.workflow_agents(&op.tenant_id).await;

    // What agents the engine loaded and what each can do: the agent routes
    // are ungated too. Which of them a tenant's plan includes is decided by
    // the API, which reads the plan.
    ListAgents { tenant_id: String } -> Result<AgentListing, String>,
    needs [],
    |engine, op| for Some(&op.tenant_id) => engine.list_agents(&op.tenant_id).await;

    AgentNamed { tenant_id: String, name: String, published: bool }
        -> Result<Option<runtara_dsl::agent_meta::AgentInfo>, String>,
    needs [],
    |engine, op| for Some(&op.tenant_id) =>
        engine.agent(&op.tenant_id, &op.name, op.published).await;

    ComponentsStatusOf { tenant_id: String } -> Result<ComponentsStatus, String>,
    needs [],
    |engine, op| for Some(&op.tenant_id) => engine.components_status(&op.tenant_id).await;

    UnstageWorkflowAgent { tenant_id: String, slug: String } -> Result<(), String>,
    needs [WorkflowDelete],
    |engine, op| for Some(&op.tenant_id) =>
        engine.unstage_workflow_agent(&op.tenant_id, &op.slug).await;

    // ---- Who a caller is ---------------------------------------------------------
    // No tenant's and with no caller: it is how a caller is found.
    ApiKeyByHash { key_hash: String } -> Result<crate::api::handlers::api_keys::ApiKey, String>,
    needs [],
    |engine, op| for None => engine.api_key_by_hash(&op.key_hash).await;

    ConversationTokenByHash { token_hash: String }
        -> Result<crate::api::services::conversation_tokens::TokenGrant, String>,
    needs [],
    |engine, op| for None => engine.conversation_token_by_hash(&op.token_hash).await;

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
    needs [WorkflowExecute, WorkflowCreate, WorkflowUpdate],
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
    needs [WorkflowExecute, WorkflowCreate, WorkflowUpdate],
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
    needs [WorkflowRead],
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

#[cfg(test)]
mod conversation_token_tests {
    use super::*;
    use crate::api::services::conversations::{Message, ReadEvents, Sender};
    use crate::auth::ConversationGrant;

    fn grant(sender: Option<Sender>) -> ConversationGrant {
        ConversationGrant {
            workflow_id: "wf-1".into(),
            key: "k".into(),
            sender,
        }
    }

    fn message(sender: Option<Sender>) -> Message {
        Message {
            id: "m-1".into(),
            text: Some("hi".into()),
            payload: None,
            sender,
            attachments: Vec::new(),
            in_reply_to: None,
        }
    }

    fn send(workflow: &str, key: &str, sender: Option<Sender>) -> SendConversationMessage {
        SendConversationMessage {
            tenant_id: "acme".into(),
            workflow_id: workflow.into(),
            key: key.into(),
            message: message(sender),
            wait_ms: 0,
        }
    }

    /// A conversation token asks its own conversation's four operations and
    /// no other; what it sends is from the token's sender, whatever the API
    /// passed on.
    #[test]
    fn a_token_asks_its_conversation_as_its_sender_and_nothing_else() {
        let ann = Sender {
            id: "ann".into(),
            name: Some("Ann".into()),
        };
        let mallory = Sender {
            id: "mallory".into(),
            name: None,
        };
        // Its own conversation, said to be from someone else: from Ann.
        let held = held_to(
            &grant(Some(ann.clone())),
            "SendConversationMessage",
            &send("wf-1", "k", Some(mallory.clone())),
        )
        .expect("its own conversation");
        assert_eq!(held.message.sender, Some(ann));
        // A token made with no sender sends with none.
        let held = held_to(
            &grant(None),
            "SendConversationMessage",
            &send("wf-1", "k", Some(mallory)),
        )
        .expect("its own conversation");
        assert_eq!(held.message.sender, None);
        // Another conversation, or another workflow's: refused.
        for (workflow, key) in [("wf-1", "other"), ("wf-2", "k")] {
            assert!(
                held_to(
                    &grant(None),
                    "SendConversationMessage",
                    &send(workflow, key, None)
                )
                .is_none()
            );
        }
        let read = ConversationEvents {
            tenant_id: "acme".into(),
            workflow_id: "wf-1".into(),
            key: "k".into(),
            read: ReadEvents::default(),
        };
        assert!(held_to(&grant(None), "ConversationEvents", &read).is_some());
        // Its own conversation's other operations: refused.
        let delete = DeleteConversation {
            tenant_id: "acme".into(),
            workflow_id: "wf-1".into(),
            key: "k".into(),
        };
        assert!(held_to(&grant(None), "DeleteConversation", &delete).is_none());
        let list = ConversationsOf {
            tenant_id: "acme".into(),
            workflow_id: "wf-1".into(),
            list: Default::default(),
        };
        assert!(held_to(&grant(None), "ConversationsOf", &list).is_none());
        // Every operation it may ask is one the engine has.
        for name in ASKED_WITH_A_CONVERSATION_TOKEN {
            assert!(NAMES.contains(name), "{name}");
        }
    }
}
